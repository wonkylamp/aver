//! `aver workspace rebase` and `aver workspace sync --base`: a workspace
//! moved onto a newer commit of its repository keeps only the agent's own
//! changes (ARCHITECTURE.md §2.3.1). The verify and accept part needs
//! Docker and is `#[ignore]`.

mod common;
mod git_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "node_modules" {
            continue;
        }
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

struct Fx {
    _dir: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("calc");
        copy_dir(&common::fixture("calc"), &repo);
        git_support::git(&repo, &["init", "--quiet", "--initial-branch=main"], b"");
        git(&repo, &["config", "user.name", "Test User"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git_support::git(&repo, &["add", "-A"], b"");
        git_support::git(&repo, &["commit", "--quiet", "-m", "calc"], b"");
        let fx = Fx {
            home: root.join("home"),
            repo,
            _dir: dir,
        };
        fx.ok(&root, &["init"]);
        fx
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_aver"))
            .args(args)
            .current_dir(cwd)
            .env("AVER_HOME", &self.home)
            .output()
            .unwrap()
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run(cwd, args);
        assert!(
            out.status.success(),
            "aver {args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// A workspace directory on the repository's HEAD.
    fn workspace(&self, label: &str) -> PathBuf {
        let out = self.ok(
            &self.repo,
            &["workspace", "new", "--no-deps", "--label", label],
        );
        PathBuf::from(out.trim())
    }

    /// Main moves on: a new file and a changed `add`, committed as B'.
    fn advance_main(&self) {
        fs::write(self.repo.join("src/upstream.js"), "module.exports = 1;\n").unwrap();
        let math = fs::read_to_string(self.repo.join("src/math.js")).unwrap();
        fs::write(
            self.repo.join("src/math.js"),
            math.replace("return a + b;", "return Number(a) + Number(b);"),
        )
        .unwrap();
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "--quiet", "-m", "main moves on"]);
    }

    /// What `git merge main` leaves in `ws`: main's version of its files.
    fn merge_main_into(&self, ws: &Path) {
        for rel in ["src/upstream.js", "src/math.js"] {
            fs::copy(self.repo.join(rel), ws.join(rel)).unwrap();
        }
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

const NEGATE: &str = "module.exports = (x) => -x;\n";

#[test]
fn after_merging_main_a_rebase_leaves_only_the_agents_own_edit() {
    let fx = Fx::new();
    let ws = fx.workspace("mine");
    let other = fx.workspace("other");
    // Another task changes the file main changes too.
    let math = fs::read_to_string(other.join("src/math.js")).unwrap();
    fs::write(
        other.join("src/math.js"),
        math.replace("return a * b;", "return b * a;"),
    )
    .unwrap();
    fx.ok(&other, &["workspace", "sync"]);

    fx.advance_main();
    fx.merge_main_into(&ws);
    fs::write(ws.join("src/negate.js"), NEGATE).unwrap();
    assert_eq!(
        fx.ok(&ws, &["diff", "--stat"]),
        "M src/math.js\nA src/negate.js\nA src/upstream.js\n"
    );
    let before = fx.ok(&ws, &["notices"]);
    assert!(
        before.contains("src/math.js: task") && before.contains("you changed"),
        "{before}"
    );

    let out = fx.ok(&ws, &["workspace", "rebase"]);
    assert!(
        out.contains("onto") && out.contains("wrote 0 files") && out.contains("now 1 files"),
        "{out}"
    );
    assert_eq!(fx.ok(&ws, &["diff", "--stat"]), "A src/negate.js\n");
    let after = fx.ok(&ws, &["notices"]);
    assert!(!after.contains("you changed"), "{after}");

    // Idempotent, and only forwards.
    let again = fx.ok(&ws, &["workspace", "rebase"]);
    assert!(again.contains("is already on"), "{again}");
    let first = git(&fx.repo, &["rev-parse", "HEAD~1"]);
    let back = fx.run(&ws, &["workspace", "rebase", &first]);
    assert_eq!(back.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&back.stderr).contains("does not descend"),
        "{}",
        String::from_utf8_lossy(&back.stderr)
    );
}

#[test]
fn sync_base_brings_main_into_a_directory_that_lacks_it() {
    let fx = Fx::new();
    let ws = fx.workspace("behind");
    fs::write(ws.join("src/negate.js"), NEGATE).unwrap();
    fx.advance_main();

    let out = fx.ok(&ws, &["workspace", "sync", "--base"]);
    assert!(out.contains("wrote 2 files"), "{out}");
    assert_eq!(
        fs::read_to_string(ws.join("src/upstream.js")).unwrap(),
        "module.exports = 1;\n"
    );
    assert!(fs::read_to_string(ws.join("src/math.js"))
        .unwrap()
        .contains("Number(a) + Number(b)"));
    assert_eq!(fx.ok(&ws, &["diff", "--stat"]), "A src/negate.js\n");
}

#[test]
fn a_conflicting_rebase_marks_the_conflict_and_moves_the_workspace() {
    let fx = Fx::new();
    let ws = fx.workspace("clash");
    let math = fs::read_to_string(ws.join("src/math.js")).unwrap();
    let mine = math.replace("return a + b;", "return a + b + 0;");
    fs::write(ws.join("src/math.js"), &mine).unwrap();
    fx.advance_main();
    let main = git(&fx.repo, &["rev-parse", "HEAD"]);

    let out = fx.run(&ws, &["workspace", "rebase"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{stdout}");
    assert!(
        stdout.contains("1 conflicting files") && stdout.contains("src/math.js"),
        "{stdout}"
    );
    let marked = fs::read_to_string(ws.join("src/math.js")).unwrap();
    assert!(
        marked.contains(&format!(
            "<<<<<<< yours\n  return a + b + 0;\n=======\n  return Number(a) + Number(b);\n>>>>>>> {}\n",
            &main[..12]
        )),
        "{marked}"
    );
    // The new base's other change came in, and the workspace sits on it.
    assert_eq!(
        fs::read_to_string(ws.join("src/upstream.js")).unwrap(),
        "module.exports = 1;\n"
    );
    assert_eq!(fx.ok(&ws, &["diff", "--stat"]), "M src/math.js\n");

    for args in [&["check"][..], &["verify"], &["workspace", "rebase"]] {
        let refused = fx.run(&ws, args);
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert_eq!(refused.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(
            stderr.contains("conflict markers") && stderr.contains("src/math.js"),
            "{args:?}: {stderr}"
        );
    }
}

/// The agent resolves the markers by hand; the next rebase accepts the
/// directory as the resolution and keeps only the agent's own change.
#[test]
fn a_hand_merged_rebase_is_accepted_once_the_markers_are_gone() {
    let fx = Fx::new();
    let ws = fx.workspace("hand");
    let math = fs::read_to_string(ws.join("src/math.js")).unwrap();
    fs::write(
        ws.join("src/math.js"),
        math.replace("return a + b;", "return a + b + 0;"),
    )
    .unwrap();
    fx.advance_main();
    assert_eq!(fx.run(&ws, &["workspace", "rebase"]).status.code(), Some(2));

    let marked = fs::read_to_string(ws.join("src/math.js")).unwrap();
    let start = marked.find("<<<<<<< yours\n").unwrap();
    let end = marked.find(">>>>>>> ").unwrap();
    let end = end + marked[end..].find('\n').unwrap() + 1;
    let resolved = format!(
        "{}  return Number(a) + Number(b) + 0;\n{}",
        &marked[..start],
        &marked[end..]
    );
    fs::write(ws.join("src/math.js"), &resolved).unwrap();

    let out = fx.ok(&ws, &["workspace", "rebase"]);
    assert!(out.contains("already on"), "{out}");
    assert_eq!(
        fs::read_to_string(ws.join("src/math.js")).unwrap(),
        resolved
    );
    let diff = fx.ok(&ws, &["diff"]);
    assert!(
        diff.contains("-  return Number(a) + Number(b);")
            && diff.contains("+  return Number(a) + Number(b) + 0;"),
        "only the agent's own change is left: {diff}"
    );
    assert_eq!(fx.ok(&ws, &["diff", "--stat"]), "M src/math.js\n");
}

#[test]
#[ignore = "runs Node natively"]
fn a_rebased_workspace_verifies_and_is_accepted_on_the_new_base() {
    let fx = Fx::new();
    let out = fx.ok(
        &fx.repo,
        &[
            "workspace",
            "new",
            "--no-deps",
            "--label",
            "neg",
            "-c",
            "neg: negate(x) returns -x",
        ],
    );
    let ws = PathBuf::from(out.trim());
    fx.advance_main();
    fx.merge_main_into(&ws);
    fs::write(ws.join("src/negate.js"), NEGATE).unwrap();
    fs::write(
        ws.join("test/negate.test.js"),
        "const test = require('node:test');\nconst assert = require('node:assert');\nlet negate = () => undefined;\ntry { negate = require('../src/negate.js'); } catch {}\ntest('[crit:neg] negates', () => assert.strictEqual(negate(2), -2));\n",
    )
    .unwrap();
    fx.ok(&ws, &["workspace", "rebase"]);

    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .arg("verify")
        .current_dir(&ws)
        .env("AVER_HOME", &fx.home)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(report.starts_with("DONE: PASS"), "{report}");

    let main = git(&fx.repo, &["rev-parse", "HEAD"]);
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .arg("accept")
        .current_dir(&ws)
        .env("AVER_HOME", &fx.home)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let branch = text
        .split_once(" as branch ")
        .and_then(|(_, rest)| rest.split_once(' '))
        .map(|(branch, _)| branch)
        .expect(&text);
    assert_eq!(git(&fx.repo, &["rev-parse", &format!("{branch}~1")]), main);
    let files = git(&fx.repo, &["show", "--name-only", "--format=", branch]);
    assert!(!files.contains("src/math.js"), "{files}");
}
