//! `aver combine` over workspace directories made outside `aver run`
//! (`aver workspace new`), named by directory, attempt id or the branch
//! `aver accept` exported, with the scripted runner: each is verified
//! against its own task on the merge, both appending to one file merge as
//! a union, and inputs on different bases are refused. No Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::fs;
use std::path::{Path, PathBuf};

use git_support::git_str;
use run_support::*;

const README: &str = "calc\nl2\n";

fn cli(d: &TestHome, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = aver(&d.home, cwd, args, b"");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn calc(d: &TestHome) -> PathBuf {
    d.repo(
        "calc",
        &[
            ("aver.toml", &aver_toml()),
            ("src/math.js", "exports.add = (a, b) => a + b;\n"),
            ("test/math.test.json", r#"{"adds": "pass-if:src/math.js"}"#),
            ("README.md", README),
        ],
    )
}

/// A workspace directory on `repo`'s HEAD for criterion `crit`, adding
/// `src/<crit>.js` with its test and appending a line to README.md.
fn solo(d: &TestHome, repo: &Path, crit: &str) -> PathBuf {
    let (code, out, err) = cli(
        d,
        repo,
        &[
            "workspace",
            "new",
            "--no-deps",
            "--label",
            crit,
            "-c",
            &format!("{crit}: adds {crit}"),
        ],
    );
    assert_eq!(code, 0, "{out}{err}");
    let ws = PathBuf::from(out.trim());
    fs::write(ws.join(format!("src/{crit}.js")), "module.exports = 1;\n").unwrap();
    fs::write(
        ws.join(format!("test/{crit}.test.json")),
        format!(r#"{{"[crit:{crit}] adds {crit}": "pass-if:src/{crit}.js"}}"#),
    )
    .unwrap();
    let readme = fs::read_to_string(ws.join("README.md")).unwrap();
    fs::write(ws.join("README.md"), format!("{readme}{crit} added\n")).unwrap();
    ws
}

fn attempt_of(ws: &Path) -> String {
    let file = fs::read_to_string(ws.join(".aver/workspace.toml")).unwrap();
    let line = file
        .lines()
        .find(|l| l.starts_with("attempt"))
        .expect("an attempt");
    line.split('"').nth(1).unwrap().to_owned()
}

#[test]
fn solo_workspaces_combine_by_name_and_attempt_crit_combine_solo() {
    let d = TestHome::start();
    let repo = calc(&d);
    let neg = solo(&d, &repo, "neg");
    let dbl = solo(&d, &repo, "dbl");

    let (code, text, err) = cli(&d, &repo, &["combine", "neg", &attempt_of(&dbl)]);
    assert_eq!(code, 0, "{text}{err}");
    assert!(text.contains("workspace neg \"adds neg\": PASS"), "{text}");
    assert!(text.contains("workspace dbl \"adds dbl\": PASS"), "{text}");
    let branch = text
        .split_once(" as branch ")
        .and_then(|(_, rest)| rest.split_once(' '))
        .map(|(b, _)| b.to_owned())
        .expect(&text);
    let files = git_str(&repo, &["show", "--name-only", "--format=", &branch]);
    assert!(
        files.contains("src/neg.js") && files.contains("src/dbl.js"),
        "{files}"
    );
    // Both appended to README.md: a union, in input order.
    assert_eq!(
        git_str(&repo, &["show", &format!("{branch}:README.md")]),
        "calc\nl2\nneg added\ndbl added"
    );
    let message = git_str(&repo, &["log", "-1", "--format=%B", &branch]);
    assert_eq!(
        message.matches("Aver-Receipt: rcpt_").count(),
        2,
        "{message}"
    );
    // The combine accepts neither: both directories stay.
    assert!(neg.exists() && dbl.exists());
}

#[test]
fn an_accepted_workspace_combines_by_its_branch_crit_combine_solo_branch() {
    let d = TestHome::start();
    let repo = calc(&d);
    let neg = solo(&d, &repo, "neg");
    solo(&d, &repo, "dbl");
    let (code, text, err) = cli(&d, &neg, &["accept"]);
    assert_eq!(code, 0, "{text}{err}");
    let accepted = text
        .split_once(" as branch ")
        .and_then(|(_, rest)| rest.split_once(' '))
        .map(|(b, _)| b.to_owned())
        .expect(&text);

    let (code, text, err) = cli(
        &d,
        &repo,
        &["combine", &accepted, "dbl", "--branch", "aver/both"],
    );
    assert_eq!(code, 0, "{text}{err}");
    let files = git_str(&repo, &["show", "--name-only", "--format=", "aver/both"]);
    assert!(
        files.contains("src/neg.js") && files.contains("src/dbl.js"),
        "{files}"
    );
}

#[test]
fn solo_workspaces_on_different_bases_are_refused_crit_combine_solo_base() {
    let d = TestHome::start();
    let repo = calc(&d);
    solo(&d, &repo, "neg");
    fs::write(repo.join("NOTES.md"), "moved on\n").unwrap();
    git_support::git(&repo, &["add", "-A"], b"");
    git_support::git(&repo, &["commit", "--quiet", "-m", "moves on"], b"");
    solo(&d, &repo, "dbl");

    let (code, text, err) = cli(&d, &repo, &["combine", "neg", "dbl"]);
    assert_eq!(code, 2, "{text}{err}");
    assert!(
        err.contains("workspace neg and workspace dbl started from different commits"),
        "{err}"
    );
    let unknown = cli(&d, &repo, &["combine", "neg", "nothing-here"]);
    assert_eq!(unknown.0, 2);
    assert!(
        unknown
            .2
            .contains("nothing-here is no run, task number, attempt"),
        "{}",
        unknown.2
    );
}
