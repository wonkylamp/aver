//! `aver accept` with no number, inside a workspace directory of `aver
//! workspace new`: the solo agent's hand-off to a branch of the user's
//! repository (the verify it needs is Docker, so that part is `#[ignore]`).

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

fn aver(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(args)
        .current_dir(cwd)
        .env("AVER_HOME", home)
        .output()
        .unwrap()
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

#[test]
fn accept_without_a_number_outside_a_workspace_says_what_it_needs() {
    let dir = tempfile::tempdir().unwrap();
    let out = aver(&dir.path().join("home"), dir.path(), &["accept"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("name the attempt to accept") && err.contains("inside a workspace directory"),
        "{err}"
    );
}

#[test]
#[ignore = "runs Node natively"]
fn a_solo_workspace_is_accepted_as_a_branch_of_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let home = root.join("home");
    let repo = root.join("calc");
    copy_dir(&common::fixture("calc"), &repo);
    git_support::git(&repo, &["init", "--quiet", "--initial-branch=main"], b"");
    git_support::git(&repo, &["add", "-A"], b"");
    git_support::git(&repo, &["commit", "--quiet", "-m", "calc"], b"");
    git(&repo, &["config", "user.name", "Test User"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    let out = aver(
        &home,
        &repo,
        &[
            "workspace",
            "new",
            "--no-deps",
            "-c",
            "neg: negate(x) returns -x",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ws = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let label = ws.file_name().unwrap().to_string_lossy().into_owned();

    // Nothing changed yet: nothing to accept.
    let out = aver(&home, &ws, &["accept"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("changed nothing"), "{err}");

    fs::write(ws.join("src/negate.js"), "module.exports = (x) => -x;\n").unwrap();
    fs::write(
        ws.join("test/negate.test.js"),
        "const test = require('node:test');\nconst assert = require('node:assert');\nlet negate = () => undefined;\ntry { negate = require('../src/negate.js'); } catch {}\ntest('[crit:neg] negates', () => assert.strictEqual(negate(2), -2));\n",
    )
    .unwrap();
    let out = aver(&home, &ws, &["verify"]);
    let report = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{report}");
    let mut lines = report.lines();
    assert!(lines.next().unwrap().starts_with("DONE: PASS"), "{report}");
    assert!(lines.next().unwrap().starts_with("Base: "), "{report}");
    assert!(lines.next().unwrap().starts_with("Next:"), "{report}");
    let handoff = lines.next().unwrap();
    assert!(
        handoff.starts_with("Hand-off: `aver accept` in this directory")
            && handoff.contains("receipt rcpt_"),
        "{report}"
    );

    let out = aver(&home, &ws, &["accept"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The branch is the title's slug with a short id from the workspace;
    // with no commit in the Git view, the title is the first criterion's
    // text (docs/CLI.md).
    let branch = text
        .split_once(" as branch ")
        .and_then(|(_, rest)| rest.split_once(' '))
        .map(|(branch, _)| branch.to_owned())
        .expect(&text);
    assert!(
        branch.starts_with("aver/negate-x-returns-x-")
            && label.ends_with(&branch[branch.len() - 4..]),
        "{branch} for {label}: {text}"
    );
    assert!(text.contains("Verdict PASS: 1/1 criteria proven"), "{text}");
    assert_eq!(git(&repo, &["rev-parse", &format!("{branch}~1")]), base);
    let files = git(&repo, &["show", "--name-only", "--format=", &branch]);
    assert!(files.contains("src/negate.js"), "{files}");
    let message = git(&repo, &["log", "-1", "--format=%B", &branch]);
    assert!(message.contains("Aver-Receipt: rcpt_"), "{message}");
    assert!(message.starts_with("negate(x) returns -x\n"), "{message}");
    assert!(!message.contains("Aver-Approved-Review"), "{message}");
    // The user's checkout is untouched.
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), base);
    assert!(!repo.join("src/negate.js").exists());
}
