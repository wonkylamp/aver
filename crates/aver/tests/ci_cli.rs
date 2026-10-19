//! `aver ci` end to end: the CLI binary on a Git repository built from
//! the calc fixture, with checks run natively (ARCHITECTURE.md §2.9). Run
//! with `cargo test -p aver -- --ignored`.

mod common;
mod git_support;

use std::path::{Path as FsPath, PathBuf};
use std::process::{Command, Output};

use aver_core::*;
use common::{assert_matches_schema, fixture};
use data_encoding::BASE64URL_NOPAD;
use git_support::git_cmd;
use tempfile::TempDir;

const DIVIDE_CHECKED: &str =
    "function divide(a, b) {\n  if (b === 0) throw new Error('division by zero');\n  return a / b;\n}";
const STUB: &str = "function divide(a, b) {\n  throw new Error('not implemented');\n}";

/// A Git repository whose first commit is the calc fixture and whose second
/// applies `edit` to `src/math.js`. Returns the directory and both SHAs.
fn calc_repo(edit: impl Fn(String) -> String) -> (TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture("calc"), dir.path());
    let git = |args: &[&str]| -> String {
        let out = git_cmd(dir.path()).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "calc"]);
    let base = git(&["rev-parse", "HEAD"]);
    let math = dir.path().join("src/math.js");
    std::fs::write(&math, edit(std::fs::read_to_string(&math).unwrap())).unwrap();
    git(&["commit", "-q", "-am", "change math"]);
    let head = git(&["rev-parse", "HEAD"]);
    (dir, base, head)
}

fn copy_dir(from: &FsPath, to: &FsPath) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "node_modules" {
            continue;
        }
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn aver_ci(repo: &FsPath, base: &str, head: &str, out: &FsPath, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aver"))
        .arg("ci")
        .arg("--repo")
        .arg(repo)
        .args(["--base", base, "--head", head, "--out-dir"])
        .arg(out)
        .arg("--no-records")
        .args(extra)
        .output()
        .unwrap()
}

/// The receipt in `out`, checked against public-key.txt and its schema.
fn receipt(out: &FsPath) -> (ReceiptId, Receipt) {
    #[derive(serde::Deserialize)]
    struct File {
        receipt_id: ReceiptId,
        receipt: Receipt,
    }
    let file: File =
        serde_json::from_slice(&std::fs::read(out.join("receipt.json")).unwrap()).unwrap();
    let key = std::fs::read_to_string(out.join("public-key.txt")).unwrap();
    let key: [u8; 32] = BASE64URL_NOPAD
        .decode(key.trim().as_bytes())
        .unwrap()
        .try_into()
        .unwrap();
    file.receipt
        .verify(&VerifyingKey::from_bytes(&key).unwrap())
        .unwrap();
    assert_eq!(file.receipt.id(), file.receipt_id);
    assert_matches_schema("receipt", &file.receipt);
    (file.receipt_id, file.receipt)
}

fn summary(out: &FsPath) -> String {
    std::fs::read_to_string(out.join("summary.md")).unwrap()
}

#[test]
#[ignore = "runs Node natively"]
fn a_pr_whose_criteria_block_is_implemented_passes_with_exit_0() {
    let (repo, base, head) = calc_repo(|m| m.replace(STUB, DIVIDE_CHECKED));
    let out = tempfile::tempdir().unwrap();
    let event = out.path().join("event.json");
    let body = "Implements divide.\r\n\r\n```aver\r\n- div-basic: divide returns the quotient\r\n- div-zero: divide throws on division by zero\r\nprotected: test/**\r\n```\r\n";
    std::fs::write(
        &event,
        serde_json::json!({ "pull_request": { "number": 3, "title": "Implement divide", "body": body, "html_url": "https://github.com/o/r/pull/3" } })
            .to_string(),
    )
    .unwrap();
    let results: PathBuf = out.path().join("results");
    let run = aver_ci(
        repo.path(),
        &base,
        &head,
        &results,
        &[
            "--github-event",
            event.to_str().unwrap(),
            "--fail-on",
            "not-pass",
            "--subject",
            "o/r#3",
        ],
    );
    assert_eq!(run.status.code(), Some(0), "{run:?}");

    let (_, receipt) = receipt(&results);
    assert_eq!(receipt.body.verdict, Verdict::Pass);
    assert_eq!(receipt.body.schema, ReceiptSchema::V3);
    let info = receipt.body.ci.as_ref().unwrap();
    assert_eq!(info.subject.as_ref().unwrap().as_str(), "o/r#3");
    assert_eq!(
        (info.base_commit.as_str(), info.head_commit.as_str()),
        (base.as_str(), head.as_str())
    );
    let text = info.criteria_text.clone().unwrap();
    assert_eq!(
        text,
        "- div-basic: divide returns the quotient\n- div-zero: divide throws on division by zero\nprotected: test/**"
    );
    for c in receipt.body.checks.iter().filter(|c| !c.proves.is_empty()) {
        assert_eq!(c.transplant, Some(Transplant::Fail), "{c:?}");
    }

    // The reproduce line runs as written once the criteria file holds the
    // receipt's criteria_text, and refuses any other criteria.
    let criteria = out.path().join("criteria.txt");
    std::fs::write(&criteria, &text).unwrap();
    let command = receipt.body.reproduce.clone();
    let sha = command
        .split("--criteria-sha256 ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_owned();
    assert!(
        command.starts_with(&format!(
            "aver ci --repo <repo> --base {base} --head {head} --criteria-sha256 "
        )),
        "{command}"
    );
    let again = out.path().join("again");
    let run = aver_ci(
        repo.path(),
        &base,
        &head,
        &again,
        &[
            "--criteria-sha256",
            &sha,
            "--criteria",
            criteria.to_str().unwrap(),
            "--fail-on",
            "not-pass",
        ],
    );
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    assert_eq!(self::receipt(&again).1.body.verdict, Verdict::Pass);
    std::fs::write(&criteria, "- div-basic: divide returns the quotient\n").unwrap();
    let run = aver_ci(
        repo.path(),
        &base,
        &head,
        &again,
        &[
            "--criteria-sha256",
            &sha,
            "--criteria",
            criteria.to_str().unwrap(),
        ],
    );
    assert_eq!(run.status.code(), Some(2), "{run:?}");
    let criteria: Vec<&str> = receipt
        .body
        .criteria
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(criteria, ["div-basic", "div-zero"]);
    assert!(receipt.body.unproven.is_empty());

    let md = summary(&results);
    assert!(
        md.starts_with("## Aver: PASS\n\nEvery criterion is proven"),
        "{md}"
    );
    assert!(
        md.contains("- Proven `div-zero`: divide throws on division by zero"),
        "{md}"
    );
    assert!(md.contains("Protected paths: `test/**`."), "{md}");
    assert!(md.contains("protected: test/**\nAVER\naver ci"), "{md}");
    assert!(md.contains("Subject: `o/r#3`"), "{md}");
    assert!(md.contains("which fails without the change"), "{md}");
}

/// A Git repository whose first commit is the calc fixture and whose second
/// writes `files` (path, contents). Returns the directory and both SHAs.
fn calc_repo_with(files: &[(&str, String)]) -> (TempDir, String, String) {
    let (dir, base, _) = calc_repo(|m| m + "\n");
    let git = |args: &[&str]| -> String {
        let out = git_cmd(dir.path()).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["reset", "-q", "--hard", &base]);
    for (path, contents) in files {
        std::fs::write(dir.path().join(path), contents).unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "head"]);
    let head = git(&["rev-parse", "HEAD"]);
    (dir, base, head)
}

fn calc_file(path: &str) -> String {
    std::fs::read_to_string(fixture("calc").join(path)).unwrap()
}

/// A PR that changes behaviour on purpose and updates the test that pinned
/// the old behaviour: the base's copy fails on the new code, the PR's version
/// passes, so the test is CHANGED and the verdict REVIEW, with the test's
/// diff in the summary and every run's output in `runs/`.
#[test]
#[ignore = "runs Node natively"]
fn an_intended_behaviour_change_with_its_test_is_review() {
    let math = calc_file("src/math.js").replace("return a - b;", "return b - a;");
    let tests = calc_file("test/math.test.js")
        .replace("assert.equal(sub(5, 3), 2);", "assert.equal(sub(3, 5), 2);");
    let (repo, base, head) = calc_repo_with(&[("src/math.js", math), ("test/math.test.js", tests)]);
    let out = tempfile::tempdir().unwrap();
    let results = out.path().join("results");
    let run = aver_ci(repo.path(), &base, &head, &results, &["--fresh"]);
    assert_eq!(
        run.status.code(),
        Some(3),
        "REVIEW needs a maintainer: {run:?}\n{}",
        summary(&results)
    );
    let (_, receipt) = receipt(&results);
    let sub = receipt
        .body
        .checks
        .get("unit::math > sub returns the difference")
        .unwrap();
    assert_eq!(
        (sub.base, sub.head, sub.cause),
        (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::Changed))
    );
    assert_eq!(receipt.body.verdict, Verdict::Review);
    let md = summary(&results);
    for line in [
        "## Aver: REVIEW",
        "- `unit::math > sub returns the difference`: the old version of this test fails on the new code; the PR's version passes.",
        "-    assert.equal(sub(5, 3), 2);",
        "+    assert.equal(sub(3, 5), 2);",
    ] {
        assert!(md.contains(line), "{line}\n{md}");
    }
    let diagnostics: serde_json::Value =
        serde_json::from_slice(&std::fs::read(results.join("diagnostics.json")).unwrap()).unwrap();
    let failure = diagnostics["failures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "unit::math > sub returns the difference")
        .unwrap();
    assert_eq!(failure["own"], "pass");
    let logs: Vec<String> = std::fs::read_dir(results.join("runs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    for name in [
        "unit.base.1.log",
        "unit.head.1.log",
        "unit.head_tests.1.log",
    ] {
        assert!(logs.iter().any(|l| l == name), "{name} in {logs:?}");
    }
}

/// The reviewer's c2 scenario: on the calc fixture both divide tests fail
/// at base. The PR makes `divide` return `a / b` and rewrites the failing
/// zero-division test to expect `Infinity`. The base's copy of that test
/// still fails on the new code and only the PR's version passes, so it is
/// CHANGED, not PREEXISTING, and the verdict REVIEW with the test's diff,
/// never PASS. `div-basic`, unchanged, is fixed.
#[test]
#[ignore = "runs Node natively"]
fn rewriting_an_already_failing_test_is_review() {
    let math =
        calc_file("src/math.js").replace(STUB, "function divide(a, b) {\n  return a / b;\n}");
    let tests = calc_file("test/divide.test.js").replace(
        "assert.throws(() => divide(1, 0), /division by zero/);",
        "assert.equal(divide(1, 0), Infinity);",
    );
    let (repo, base, head) =
        calc_repo_with(&[("src/math.js", math), ("test/divide.test.js", tests)]);
    let out = tempfile::tempdir().unwrap();
    let results = out.path().join("results");
    let run = aver_ci(repo.path(), &base, &head, &results, &["--fresh"]);
    assert_eq!(
        run.status.code(),
        Some(3),
        "REVIEW needs a maintainer: {run:?}\n{}",
        summary(&results)
    );
    let (_, receipt) = receipt(&results);
    let entry = |name: &str| {
        receipt
            .body
            .checks
            .iter()
            .find(|e| e.id.as_str().starts_with("unit::") && e.id.as_str().contains(name))
            .unwrap_or_else(|| panic!("{name} in {:?}", receipt.body.checks))
    };
    let zero = entry("[crit:div-zero]");
    assert_eq!(
        (zero.base, zero.head, zero.cause),
        (BaseStatus::Fail, HeadStatus::Fail, Some(Cause::Changed))
    );
    let basic = entry("[crit:div-basic]");
    assert_eq!(
        (basic.base, basic.head, basic.cause),
        (BaseStatus::Fail, HeadStatus::Pass, None)
    );
    assert_eq!(receipt.body.verdict, Verdict::Review);
    let md = summary(&results);
    for line in [
        "## Aver: REVIEW",
        "only the PR's rewritten version passes, so it was rewritten, not fixed",
        "+  assert.equal(divide(1, 0), Infinity);",
    ] {
        assert!(md.contains(line), "{line}\n{md}");
    }
}

/// An invalid report path in the base's aver.toml is an exit-2 error that
/// names the problem once.
#[test]
#[ignore = "runs Node natively"]
fn a_bad_report_path_is_reported_once() {
    let config = calc_file("aver.toml").replace("/aver/out/unit.xml", "/tmp/unit.xml");
    let (repo, _, head) = calc_repo_with(&[("aver.toml", config)]);
    let out = tempfile::tempdir().unwrap();
    let run = aver_ci(repo.path(), &head, &head, out.path(), &[]);
    assert_eq!(run.status.code(), Some(2), "{run:?}");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(
        stderr.matches("must start with /aver/out/").count(),
        1,
        "{stderr}"
    );
    let md = summary(out.path());
    assert_eq!(md.matches("must start with /aver/out/").count(), 1, "{md}");
}

const WAITING: &str = "waiting for another aver process using";

/// `aver ci` on its own home takes the home's lock (`Home::lock`): a second
/// one waits, says so once on stderr, and goes on once the first lets go.
/// No checks run: the waiting process fails on its bad repository after it
/// gets the lock, which is all this needs.
#[test]
fn a_second_ci_waits_for_the_home_lock() {
    use std::io::Read;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let home = aver::keys::Home::new(dir.path().join("home"));
    let held = home.lock(|| panic!("nobody else holds it")).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(["ci", "--repo"])
        .arg(dir.path().join("no-such-repo"))
        .args(["--base", "a", "--head", "b", "--home"])
        .arg(home.root())
        .arg("--out-dir")
        .arg(dir.path().join("out"))
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut text = String::new();
        loop {
            match stderr.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    text.push_str(&String::from_utf8_lossy(&buf[..n]));
                    let _ = tx.send(text.clone());
                }
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = String::new();
    while !seen.contains(WAITING) {
        let left = deadline.saturating_duration_since(Instant::now());
        seen = rx
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("no waiting message: {seen:?}"));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the second process waits while the lock is held"
    );

    drop(held);
    let status = child.wait().unwrap();
    assert_eq!(
        status.code(),
        Some(2),
        "it went on and failed on the bad repository"
    );
    let mut text = seen;
    while let Ok(more) = rx.recv_timeout(Duration::from_secs(5)) {
        text = more;
    }
    assert_eq!(text.matches(WAITING).count(), 1, "{text}");

    let again = home.lock(|| panic!("the lock was released")).unwrap();
    drop(again);
}
