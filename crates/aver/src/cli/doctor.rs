//! `aver doctor`: checks what Aver needs on this machine and in this
//! repository, and says how to fix each problem. Reads only: it never
//! installs, starts or stops anything. Exit 0 when everything required is
//! there, 1 when something required is missing.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use aver::keys::Home;

use super::locate;

#[derive(clap::Args)]
pub struct Args {
    /// Print every check, also those that passed, instead of a summary and
    /// the problems.
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    /// Works without it, or works worse.
    Warn,
    /// Aver cannot work without it.
    Fail,
}

struct Check {
    name: &'static str,
    level: Level,
    detail: String,
    fix: Option<String>,
}

fn ok(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        level: Level::Ok,
        detail: detail.into(),
        fix: None,
    }
}

fn problem(
    name: &'static str,
    level: Level,
    detail: impl Into<String>,
    fix: impl Into<String>,
) -> Check {
    Check {
        name,
        level,
        detail: detail.into(),
        fix: Some(fix.into()),
    }
}

/// Runs `program args` with no stdin, at most `limit`; its stdout when it
/// exits 0.
fn output(program: &Path, args: &[&str], limit: Duration) -> Result<String, String> {
    match captured(program, args, limit)? {
        (true, out, _) => Ok(out),
        (false, _, err) => Err(err),
    }
}

/// Runs `program args` with no stdin, at most `limit`: whether it exited 0,
/// its stdout, and the first line of its stderr (or "failed"). Some
/// commands answer on stdout with a non-zero exit (`claude auth status`
/// when signed out).
fn captured(
    program: &Path,
    args: &[&str],
    limit: Duration,
) -> Result<(bool, String, String), String> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let first = err
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
                .trim()
                .to_owned();
            Ok((
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                first,
            ))
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => {
            // SAFETY: kill(2) on the child we spawned.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            Err(format!("no answer within {} s", limit.as_secs()))
        }
    }
}

const SHORT: Duration = Duration::from_secs(10);

fn platform() -> Check {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    match os {
        "macos" => ok("system", format!("macOS {arch}")),
        "linux" => {
            let version = std::fs::read_to_string("/proc/version").unwrap_or_default();
            if version.contains("microsoft-standard-WSL2") || version.contains("WSL2") {
                ok("system", format!("Linux {arch} (WSL2)"))
            } else if version.contains("Microsoft") {
                problem(
                    "system",
                    Level::Fail,
                    format!("Linux {arch} under WSL 1, which Aver does not support"),
                    "convert the distribution to WSL 2: wsl --set-version <distro> 2 (docs/WINDOWS.md)",
                )
            } else {
                ok("system", format!("Linux {arch}"))
            }
        }
        _ => problem(
            "system",
            Level::Fail,
            format!("{os} {arch} is not supported"),
            "use macOS or Linux; on Windows, run Aver inside WSL 2 (https://github.com/wonkylamp/aver/blob/main/docs/WINDOWS.md)",
        ),
    }
}

fn git() -> Check {
    match locate::which("git") {
        None => problem(
            "git",
            Level::Fail,
            "not found on PATH",
            "install Git: https://git-scm.com/downloads (macOS: xcode-select --install)",
        ),
        Some(path) => match output(&path, &["--version"], SHORT) {
            Ok(v) => ok(
                "git",
                v.trim().trim_start_matches("git version ").to_owned(),
            ),
            Err(e) => problem(
                "git",
                Level::Fail,
                format!("{} fails: {e}", path.display()),
                "reinstall Git: https://git-scm.com/downloads",
            ),
        },
    }
}

fn existing(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf)
}

// The statvfs field widths differ by platform (u32 blocks on macOS).
#[allow(clippy::unnecessary_cast)]
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: statvfs fills `st` for a NUL-terminated path.
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0)
        .then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

fn disk(home: &Home) -> Check {
    const GIB: u64 = 1 << 30;
    let Some(dir) = existing(home.root()) else {
        return problem(
            "disk",
            Level::Warn,
            "cannot tell",
            "check the free space under $AVER_HOME",
        );
    };
    match free_bytes(&dir) {
        None => problem(
            "disk",
            Level::Warn,
            format!("cannot read the free space of {}", dir.display()),
            "check it with df -h",
        ),
        Some(free) => {
            let detail = format!(
                "{:.1} GiB free under {}",
                free as f64 / GIB as f64,
                dir.display()
            );
            if free < 2 * GIB {
                problem("disk", Level::Fail, detail, "free at least 2 GiB: workspaces and slots live there (aver gc removes idle caches)")
            } else if free < 10 * GIB {
                problem(
                    "disk",
                    Level::Warn,
                    detail,
                    "10 GiB or more is comfortable; aver gc removes idle caches",
                )
            } else {
                ok("disk", detail)
            }
        }
    }
}

/// The build artifacts in the home's workspaces: each workspace
/// directory's Cargo `target` directories and its `aver check` copies, by
/// apparent size (blocks a copy-on-write clone shares count once per
/// clone). A warning when they are large or the disk is low.
fn workspaces(home: &Home) -> Option<Check> {
    const GIB: u64 = 1 << 30;
    let root = home.root();
    let mut sizes: Vec<(u64, PathBuf)> = Vec::new();
    for repo in std::fs::read_dir(root.join("work")).ok()?.flatten() {
        for dir in std::fs::read_dir(repo.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let mut bytes = 0;
            for entry in std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                if aver::disk::is_target(&entry.path()) {
                    bytes += aver::disk::apparent_bytes(&entry.path());
                }
            }
            let ws = dir.path().join(".aver/workspace.toml");
            let id = std::fs::read_to_string(&ws).ok().and_then(|text| {
                let table: toml::Table = text.parse().ok()?;
                table.get("workspace")?.as_str().map(str::to_owned)
            });
            if let Some(id) = id {
                for kind in ["check", "judged", "combined"] {
                    bytes += aver::disk::apparent_bytes(&root.join(kind).join(&id));
                }
            }
            sizes.push((bytes, dir.path()));
        }
    }
    if sizes.is_empty() {
        return None;
    }
    sizes.sort_by(|a, b| b.cmp(a));
    let total: u64 = sizes.iter().map(|(b, _)| b).sum();
    let gib = |b: u64| b as f64 / GIB as f64;
    let mut detail = format!(
        "{} workspace director{}, {:.1} GiB of build artifacts",
        sizes.len(),
        if sizes.len() == 1 { "y" } else { "ies" },
        gib(total)
    );
    if sizes[0].0 > 0 {
        detail.push_str(&format!(
            " (largest {:.1} GiB: {})",
            gib(sizes[0].0),
            sizes[0].1.display()
        ));
    }
    let low = existing(root)
        .and_then(|d| free_bytes(&d))
        .is_some_and(|free| free < 10 * GIB);
    Some(if total > 20 * GIB || (low && total > GIB) {
        problem(
            "workspaces",
            Level::Warn,
            detail,
            "aver gc --builds reclaims the build artifacts of workspaces idle for 10 minutes (aver gc does it after a day); aver workspace rm <dir> removes one",
        )
    } else {
        ok("workspaces", detail)
    })
}

/// The slots (ARCHITECTURE.md §1.8.5) by real disk use (what removing each
/// alone frees, [`aver::disk::usage::real_bytes`]; blocks a transplant
/// slot still shares with its base count in neither), the largest first: a base slot per base tree, a judged and a transplant slot per
/// workspace that verified, each a copy-on-write clone of where its
/// dependencies were installed. A warning when they are large or the disk
/// is low.
fn slots(home: &Home) -> Option<Check> {
    const GIB: u64 = 1 << 30;
    const SHOWN: usize = 4;
    let root = home.root().join("sides");
    let mut sizes: Vec<(u64, String)> = Vec::new();
    for repo in std::fs::read_dir(&root).ok()?.flatten() {
        let name = repo.file_name().to_string_lossy().into_owned();
        if name != "default" && !(name.len() == 16 && name.bytes().all(|b| b.is_ascii_hexdigit())) {
            continue;
        }
        for pool in std::fs::read_dir(repo.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let slot = pool.file_name().to_string_lossy().into_owned();
            if pool.file_type().is_ok_and(|t| t.is_dir()) && !slot.contains('.') {
                let bytes = aver::disk::usage::real_bytes(&pool.path());
                sizes.push((
                    bytes,
                    format!("{name}/{}", pool.file_name().to_string_lossy()),
                ));
            }
        }
    }
    if sizes.is_empty() {
        return None;
    }
    sizes.sort_by(|a, b| b.cmp(a));
    let total: u64 = sizes.iter().map(|(b, _)| b).sum();
    let gib = |bytes: u64| format!("{:.1} GiB", bytes as f64 / GIB as f64);
    let mut detail = format!(
        "{} slot{} under {}, {} that removing each alone frees",
        sizes.len(),
        if sizes.len() == 1 { "" } else { "s" },
        root.display(),
        gib(total),
    );
    let shown: Vec<String> = sizes
        .iter()
        .take(SHOWN)
        .map(|(b, name)| format!("{name} {}", gib(*b)))
        .collect();
    detail.push_str(&format!(" ({}", shown.join(", ")));
    if sizes.len() > SHOWN {
        detail.push_str(&format!(", {} more", sizes.len() - SHOWN));
    }
    detail.push(')');
    let low = existing(home.root())
        .and_then(|d| free_bytes(&d))
        .is_some_and(|free| free < 10 * GIB);
    Some(if total > SLOTS_WARN_BYTES || (low && total > GIB) {
        problem(
            "slots",
            Level::Warn,
            detail,
            "aver gc removes the slots of released workspaces and idle base slots",
        )
    } else {
        ok("slots", detail)
    })
}

/// Slots larger than this together get a warning.
const SLOTS_WARN_BYTES: u64 = 30 << 30;

/// The agent CLIs found, and whether Claude Code is signed in.
fn agents() -> Vec<Check> {
    let found = super::setup::detect_in(&std::env::var_os("PATH").unwrap_or_default());
    let mut out = Vec::new();
    if found.is_empty() {
        out.push(problem(
            "agents",
            Level::Warn,
            "no agent tool found (claude, cursor, codex, gemini, code)",
            "install one, e.g. Claude Code: https://code.claude.com/docs/en/setup; aver run needs claude or codex",
        ));
        return out;
    }
    let names: Vec<String> = found.iter().map(|f| f.label.to_owned()).collect();
    out.push(ok("agents", names.join(", ")));
    if let Some(claude) = locate::which("claude") {
        // `claude auth status` reports the sign-in without printing a
        // credential; only its loggedIn flag is read. Signed out, it exits
        // 1 and still prints the JSON.
        let status = captured(
            &claude,
            &["auth", "status", "--json"],
            Duration::from_secs(15),
        )
        .and_then(|(ok, text, err)| {
            if ok || text.trim_start().starts_with('{') {
                Ok(text)
            } else {
                Err(err)
            }
        });
        out.push(match status {
            Ok(text) => match serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v["loggedIn"].as_bool())
            {
                Some(true) => ok("claude login", "signed in"),
                Some(false) => problem(
                    "claude login",
                    Level::Warn,
                    "not signed in",
                    "run `claude auth login` (or `claude` and /login)",
                ),
                None => problem(
                    "claude login",
                    Level::Warn,
                    "claude auth status gave no answer Aver can read",
                    "run `claude auth status` to check",
                ),
            },
            Err(e) => problem(
                "claude login",
                Level::Warn,
                format!("claude auth status failed ({e})"),
                "update Claude Code (claude update), then run `claude auth status`",
            ),
        });
    }
    out
}

/// macOS: whether apps started from the Dock or Finder find `aver`.
fn gui_path() -> Option<Check> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let launchd = output(Path::new("/bin/launchctl"), &["getenv", "PATH"], SHORT)
        .ok()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| locate::GUI_PATH.to_owned());
    Some(
        match locate::which_in("aver", std::ffi::OsStr::new(&launchd)) {
            Some(p) => ok("gui path", format!("apps find aver at {}", p.display())),
            None => {
                let exe = std::env::current_exe()
                    .map(|e| e.display().to_string())
                    .unwrap_or_else(|_| "aver".into());
                // Nothing Aver writes is affected, so this is no warning.
                ok(
                    "gui path",
                    format!("apps started from the Dock get a PATH without aver; the plugin and `aver setup` find it themselves, and a hook you write by hand needs the full path {exe}"),
                )
            }
        },
    )
}

/// The check generic mode (or the default environment) runs for `family`.
fn check_of(family: aver::env::Family) -> &'static str {
    use aver::env::Family;
    match family {
        Family::Node => "npm test",
        Family::Python => "python -m pytest",
        Family::Rust => "cargo test --workspace",
        Family::Go => "go test ./...",
    }
}

/// Whether `aver check`, which runs the tests natively, finds each
/// ecosystem's tool here; `aver verify` runs them natively too.
fn toolchains() -> Vec<Check> {
    use aver::env::Family;
    let Some(root) = std::env::current_dir()
        .ok()
        .and_then(|cwd| super::integrate::repo_root(&cwd).ok())
    else {
        return Vec::new();
    };
    let later = "aver check and aver verify run the tests natively and cannot run them here";
    let mut out = Vec::new();
    for family in aver::languages::families_in(&root) {
        let (tool, fix) = match family {
            Family::Node => ("npm", "install Node.js with npm: https://nodejs.org"),
            Family::Rust => ("cargo", "install Rust: https://rustup.rs"),
            Family::Go => ("go", "install Go: https://go.dev/dl/"),
            Family::Python => {
                let python = locate::which("python").or_else(|| locate::which("python3"));
                out.push(match python {
                    None => problem(
                        "python",
                        Level::Warn,
                        format!("neither python nor python3 is on PATH; {later}"),
                        "install Python 3 (macOS: xcode-select --install, or brew install python)",
                    ),
                    Some(py) => {
                        let wants = ["pyproject.toml", "requirements.txt", "requirements-dev.txt"]
                            .iter()
                            .any(|f| {
                                std::fs::read_to_string(root.join(f))
                                    .is_ok_and(|t| t.contains("pytest"))
                            });
                        let has = !wants
                            || output(&py, &["-c", "import pytest"], SHORT).is_ok();
                        if has {
                            ok("python", format!("{} (aver check runs the tests with it)", py.display()))
                        } else {
                            problem(
                                "python",
                                Level::Warn,
                                format!("{} has no pytest, which the project's tests use; {later}", py.display()),
                                format!("{} -m pip install pytest (or install the project's test dependencies)", py.display()),
                            )
                        }
                    }
                });
                continue;
            }
        };
        out.push(match locate::which(tool) {
            Some(p) => ok(
                tool,
                format!(
                    "{tool} at {} (aver check runs the tests with it)",
                    p.display()
                ),
            ),
            None => problem(
                tool,
                Level::Warn,
                format!("{tool} is not on PATH; {later}"),
                fix,
            ),
        });
    }
    out
}

/// Whether the Go and Rust here are new enough for the repository's root
/// `go.mod` (`go` and `toolchain` lines) and `Cargo.toml` (`rust-version`):
/// a native verify runs them as they are (`GOTOOLCHAIN=local`), so a
/// toolchain older than the project asks for runs no test at all.
fn project_toolchains() -> Vec<Check> {
    let Some(root) = std::env::current_dir()
        .ok()
        .and_then(|cwd| super::integrate::repo_root(&cwd).ok())
    else {
        return Vec::new();
    };
    let read = |f: &str| std::fs::read_to_string(root.join(f)).ok();
    let (go_mod, cargo) = (read("go.mod"), read("Cargo.toml"));
    if go_mod.is_none() && cargo.is_none() {
        return Vec::new();
    }
    // A pinned Rust toolchain is what rustc resolves in the root; without
    // one, a neutral directory keeps a stray override out.
    let pinned =
        root.join("rust-toolchain.toml").is_file() || root.join("rust-toolchain").is_file();
    let cwd = if pinned {
        root.clone()
    } else {
        std::env::temp_dir()
    };
    let (go, rustc) = aver::exec::toolchain::local_versions(&cwd);
    aver::exec::toolchain::mismatches(
        go_mod.as_deref(),
        cargo.as_deref(),
        go.as_deref(),
        rustc.as_deref(),
    )
    .into_iter()
    .map(|m| {
        problem(
            "toolchain",
            if m.fatal { Level::Fail } else { Level::Warn },
            m.to_string(),
            m.fix,
        )
    })
    .collect()
}

/// Whether Aver can verify the repository holding the current directory.
fn repo() -> Option<Check> {
    let Some(root) = std::env::current_dir()
        .ok()
        .and_then(|cwd| super::integrate::repo_root(&cwd).ok())
    else {
        return Some(ok(
            "repository",
            "not in a Git repository; run aver doctor in one to check the project",
        ));
    };
    let has = |f: &str| root.join(f).is_file();
    Some(if has("aver.toml") {
        ok(
            "repository",
            format!("{}: configured by aver.toml", root.display()),
        )
    } else if has("package.json") && has("package-lock.json") {
        ok(
            "repository",
            format!(
                "{}: Node with npm (package-lock.json); the default check is npm test",
                root.display()
            ),
        )
    } else if has("package.json") {
        let other = ["pnpm-lock.yaml", "yarn.lock", "bun.lockb", "bun.lock"]
            .into_iter()
            .find(|f| has(f));
        problem(
            "repository",
            Level::Warn,
            match other {
                Some(lock) => format!("{}: Node with {lock}; Aver installs with npm ci", root.display()),
                None => format!("{}: package.json without a lockfile", root.display()),
            },
            "commit a package-lock.json (npm install), or describe install and checks in aver.toml (docs/CLI.md)",
        )
    } else if let Some(project) = [
        "Cargo.toml",
        "go.mod",
        "pyproject.toml",
        "uv.lock",
        "setup.py",
        "requirements.txt",
    ]
    .into_iter()
    .find(|f| has(f))
    {
        // Generic mode (ARCHITECTURE.md §1.4.1) gives these packages their
        // own checks; a Python project needs tests to get one.
        let families = aver::languages::families_in(&root);
        let checks: Vec<&str> = families.iter().map(|f| check_of(*f)).collect();
        ok(
            "repository",
            format!(
                "{}: {} ({project}); checks: {}",
                root.display(),
                aver::languages::names(&families),
                checks.join(", ")
            ),
        )
    } else {
        let found: Vec<&str> = ["pom.xml", "Gemfile"]
            .into_iter()
            .filter(|f| has(f))
            .collect();
        problem(
            "repository",
            Level::Warn,
            if found.is_empty() {
                format!("{}: no ecosystem Aver detects", root.display())
            } else {
                format!(
                    "{}: {} found; Aver detects Node, Python, Rust and Go",
                    root.display(),
                    found.join(", ")
                )
            },
            "describe the image, install command and checks in aver.toml (docs/CLI.md)",
        )
    })
}

/// Whether the repository's Claude Code hooks are those `aver integrate
/// claude` would write now (ARCHITECTURE.md §2.12). `None` outside a
/// repository or when Aver's hooks are not installed there.
fn integration() -> Option<Check> {
    use aver::integrate::{self, INTEGRATION_REV};
    let root = std::env::current_dir()
        .ok()
        .and_then(|cwd| super::integrate::repo_root(&cwd).ok())?;
    let read = |rel: &str| -> serde_json::Value {
        std::fs::read_to_string(root.join(rel))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    };
    let aver_hooks = |hooks: &serde_json::Value| -> std::collections::BTreeSet<(String, String)> {
        hooks
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(event, groups)| {
                groups
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|g| g["hooks"].as_array())
                    .flatten()
                    .filter_map(|h| h["command"].as_str())
                    .filter(|c| integrate::parse_hook(c).is_some())
                    .map(move |c| (event.clone(), c.to_owned()))
            })
            .collect()
    };
    // Aver 0.4 wrote its hooks into the local settings; finding any there
    // means the integration is older than this aver's.
    let mut installed = aver_hooks(&read(integrate::SETTINGS)["hooks"]);
    installed.extend(aver_hooks(&read(integrate::LOCAL_SETTINGS)["hooks"]));
    if installed.is_empty() {
        return None;
    }
    let wanted = aver_hooks(&integrate::hooks());
    if installed == wanted {
        return Some(ok(
            "integration",
            format!("Claude Code hooks are current (rev {INTEGRATION_REV})"),
        ));
    }
    let revs: std::collections::BTreeSet<String> = installed
        .iter()
        .filter_map(|(_, c)| integrate::parse_hook(c))
        .map(|h| h.rev.map_or_else(|| "none".to_owned(), |r| r.to_string()))
        .collect();
    let revs: Vec<String> = revs.into_iter().collect();
    Some(problem(
        "integration",
        Level::Warn,
        format!(
            "Claude Code hooks in {} or {} differ from what this aver writes (installed rev {}, this aver rev {INTEGRATION_REV})",
            integrate::SETTINGS,
            integrate::LOCAL_SETTINGS,
            revs.join(", ")
        ),
        format!("run aver integrate claude in {}", root.display()),
    ))
}

/// The last unclean stop and what its recovery found.
fn recovery(home: &Home) -> Option<Check> {
    let r = aver::recover::last(home)?;
    if r.interrupted.is_empty() {
        return Some(ok("recovery", r.summary()));
    }
    Some(problem(
        "recovery",
        Level::Warn,
        r.summary(),
        format!(
            "`aver status {}` shows what each run recorded; `aver discard` or `aver rerun` it",
            r.interrupted[0]
        ),
    ))
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<ExitCode> {
    let mut checks = vec![platform(), git()];
    checks.push(disk(home));
    checks.extend(workspaces(home));
    checks.extend(slots(home));
    checks.extend(agents());
    checks.extend(gui_path());
    checks.extend(repo());
    checks.extend(integration());
    checks.extend(toolchains());
    checks.extend(project_toolchains());
    checks.extend(recovery(home));

    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    // By default: one line of what passed, the repository line (what was
    // found), then each problem with its fix; --verbose prints every check.
    if !args.verbose {
        let passed: Vec<&str> = checks
            .iter()
            .filter(|c| c.level == Level::Ok && c.name != "repository")
            .map(|c| c.name)
            .collect();
        if !passed.is_empty() {
            println!(
                "ok    {} (aver doctor --verbose shows each)",
                passed.join(", ")
            );
        }
    }
    for c in &checks {
        if !args.verbose && c.level == Level::Ok && c.name != "repository" {
            continue;
        }
        let mark = match c.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        println!("{mark}  {:<width$}  {}", c.name, c.detail);
        if let Some(fix) = &c.fix {
            println!("      {:<width$}  fix: {fix}", "");
        }
    }
    let failed = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warned = checks.iter().filter(|c| c.level == Level::Warn).count();
    println!();
    if failed > 0 {
        println!("{failed} required check(s) failed. Fix them, then run aver doctor again.");
        return Ok(ExitCode::from(1));
    }
    if warned > 0 {
        println!(
            "Aver can run here; {warned} warning(s) above say what works worse or not at all."
        );
    } else {
        println!("Everything Aver needs is here.");
    }
    println!("Next: aver workspace new -c \"<id>: <what done means>\" makes a workspace directory for one agent; aver run \"<task>\" --agents 2 runs several.");
    Ok(ExitCode::SUCCESS)
}
