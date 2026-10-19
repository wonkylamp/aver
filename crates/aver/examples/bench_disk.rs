//! Disk workspace benchmark (ARCHITECTURE.md §2.3.1): workspace directory
//! creation and sync on a real repository (date-fns) and on a synthetic
//! 16,000-file, 100k-line TypeScript repository with a 20,000-file
//! `node_modules`, against `git worktree add`.
//!
//! For each repository it imports the commit, then materializes `--count`
//! workspace directories one after another (the first pays for the
//! checkout; with dependencies, the first also adopts the store), times
//! no-change and 10-file syncs, and measures disk by the volume's free-space
//! delta (`statfs`), because `du` counts cloned blocks once per clone.
//! Run through `bench/workspace-disk.sh` (release mode).

use std::fs;
use std::path::{Path as FsPath, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use aver::disk::{CloneStrategy, DiskConfig};
use aver::env::{StaticResolver, TreeEnv};
use aver::git::{self, ImportOptions};
use aver::keys::{self, Home};
use aver::workspace::{MaterializeOptions, Workspaces};
use clap::Parser;
use serde_json::{json, Value};

const SYNTHETIC_DIRS: usize = 100;
const SYNTHETIC_FILES_PER_DIR: usize = 160;
const DEP_PACKAGES: usize = 400;
const DEP_FILES_PER_PACKAGE: usize = 50;
const SYNC_RUNS: usize = 30;
const EDITED_FILES: usize = 10;

#[derive(Parser)]
struct Args {
    /// A Git repository to benchmark (bare or not).
    #[arg(long)]
    repo: Option<PathBuf>,
    /// The commit in --repo.
    #[arg(long, default_value = "HEAD")]
    rev: String,
    /// Benchmark the synthetic repository too (or only, without --repo).
    #[arg(long)]
    synthetic: bool,
    /// Workspace directories to create per repository.
    #[arg(long, default_value_t = 20)]
    count: usize,
    /// Force a clone strategy (clonefile, reflink, copy); default: probe.
    #[arg(long)]
    clone: Option<String>,
    /// Directory for the temporary homes and repositories (must be on the
    /// volume you want to measure).
    #[arg(long)]
    scratch: Option<PathBuf>,
    /// Where to write the JSON results.
    #[arg(long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../../bench/results/disk.json"))]
    out: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; run bench/workspace-disk.sh for release numbers");
    }
    let scratch = match &args.scratch {
        Some(dir) => {
            fs::create_dir_all(dir)?;
            tempfile::tempdir_in(dir)?
        }
        None => tempfile::tempdir()?,
    };
    let clone = args
        .clone
        .as_deref()
        .map(|s| CloneStrategy::parse(s).context("--clone is clonefile, reflink or copy"))
        .transpose()?;
    let mut results = Vec::new();
    if let Some(repo) = &args.repo {
        results.push(bench_repo(
            "repo",
            repo,
            &args.rev,
            None,
            args.count,
            clone,
            scratch.path(),
        )?);
    }
    if args.synthetic || args.repo.is_none() {
        let repo = scratch.path().join("synthetic");
        let deps = synthetic_repo(&repo)?;
        results.push(bench_repo(
            "synthetic",
            &repo,
            "HEAD",
            Some(deps),
            args.count,
            clone,
            scratch.path(),
        )?);
    }
    let out = json!({
        "machine": machine(),
        "count": args.count,
        "results": results,
    });
    if let Some(dir) = args.out.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&args.out, serde_json::to_string_pretty(&out)?)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn bench_repo(
    name: &str,
    repo: &FsPath,
    rev: &str,
    deps_files: Option<u64>,
    count: usize,
    clone: Option<CloneStrategy>,
    scratch: &FsPath,
) -> anyhow::Result<Value> {
    let home_dir = scratch.join(format!("home-{name}"));
    let home = Home::new(&home_dir);
    let init = keys::init(&home)?;
    let store = Arc::new(home.open_store()?);
    let mut config = DiskConfig::under(&home_dir);
    config.clone = clone;
    let ws = Workspaces::new(
        store.clone(),
        Arc::new(init.server.signing_key.clone()),
        Arc::new(TreeEnv(
            Arc::new(StaticResolver::default()),
            aver::env::Detection::Off,
        )),
    )
    .with_disk_config(config);
    let source = repo.to_string_lossy().into_owned();
    let import = git::import(
        &store,
        &ImportOptions::new(
            &source,
            rev,
            init.admin.id(),
            &init.server.signing_key,
            &aver::env::DockerResolver,
        ),
    )?;
    let me = init.admin.id();
    let deps = deps_files.is_some();
    let opts = |n: usize| MaterializeOptions {
        repo: Some(repo.to_owned()),
        label: Some(format!("w{n}")),
        deps,
        key: None,
        ..Default::default()
    };

    let free_before = free_bytes(&home_dir)?;
    let mut creates = Vec::with_capacity(count);
    let mut dirs = Vec::with_capacity(count);
    let mut ids = Vec::with_capacity(count);
    for n in 0..count {
        let t = Instant::now();
        let open = ws.open(&me, &import.snapshot)?;
        let m = ws.materialize(&open.id, &opts(n))?;
        creates.push(t.elapsed());
        dirs.push(m.dir.clone());
        ids.push(open.id);
    }
    let free_after = free_bytes(&home_dir)?;
    let strategy = ws.disk().strategy()?;

    let first = &ids[0];
    let mut no_change = Vec::with_capacity(SYNC_RUNS);
    for _ in 0..SYNC_RUNS {
        let t = Instant::now();
        let report = ws.sync(&me, first)?.context("disk mode")?;
        no_change.push(t.elapsed());
        if report.updated != 0 {
            bail!("a no-change sync changed {} rows", report.updated);
        }
    }
    let files = list_source_files(&dirs[0], EDITED_FILES);
    let mut edits = Vec::new();
    for round in 0..5 {
        for f in &files {
            let mut text = fs::read_to_string(f)?;
            text.push_str(&format!("// edit {round}\n"));
            fs::write(f, text)?;
        }
        let t = Instant::now();
        let report = ws.sync(&me, first)?.context("disk mode")?;
        edits.push(t.elapsed());
        if report.updated as usize != files.len() && round > 0 {
            bail!("a {}-file edit synced {} rows", files.len(), report.updated);
        }
    }
    let t = Instant::now();
    ws.snapshot(&me, first)?;
    let snapshot_after_sync = t.elapsed();

    let worktree = worktree_compare(repo, rev, scratch, 3)?;
    let disk_per_ws = (free_before.saturating_sub(free_after)) as f64 / count as f64;
    Ok(json!({
        "name": name,
        "repo": repo.file_name().map(|n| n.to_string_lossy().into_owned()),
        "files": import.files,
        "bytes": import.bytes,
        "deps_files": deps_files,
        "clone": strategy.as_str(),
        "import_ms": ms(import.duration),
        "create_first_ms": ms(creates[0]),
        "create_warm": stats(&creates[1..]),
        "sync_no_change": stats(&no_change),
        "sync_10_files": stats(&edits),
        "snapshot_after_sync_ms": ms(snapshot_after_sync),
        "disk_per_workspace_bytes": disk_per_ws,
        "worktree": worktree,
    }))
}

/// `git worktree add --detach` timings and free-space delta, for comparison
/// (no dependency install).
fn worktree_compare(repo: &FsPath, rev: &str, scratch: &FsPath, n: usize) -> anyhow::Result<Value> {
    let base = scratch.join(format!("worktrees-{}", ulid::Ulid::generate()));
    fs::create_dir_all(&base)?;
    let before = free_bytes(&base)?;
    let mut times = Vec::new();
    for i in 0..n {
        let dir = base.join(format!("wt{i}"));
        let t = Instant::now();
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&dir)
            .arg(rev)
            .status()?;
        if !status.success() {
            bail!("git worktree add failed");
        }
        times.push(t.elapsed());
    }
    let after = free_bytes(&base)?;
    for i in 0..n {
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "remove", "--force"])
            .arg(base.join(format!("wt{i}")))
            .status();
    }
    Ok(json!({
        "create": stats(&times),
        "disk_per_worktree_bytes": before.saturating_sub(after) as f64 / n as f64,
    }))
}

fn list_source_files(dir: &FsPath, n: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let path = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            } else if name.ends_with(".ts") && out.len() < n {
                out.push(path);
            }
        }
        if out.len() >= n {
            break;
        }
    }
    out
}

/// Writes and commits a 16,000-file TypeScript repository with an npm
/// lockfile, and a matching 20,000-file `node_modules` (untracked) with the
/// hidden lockfile npm writes, so materialize adopts it. Returns the
/// dependency file count.
fn synthetic_repo(repo: &FsPath) -> anyhow::Result<u64> {
    fs::create_dir_all(repo)?;
    for d in 0..SYNTHETIC_DIRS {
        let dir = repo.join(format!("src/mod{d:03}"));
        fs::create_dir_all(&dir)?;
        for f in 0..SYNTHETIC_FILES_PER_DIR {
            let text = format!(
                "import {{ helper }} from '../shared';\n\nexport function f{d}_{f}(x: number): number {{\n  const y = helper(x) + {f};\n  return y * {d};\n}}\n"
            );
            fs::write(dir.join(format!("file{f:03}.ts")), text)?;
        }
    }
    let mut packages = serde_json::Map::new();
    packages.insert("".into(), json!({"name": "synthetic", "version": "1.0.0"}));
    let nm = repo.join("node_modules");
    let mut dep_files = 0;
    for p in 0..DEP_PACKAGES {
        let name = format!("pkg{p:03}");
        let pdir = nm.join(&name);
        fs::create_dir_all(pdir.join("lib"))?;
        fs::write(
            pdir.join("package.json"),
            format!("{{\"name\":\"{name}\",\"version\":\"1.0.0\"}}"),
        )?;
        dep_files += 1;
        for f in 1..DEP_FILES_PER_PACKAGE {
            fs::write(
                pdir.join(format!("lib/m{f:02}.js")),
                format!("module.exports = {f};\n"),
            )?;
            dep_files += 1;
        }
        packages.insert(
            format!("node_modules/{name}"),
            json!({"version": "1.0.0", "resolved": format!("https://registry.example/{name}-1.0.0.tgz"), "integrity": format!("sha512-{name}")}),
        );
    }
    let lock = json!({"name": "synthetic", "version": "1.0.0", "lockfileVersion": 3, "requires": true, "packages": packages});
    fs::write(
        repo.join("package-lock.json"),
        serde_json::to_string_pretty(&lock)?,
    )?;
    let mut hidden = lock.clone();
    hidden["packages"]
        .as_object_mut()
        .expect("object")
        .remove("");
    fs::write(
        nm.join(".package-lock.json"),
        serde_json::to_string(&hidden)?,
    )?;
    fs::write(
        repo.join("package.json"),
        "{\"name\":\"synthetic\",\"version\":\"1.0.0\"}\n",
    )?;
    fs::write(repo.join(".gitignore"), "node_modules/\n")?;
    fs::write(
        repo.join("src/shared.ts"),
        "export const helper = (x: number) => x + 1;\n",
    )?;
    let git = |args: &[&str]| -> anyhow::Result<()> {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "bench")
            .env("GIT_AUTHOR_EMAIL", "bench@example.com")
            .env("GIT_COMMITTER_NAME", "bench")
            .env("GIT_COMMITTER_EMAIL", "bench@example.com")
            .status()?;
        if !status.success() {
            bail!("git {args:?} failed");
        }
        Ok(())
    };
    git(&["init", "--quiet", "--initial-branch=main"])?;
    git(&["add", "-A"])?;
    git(&["commit", "--quiet", "-m", "synthetic"])?;
    Ok(dep_files)
}

fn free_bytes(path: &FsPath) -> anyhow::Result<u64> {
    let c = std::ffi::CString::new(path.to_string_lossy().as_bytes())?;
    // SAFETY: a zeroed statfs is a valid out-parameter for the call.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid path and `st` a valid out-parameter.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        bail!("statfs: {}", std::io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)]
    Ok(st.f_bavail as u64 * st.f_bsize as u64)
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

fn stats(samples: &[Duration]) -> Value {
    let mut v: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    v.sort_by(f64::total_cmp);
    let pct = |p: f64| {
        if v.is_empty() {
            return 0.0;
        }
        let i = ((v.len() as f64 - 1.0) * p).round() as usize;
        (v[i] * 1000.0).round() / 1000.0
    };
    json!({ "n": v.len(), "p50_ms": pct(0.5), "p95_ms": pct(0.95), "max_ms": pct(1.0) })
}

fn machine() -> Value {
    let sysctl = |key: &str| {
        Command::new("sysctl")
            .args(["-n", key])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    let mut load = [0f64; 3];
    // SAFETY: `load` has room for the three averages requested.
    let got = unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    json!({
        "load_average": (got == 3).then_some(load),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "model": sysctl("hw.model"),
        "cpu": sysctl("machdep.cpu.brand_string"),
        "cores": std::thread::available_parallelism().map(|n| n.get()).ok(),
    })
}
