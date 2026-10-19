//! The host profile a run's executor id names (ARCHITECTURE.md §1.4.3):
//! the toolchains the caller's `PATH` resolves and what each prints for
//! `--version`, so a cached run is never served to a machine whose tools
//! differ.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path as FsPath, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aver_core::{HostArch, HostOs, HostProfile, HostTool};

use super::proc::{is_executable, stripped};

/// Toolchain names the profile records, besides the first word of each
/// check command.
pub const TOOLS: &[&str] = &[
    "bash", "bun", "bundle", "cargo", "composer", "deno", "git", "go", "gradle", "java", "javac",
    "make", "mvn", "node", "npm", "npx", "php", "pip", "pip3", "pnpm", "pytest", "python",
    "python3", "rake", "ruby", "rustc", "rustup", "sh", "uv", "yarn",
];

/// The profile of the tools `vars`' `PATH` resolves ([`TOOLS`] and
/// `extra`). `versions` caches what each binary printed, keyed by the path
/// it was found at and stamped with its real file's size, mtime and inode, so a process does not rerun them all (a
/// JVM's `--version` takes a second).
pub fn profile(vars: &BTreeMap<String, String>, extra: &[&str], versions: &FsPath) -> HostProfile {
    let dirs: Vec<PathBuf> = vars
        .get("PATH")
        .map(|p| std::env::split_paths(&OsString::from(p)).collect())
        .unwrap_or_default();
    let mut names: BTreeSet<&str> = TOOLS.iter().copied().collect();
    names.extend(extra.iter().copied());
    let found: Vec<(String, PathBuf)> = names
        .into_iter()
        .filter_map(|name| {
            dirs.iter()
                .filter(|d| d.is_absolute())
                .map(|d| d.join(name))
                .find(|c| is_executable(c))
                .map(|at| (name.to_owned(), at))
        })
        .collect();
    // One process at a time per home, so a later one reads the first one's
    // answers: a probe that timed out under load would otherwise give it
    // another profile, and so another executor id and run cache key. The
    // lock also creates the cache's directory.
    let _probing = super::proc::lock_in_place(&versions.with_extension("lock")).ok();
    let cache = Arc::new(parking_lot::Mutex::new(
        fs::read(versions)
            .ok()
            .and_then(|b| serde_json::from_slice::<BTreeMap<String, (String, String)>>(&b).ok())
            .unwrap_or_default(),
    ));
    let env: Arc<Vec<(String, String)>> = Arc::new(
        vars.iter()
            .filter(|(k, _)| !stripped(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    let handles: Vec<_> = found
        .into_iter()
        .map(|(name, at)| {
            let (cache, env) = (cache.clone(), env.clone());
            std::thread::spawn(move || {
                let real = fs::canonicalize(&at).ok()?;
                // Keyed by the path as invoked: a proxy (rustup's `cargo`
                // and `rustc`) answers per name from one real binary.
                let key = at.to_string_lossy().into_owned();
                let meta = fs::metadata(&real).ok()?;
                let mtime =
                    i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
                let stamp = format!("{}:{mtime}:{}", meta.len(), meta.ino());
                let hit = cache
                    .lock()
                    .get(&key)
                    .filter(|(s, _)| *s == stamp)
                    .and_then(|(_, hex)| data_encoding::HEXLOWER.decode(hex.as_bytes()).ok());
                let version = match hit {
                    Some(v) => v,
                    None => {
                        // The name as the command uses it, so a rustup or
                        // version-manager proxy answers as it will in the run.
                        let v = version_output(&at, &env);
                        cache
                            .lock()
                            .insert(key.clone(), (stamp, data_encoding::HEXLOWER.encode(&v)));
                        v
                    }
                };
                Some(HostTool {
                    name,
                    path: real.to_string_lossy().into_owned(),
                    version,
                })
            })
        })
        .collect();
    let tools: Vec<HostTool> = handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .collect();
    if let Ok(bytes) = serde_json::to_vec(&*cache.lock()) {
        let tmp = versions.with_extension(format!("tmp-{}", ulid::Ulid::generate()));
        if fs::write(&tmp, bytes).is_ok() {
            let _ = fs::rename(&tmp, versions);
        }
    }
    let host = aver_core::HostTarget::current();
    HostProfile::of(
        host.as_ref().map_or(HostOs::Linux, |h| h.os),
        host.as_ref().map_or(HostArch::Amd64, |h| h.arch),
        &tools,
    )
}

/// What a tool prints for `--version` under the caller's environment,
/// bounded by a few seconds.
fn version_output(tool: &FsPath, env: &[(String, String)]) -> Vec<u8> {
    let Ok(mut child) = Command::new(tool)
        .arg("--version")
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    else {
        return Vec::new();
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                // SAFETY: signals the process group spawned above.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut out);
    }
    out
}
