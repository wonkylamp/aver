//! `aver setup`'s output: short by default (one line per file it would
//! write, then the question), each file in full with `--verbose`, and a
//! "no" piped to the question is a clean exit that writes nothing.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = \"x\"\n").unwrap();
        git(&repo, &["init", "-q"]);
        Self { dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn setup(&self, args: &[&str], stdin: Option<&str>) -> (Output, String) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_aver"))
            .arg("setup")
            .args(["--tool", "cursor", "--tool", "agents-md"])
            .args(args)
            .current_dir(self.repo())
            .env("AVER_HOME", self.dir.path().join("aver-home"))
            .env("HOME", self.dir.path().join("home"))
            .env("AVER_APPS_DIR", self.dir.path().join("apps"))
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(text) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        (out, text)
    }
}

#[test]
fn setup_lists_paths_by_default_and_contents_with_verbose() {
    let r = Repo::new();
    let (out, short) = r.setup(&[], Some("n\n"));
    assert_eq!(out.status.code(), Some(0), "declining is no error: {short}");
    assert!(short.contains("Nothing written."), "{short}");
    assert!(
        short.contains("--- .cursor/rules/aver.mdc (new file)"),
        "{short}"
    );
    assert!(short.contains("--- AGENTS.md (new file)"), "{short}");
    assert!(
        short.contains("--verbose shows each file in full"),
        "{short}"
    );
    assert!(
        !short.contains("### The loop"),
        "the instructions are not printed: {short}"
    );
    assert!(short.lines().count() < 25, "{short}");
    assert!(!r.repo().join("AGENTS.md").exists(), "nothing written");

    let (_, long) = r.setup(&["--verbose"], Some("n\n"));
    assert!(long.contains("### The loop"), "{long}");
    assert!(long.lines().count() > short.lines().count() * 3, "{long}");

    // No answer at all (an empty pipe) still asks for --yes.
    let (out, _) = r.setup(&[], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("pass --yes"));

    let (out, done) = r.setup(&["--yes"], None);
    assert!(out.status.success(), "{done}");
    assert!(r.repo().join("AGENTS.md").is_file());
    assert!(
        done.lines()
            .last()
            .unwrap()
            .contains("aver workspace new -c"),
        "{done}"
    );
}
