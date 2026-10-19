//! Review fixes for combine and the recovery tooling, each a repro with
//! scripted fake agents (`--agent-cmd`-style shell scripts) and the
//! scripted runner: `aver combine --partial` keeps the greedy subset its
//! docs describe, a failure is printed once, the batch summary's overlap
//! list names accepted tasks, every fetch command `aver workspace rm`
//! prints works, and `aver setup` sets up only the tools the repository or
//! user evidently uses. No Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aver::run::batch::record::{self, CombineStatus, ExcludedWhy};
use aver::run::batch::{input, BatchOutcome};
use git_support::git_str;
use run_support::*;

const README: &str = "calc\nl2\nl3\nl4\nl5\n";

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

fn cli(d: &TestHome, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = aver(&d.home, cwd, args, b"");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn tasks(text: &str) -> Vec<input::BatchTask> {
    input::parse_tasks(text).unwrap()
}

/// A task adding `src/<file>` and a test that passes once it exists.
fn adds(file: &str, crit: &str) -> String {
    format!(
        "mkdir -p src test\n\
         echo 'module.exports = (x) => x;' > src/{file}.js\n\
         echo '{{\"[crit:{crit}] works\": \"pass-if:src/{file}.js\"}}' > test/{file}.test.json\n"
    )
}

/// Line 3 of README.md set to `text`; `after` keeps what is there and
/// appends, as a task stacked on another.
fn line_3(text: &str, after: bool) -> String {
    let line = if after {
        format!("$(sed -n 3p README.md) + {text}")
    } else {
        text.to_owned()
    };
    format!(
        "{{ sed -n 1,2p README.md; echo \"{line}\"; sed -n '4,$p' README.md; }} > README.new\n\
         mv README.new README.md\n"
    )
}

/// #1 adds negate and #2 double, both rewriting line 3 of README.md (the
/// `module.exports` line of the review's repro), and #3 adds abs on top of
/// #1 (`after: #1`, so #1 is accepted before it starts) and rewrites the
/// same line again.
fn three_on_one_line(d: &TestHome) -> (PathBuf, BatchOutcome) {
    let repo = calc(d);
    let script = d.case_agent(
        "agent.sh",
        &[
            (
                "Add negate",
                &format!("{}{}", adds("neg", "neg"), line_3("negate", false)),
            ),
            (
                "Add double",
                &format!("{}{}", adds("dbl", "dbl"), line_3("double", false)),
            ),
            (
                "Add abs",
                &format!("{}{}", adds("abs", "abs"), line_3("abs", true)),
            ),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks(
            "Add negate\n  - neg: negate\n\
             Add double\n  - dbl: double\n\
             Add abs\n  after: #1\n  - abs: abs\n",
        ),
        &script,
        3,
    ));
    assert!(out.all_ready(), "{transcript}");
    (repo, out)
}

#[test]
fn partial_keeps_a_dependent_with_its_prerequisite_crit_combine_partial_conflict() {
    let d = TestHome::start();
    let (repo, out) = three_on_one_line(&d);
    let id = out.state.id.clone();

    let (code, json, err) = cli(&d, &repo, &["combine", "--all-pass", "--partial", "--json"]);
    assert_eq!(code, 1, "#2 was left out: {json}{err}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["status"], "partial", "{json}");
    let mut included: Vec<u64> = v["included"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["number"].as_u64().unwrap())
        .collect();
    included.sort_unstable();
    assert_eq!(included, [1, 3], "#3 brings #1: {json}");
    let excluded = v["excluded"].as_array().unwrap();
    assert_eq!(excluded.len(), 1, "{json}");
    assert_eq!(excluded[0]["number"], 2, "{json}");
    assert_eq!(excluded[0]["why"]["kind"], "conflict", "{json}");
    assert_eq!(
        excluded[0]["why"]["with"]["number"], 1,
        "#2 conflicts with the kept #1, not an excluded task: {json}"
    );
    let branch = v["branch"].as_str().unwrap().to_owned();
    assert_eq!(
        git_str(&repo, &["show", &format!("{branch}:README.md")]),
        "calc\nl2\nnegate + abs\nl4\nl5"
    );

    let rec = record::last(&d.home, &id).unwrap();
    assert_eq!(rec.status, CombineStatus::Partial);
    assert_eq!(rec.included, [1, 3]);
    assert_eq!(rec.excluded.len(), 1);
    assert_eq!(rec.excluded[0].number, 2);
    assert_eq!(
        rec.excluded[0].why,
        ExcludedWhy::Conflict {
            with: 1,
            paths: vec!["README.md".into()]
        }
    );
}

/// #1 adds src/greet.js and exports it from src/math.js; #2 adds
/// src/shout.js, whose test (as `shout()` uppercasing `greet()` and
/// expecting "HEY") holds only while src/math.js is as #2 found it. The
/// merge is clean as text and #2 fails on it.
fn greet_and_shout(d: &TestHome) -> (PathBuf, BatchOutcome) {
    let repo = calc(d);
    let script = d.case_agent(
        "agent.sh",
        &[
            (
                "Add greet",
                &format!(
                    "{}echo 'exports.greet = require(\"./greet\");' >> src/math.js\n",
                    adds("greet", "greet")
                ),
            ),
            (
                "Add shout",
                "mkdir -p test\ncp src/math.js src/shout.js\n\
                 echo '{\"[crit:shout] shouts HEY\": \"agree:src/shout.js=src/math.js\"}' > test/shout.test.json\n",
            ),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks("Add greet\n  - greet: greet\nAdd shout\n  - shout: shout\n"),
        &script,
        2,
    ));
    assert!(out.all_ready(), "{transcript}");
    (repo, out)
}

#[test]
fn partial_exports_the_one_task_left_crit_combine_partial_semantic() {
    let d = TestHome::start();
    let (repo, out) = greet_and_shout(&d);
    let id = out.state.id.clone();

    let (code, text, err) = cli(&d, &repo, &["combine", "--all-pass", "--partial"]);
    assert_eq!(code, 1, "#2 was left out: {text}{err}");
    let rec = record::last(&d.home, &id).unwrap();
    assert_eq!(rec.status, CombineStatus::Partial, "{text}");
    assert_eq!(rec.included, [1], "{text}");
    assert_eq!(rec.excluded.len(), 1, "{text}");
    assert_eq!(rec.excluded[0].number, 2);
    assert!(
        matches!(rec.excluded[0].why, ExcludedWhy::NotPass { .. }),
        "{:?}",
        rec.excluded
    );
    let branch = rec.branch.clone().expect("#1 was exported");
    assert!(
        text.contains(&format!("Combined 1 runs as branch {branch}")),
        "{text}"
    );
    assert!(
        text.contains("#2 (\"Add shout\") left out: #2 (\"Add shout\") is"),
        "{text}"
    );
    let files = git_str(&repo, &["show", "--name-only", "--format=", &branch]);
    assert!(
        files.contains("src/greet.js") && !files.contains("src/shout.js"),
        "{files}"
    );
}

#[test]
fn a_failure_is_printed_once_crit_combine_tools_once() {
    let d = TestHome::start();
    let (repo, _) = greet_and_shout(&d);
    let (code, text, err) = cli(&d, &repo, &["combine", "--all-pass"]);
    assert_eq!(code, 1, "{text}{err}");
    assert!(
        text.contains("Merged cleanly, but not every task passes on the result"),
        "{text}"
    );
    assert_eq!(
        text.matches("#2 \"Add shout\": FAIL").count(),
        1,
        "the verdict line: {text}"
    );
    assert!(
        !text.contains("#2 (\"Add shout\"): #2 (\"Add shout\") is"),
        "not again as a left-out line: {text}"
    );
}

#[test]
fn the_overlap_list_names_accepted_tasks_crit_combine_tools_overlap() {
    let d = TestHome::start();
    let (repo, out) = three_on_one_line(&d);
    let pairs: Vec<([u32; 2], String)> = out
        .summary
        .collisions
        .iter()
        .map(|c| (c.tasks, c.path.clone()))
        .collect();
    assert!(
        pairs.contains(&([1, 2], "README.md".into())),
        "#1 was accepted when #3 started, and still overlaps #2: {pairs:?}"
    );
    assert!(pairs.contains(&([2, 3], "README.md".into())), "{pairs:?}");
    let (_, text, err) = cli(&d, &repo, &["status"]);
    assert!(
        text.contains("overlap: #1 and #2 both change README.md"),
        "{text}{err}"
    );
}

fn git(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Agent")
        .env("GIT_AUTHOR_EMAIL", "agent@example.com")
        .env("GIT_COMMITTER_NAME", "Agent")
        .env("GIT_COMMITTER_EMAIL", "agent@example.com")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn every_printed_fetch_command_works_crit_combine_tools_fetch_cmd() {
    let d = TestHome::start();
    let repo = d.repo("calc", &[("README.md", README)]);
    let base = git_str(&repo, &["rev-parse", "HEAD"]);
    let (code, out, err) = cli(&d, &repo, &["workspace", "new", ".", "--no-deps"]);
    assert_eq!(code, 0, "{out}{err}");
    let ws = PathBuf::from(out.trim());
    let label = ws.file_name().unwrap().to_string_lossy().into_owned();
    let view_branch = format!("aver/{label}");

    // The agent commits on its Git view's branch, and `aver accept` exports
    // the squashed result under that same name in the repository.
    std::fs::write(ws.join("notes.txt"), "work\n").unwrap();
    let (ok, text) = git(&ws, &["add", "notes.txt"]);
    assert!(ok, "{text}");
    let (ok, text) = git(&ws, &["commit", "-q", "-m", "agent work"]);
    assert!(ok, "{text}");
    let (ok, text) = git(&repo, &["branch", &view_branch, &base]);
    assert!(ok, "{text}");

    let (code, _, err) = cli(&d, &repo, &["workspace", "rm", ws.to_str().unwrap()]);
    assert_eq!(code, 2, "kept for its unfetched commit: {err}");
    let commands: Vec<&str> = err
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("git -C "))
        .collect();
    assert!(
        commands.iter().any(|c| c.contains(" fetch ")),
        "a fetch command: {err}"
    );
    for c in &commands {
        let status = Command::new("sh")
            .args(["-c", c])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(0), "{c}\n{err}");
    }
    assert_eq!(
        git_str(&repo, &["rev-parse", &view_branch]),
        base,
        "the accepted branch is untouched"
    );
    assert_eq!(
        git_str(
            &repo,
            &[
                "log",
                "-1",
                "--format=%s",
                &format!("{view_branch}-commits")
            ]
        ),
        "agent work"
    );
    // Fetched into a branch, the commit holds the directory's changes, so
    // the removal needs no --force.
    let (code, _, err) = cli(&d, &repo, &["workspace", "rm", ws.to_str().unwrap()]);
    assert_eq!(code, 0, "a branch holds its changes: {err}");
    assert!(!err.contains("fetch"), "nothing left to fetch: {err}");
    assert!(!ws.exists());
}

/// `aver setup --yes` in `repo` with only `tools` (fake executables) and
/// Git on `PATH`.
fn setup(root: &Path, repo: &Path, tools: &[&str], args: &[&str]) -> (i32, String) {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for t in tools {
        let p = bin.join(t);
        std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = Command::new(aver_bin())
        .arg("setup")
        .args(args)
        .current_dir(repo)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("AVER_HOME", root.join("aver-home"))
        .env("HOME", root.join("home"))
        .env("AVER_APPS_DIR", root.join("apps"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn setup_writes_only_the_tools_in_use_crit_combine_tools_setup_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "x\n").unwrap();
    let (ok, text) = git(&repo, &["init", "-q"]);
    assert!(ok, "{text}");
    let tools = ["claude", "code", "cursor"];

    let (code, text) = setup(root, &repo, &tools, &["--yes"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        repo.join(".claude/settings.json").is_file(),
        "Claude Code is installed: {text}"
    );
    for p in [
        ".vscode/mcp.json",
        ".github/copilot-instructions.md",
        ".cursor",
    ] {
        assert!(!repo.join(p).exists(), "{p} written: {text}");
    }
    assert!(text.contains("Skipping VS Code"), "{text}");
    assert!(text.contains("aver setup --tool vscode"), "{text}");
    assert!(text.contains("Skipping Cursor"), "{text}");

    // A `.vscode/` directory shows the editor, not Copilot: nothing for it.
    std::fs::create_dir_all(repo.join(".vscode")).unwrap();
    let (code, text) = setup(root, &repo, &tools, &["--yes"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("Skipping VS Code"), "{text}");
    assert_eq!(
        std::fs::read_dir(repo.join(".vscode")).unwrap().count(),
        0,
        "{text}"
    );
    assert!(!repo.join(".github").exists(), "{text}");

    // A repository with Copilot instructions of its own gets Aver's block.
    std::fs::create_dir_all(repo.join(".github")).unwrap();
    std::fs::write(repo.join(".github/copilot-instructions.md"), "Mine.\n").unwrap();
    let (code, text) = setup(root, &repo, &tools, &["--yes"]);
    assert_eq!(code, 0, "{text}");
    let copilot = std::fs::read_to_string(repo.join(".github/copilot-instructions.md")).unwrap();
    assert!(
        copilot.starts_with("Mine.\n") && copilot.contains("BEGIN aver"),
        "{text}"
    );
    assert!(!repo.join(".cursor").exists(), "{text}");

    // Copilot on request, in a repository that showed no sign of it.
    let other = root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let (ok, text) = git(&other, &["init", "-q"]);
    assert!(ok, "{text}");
    let (code, text) = setup(root, &other, &[], &["--yes", "--tool", "copilot"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        other.join(".github/copilot-instructions.md").is_file(),
        "{text}"
    );
}

/// #1 adds greet to src/math.js; #2's test holds only while src/math.js
/// is as it found it, so it fails beside #1; #3 adds double. The whole
/// merge fails; kept greedily, #1 stays, #2 is left out for failing with
/// it, and #3, fine with #1, stays: never left out for #2.
#[test]
fn partial_keeps_a_later_task_that_passes_with_the_kept_ones_crit_combine_partial_greedy() {
    let d = TestHome::start();
    let repo = calc(&d);
    let script = d.case_agent(
        "agent.sh",
        &[
            (
                "Add greet",
                &format!(
                    "{}echo 'exports.greet = require(\"./greet\");' >> src/math.js\n",
                    adds("greet", "greet")
                ),
            ),
            (
                "Add shout",
                "mkdir -p test\ncp src/math.js src/shout.js\n\
                 echo '{\"[crit:shout] shouts HEY\": \"agree:src/shout.js=src/math.js\"}' > test/shout.test.json\n",
            ),
            ("Add double", &adds("dbl", "dbl")),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks("Add greet\n  - greet: greet\nAdd shout\n  - shout: shout\nAdd double\n  - dbl: double\n"),
        &script,
        3,
    ));
    assert!(out.all_ready(), "{transcript}");
    let id = out.state.id.clone();

    let (code, text, err) = cli(&d, &repo, &["combine", "--all-pass", "--partial"]);
    assert_eq!(code, 1, "#2 was left out: {text}{err}");
    let rec = record::last(&d.home, &id).unwrap();
    assert_eq!(rec.status, CombineStatus::Partial, "{text}");
    assert_eq!(rec.included, [1, 3], "{text}");
    assert_eq!(rec.excluded.len(), 1, "{text}");
    assert_eq!(rec.excluded[0].number, 2);
    assert!(
        matches!(rec.excluded[0].why, ExcludedWhy::NotPass { .. }),
        "{:?}",
        rec.excluded
    );
    assert_eq!(
        text.matches("#2 (\"Add shout\") left out").count(),
        1,
        "{text}"
    );
    let branch = rec.branch.expect("exported");
    let files = git_str(&repo, &["show", "--name-only", "--format=", &branch]);
    assert!(
        files.contains("src/greet.js")
            && files.contains("src/dbl.js")
            && !files.contains("src/shout.js"),
        "{files}"
    );
}
