//! `aver doctor` against stub `git`, `node` and `claude`
//! commands on a `PATH` of their own: what it reports, the fix for each
//! problem, and its exit code. No Node, no Claude.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Machine {
    dir: tempfile::TempDir,
}

impl Machine {
    fn new() -> Self {
        let m = Machine {
            dir: tempfile::tempdir().unwrap(),
        };
        for d in ["bin", "home", "aver-home", "apps", "cwd"] {
            std::fs::create_dir_all(m.path(d)).unwrap();
        }
        m
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// A command on the machine's `PATH` that runs `script` (sh).
    fn stub(&self, name: &str, script: &str) -> &Self {
        let path = self.path("bin").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        self
    }

    fn doctor(&self) -> (Output, String) {
        self.doctor_with(&["--verbose"])
    }

    fn doctor_with(&self, args: &[&str]) -> (Output, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_aver"))
            .arg("doctor")
            .args(args)
            .current_dir(self.path("cwd"))
            .env("PATH", self.path("bin"))
            .env("HOME", self.path("home"))
            .env("AVER_HOME", self.path("aver-home"))
            .env("AVER_APPS_DIR", self.path("apps"))
            .env_remove("AVER_URL")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        (out, text)
    }
}

const GIT: &str = r#"[ "$1" = --version ] && { echo "git version 2.45.0"; exit 0; }; exit 128"#;
const NODE: &str = r#"echo v22.11.0"#;

/// How many required checks fail besides those a test sets up to fail: the
/// disk check measures the real free space under the temporary home, which
/// a small tmpfs (a sandbox's) fails.
fn machine_fails(text: &str) -> usize {
    usize::from(line(text, "disk").starts_with("FAIL"))
}

fn exit_for(failed: usize) -> Option<i32> {
    Some(if failed == 0 { 0 } else { 1 })
}

fn line<'a>(text: &'a str, name: &str) -> &'a str {
    text.lines()
        .find(|l| l.get(6..).is_some_and(|rest| rest.starts_with(name)))
        .unwrap_or_else(|| panic!("no {name} line in\n{text}"))
}

#[test]
fn everything_required_present_exits_zero() {
    let m = Machine::new();
    m.stub("git", GIT).stub("node", NODE).stub(
        "claude",
        r#"[ "$1 $2" = "auth status" ] && echo '{"loggedIn":true,"authMethod":"claude.ai"}'"#,
    );
    let (out, text) = m.doctor();
    assert_eq!(out.status.code(), exit_for(machine_fails(&text)), "{text}");
    assert!(line(&text, "git").starts_with("ok"), "{text}");
    assert!(line(&text, "git").contains("2.45.0"), "{text}");
    assert!(
        !text
            .lines()
            .any(|l| l.get(6..).is_some_and(|r| r.starts_with("docker"))),
        "Docker is no longer a doctor line: {text}"
    );

    assert!(line(&text, "agents").contains("Claude Code"), "{text}");
    assert!(line(&text, "claude login").contains("signed in"), "{text}");
    assert!(
        !text.lines().any(|l| l.contains("daemon")),
        "no daemon line: {text}"
    );
}

#[test]
fn missing_tools_are_named_with_where_to_get_them() {
    let m = Machine::new();
    m.stub("node", "echo v18.20.4").stub(
        "claude",
        r#"[ "$1 $2" = "auth status" ] && echo '{"loggedIn":false}'"#,
    );
    let (out, text) = m.doctor();
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(line(&text, "git").starts_with("FAIL"), "{text}");
    assert!(text.contains("https://git-scm.com/downloads"), "{text}");
    assert!(
        !text.contains("v18.20.4"),
        "Node is no longer a doctor line\n{text}"
    );
    assert!(line(&text, "claude login").starts_with("warn"), "{text}");
    assert!(text.contains("claude auth login"), "{text}");
}

#[test]
fn a_repository_without_a_lockfile_is_a_warning() {
    let m = Machine::new();
    let repo = m.path("cwd");
    m.stub("node", NODE);
    // A real git, for rev-parse.
    let git = which_git();
    std::os::unix::fs::symlink(&git, m.path("bin/git")).unwrap();
    let ok = Command::new(&git)
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(ok.success());
    std::fs::write(repo.join("package.json"), "{}").unwrap();
    let (out, text) = m.doctor();
    assert_eq!(out.status.code(), exit_for(machine_fails(&text)), "{text}");
    let r = line(&text, "repository");
    assert!(
        r.starts_with("warn") && r.contains("without a lockfile"),
        "{text}"
    );
    std::fs::write(repo.join("package-lock.json"), "{}").unwrap();
    let (_, text) = m.doctor();
    assert!(line(&text, "repository").starts_with("ok"), "{text}");
}

/// The build artifacts of the home's workspace directories are counted
/// and the largest named (an informational line, printed with
/// `--verbose`).
#[test]
fn workspace_build_artifacts_are_reported() {
    let m = Machine::new();
    m.stub("git", GIT).stub("node", NODE);
    let (_, text) = m.doctor();
    assert!(
        !text
            .lines()
            .any(|l| l.get(6..).is_some_and(|r| r.starts_with("workspaces"))),
        "no workspace directories, no line: {text}"
    );
    let ws = m.path("aver-home/work/repo/ws-1");
    let target = ws.join("target");
    std::fs::create_dir_all(target.join("debug/.fingerprint")).unwrap();
    std::fs::write(target.join("CACHEDIR.TAG"), "Signature: cargo\n").unwrap();
    std::fs::write(target.join("debug/libbig.rlib"), vec![0u8; 3 << 20]).unwrap();
    std::fs::create_dir_all(m.path("aver-home/work/repo/ws-2/src")).unwrap();
    let (_, text) = m.doctor();
    let w = line(&text, "workspaces");
    assert!(
        w.starts_with("ok")
            && w.contains("2 workspace directories, 0.0 GiB of build artifacts (largest 0.0 GiB:")
            && w.contains("ws-1"),
        "{text}"
    );
    // The default output folds the workspaces summary into the line of
    // what passed.
    let (_, short) = m.doctor_with(&[]);
    let first = short.lines().next().unwrap();
    assert!(
        first.starts_with("ok    ") && first.contains("workspaces"),
        "{short}"
    );
    assert!(!short.contains("2 workspace directories"), "{short}");
}

/// Slot sizes are what removing each slot frees: on APFS a slot cloned
/// from where its dependencies were installed counts nothing for the
/// blocks it shares with that source, and a sparse file counts no length.
#[test]
fn slot_sizes_count_what_removing_them_frees() {
    let m = Machine::new();
    m.stub("git", GIT).stub("node", NODE);
    let source = m.path("checkout");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("big"), vec![7u8; 160 << 20]).unwrap();
    let sides = m.path("aver-home/sides/default");
    std::fs::create_dir_all(sides.join("B-own")).unwrap();
    std::fs::write(sides.join("B-own/big"), vec![7u8; 160 << 20]).unwrap();
    std::fs::File::create(sides.join("B-own/sparse"))
        .unwrap()
        .set_len(4 << 30)
        .unwrap();
    let strategy = aver::disk::clone::probe(&m.path("probe"), m.dir.path()).unwrap();
    aver::disk::clone::clone_tree(strategy, &source, &sides.join("J-clone")).unwrap();
    let (_, text) = m.doctor();
    let slots = line(&text, "slots");
    assert!(slots.contains("B-own 0.2 GiB"), "{slots}");
    if cfg!(target_os = "macos") && strategy != aver::disk::CloneStrategy::Copy {
        assert!(
            slots.contains("0.2 GiB that removing each alone frees")
                && slots.contains("J-clone 0.0 GiB"),
            "{slots}"
        );
    } else {
        assert!(slots.contains("J-clone 0.2 GiB"), "{slots}");
    }
}

/// By default doctor prints one line for what passed, then only the
/// problems with their fixes, and the next command.
#[test]
fn the_default_output_is_short_and_names_the_problems() {
    let m = Machine::new();
    m.stub("git", GIT).stub("node", NODE);
    let (_, text) = m.doctor_with(&[]);
    let first = text.lines().next().unwrap();
    assert!(
        first.starts_with("ok    ") && first.contains("git"),
        "{text}"
    );
    assert!(
        !text.contains("2.45.0"),
        "passed checks have no line: {text}"
    );
    assert!(line(&text, "agents").starts_with("warn"), "{text}");
    assert!(text.contains("fix: install one"), "{text}");
    assert!(
        text.lines()
            .last()
            .unwrap()
            .starts_with("Next: aver workspace new"),
        "{text}"
    );
    assert!(text.lines().count() <= 10, "{text}");
}

/// Signed out, `claude auth status` exits 1 and still prints its JSON:
/// doctor says to log in, not to update Claude Code.
#[test]
fn a_signed_out_claude_is_told_to_log_in() {
    let m = Machine::new();
    m.stub("git", GIT).stub("node", NODE).stub(
        "claude",
        r#"[ "$1 $2" = "auth status" ] && { echo '{"loggedIn":false}'; exit 1; }"#,
    );
    let (_, text) = m.doctor();
    let login = line(&text, "claude login");
    assert!(
        login.starts_with("warn") && login.contains("not signed in"),
        "{text}"
    );
    assert!(text.contains("claude auth login"), "{text}");
    assert!(!text.contains("claude update"), "{text}");
}

/// A Python repository's line names its check, and a machine with
/// neither python nor python3 is told `aver check` cannot run it.
#[test]
fn a_python_repository_names_its_check_and_the_missing_interpreter() {
    let m = Machine::new();
    m.stub("node", NODE);
    std::os::unix::fs::symlink(which_git(), m.path("bin/git")).unwrap();
    let repo = m.path("cwd");
    assert!(Command::new(which_git())
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    std::fs::write(repo.join("pyproject.toml"), "[project]\nname = \"x\"\n").unwrap();
    let (_, text) = m.doctor();
    let r = line(&text, "repository");
    assert!(
        r.contains("Python") && r.contains("python -m pytest"),
        "{text}"
    );
    let py = line(&text, "python");
    assert!(
        py.starts_with("warn") && py.contains("neither python nor python3"),
        "{text}"
    );
}

/// A Go older than go.mod's `go` line under `GOTOOLCHAIN=local` and a Rust
/// older than Cargo.toml's `rust-version` run no test natively, so doctor
/// fails each with the fix instead of saying everything is fine. Under
/// `auto` Go fetches the version asked for, so that Go is no problem.
#[test]
fn a_toolchain_older_than_the_project_asks_for_fails_crit_ci_trust_doctor() {
    let m = Machine::new();
    let mode = m.path("gotoolchain");
    std::fs::write(&mode, "local\n").unwrap();
    m.stub("node", NODE)
        .stub(
            "go",
            &format!(
                r#"[ "$1" = env ] && read -r mode < '{}' && echo "$mode" && exit 0
[ "$1" = version ] && echo "go version go1.21.5 linux/amd64""#,
                mode.display()
            ),
        )
        .stub(
            "rustc",
            r#"[ "$1" = --version ] && echo "rustc 1.97.0 (c980f4866 2026-06-30)""#,
        );
    std::os::unix::fs::symlink(which_git(), m.path("bin/git")).unwrap();
    let repo = m.path("cwd");
    assert!(Command::new(which_git())
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    std::fs::write(repo.join("go.mod"), "module example.com/calc\n\ngo 1.22\n").unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"calc\"\nversion = \"0.1.0\"\nrust-version = \"1.99\"\n",
    )
    .unwrap();
    let (out, text) = m.doctor_with(&[]);
    assert_eq!(out.status.code(), Some(1), "{text}");
    let problems: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("FAIL  toolchain"))
        .collect();
    assert_eq!(problems.len(), 2, "{text}");
    assert!(
        problems[0].contains("go.mod: go 1.22") && problems[0].contains("go 1.21.5"),
        "{text}"
    );
    assert!(
        problems[1].contains("rust-version = \"1.99\"") && problems[1].contains("rustc 1.97.0"),
        "{text}"
    );
    assert!(
        text.contains("fix: install the Go version go.mod asks for"),
        "{text}"
    );

    std::fs::write(&mode, "auto\n").unwrap();
    let (_, text) = m.doctor_with(&[]);
    let problems: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("FAIL  toolchain"))
        .collect();
    assert_eq!(problems.len(), 1, "{text}");
    assert!(problems[0].contains("rust-version"), "{text}");

    std::fs::write(repo.join("go.mod"), "module example.com/calc\n\ngo 1.21\n").unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"calc\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (_, text) = m.doctor_with(&[]);
    assert!(!text.contains("  toolchain"), "{text}");
}

fn which_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| Path::new(p).is_file())
        .expect("git on PATH")
}
