//! "Nothing reaches you until it's proven" in CI (review fixes H1 and H2):
//! a base that ran no test never explains a head failure, a test the
//! change breaks and then skips or deletes is the change's failure, and
//! `aver ci` exits 3 for `REVIEW` and `UNPROVEN`. The scripted tests run in
//! process with no Docker; the `#[ignore]`d ones run the `aver` binary
//! natively on copies of `fixtures/calc`, `go-calc` and `rust-calc`
//! (`make test-native`).

mod common;
mod git_support;
mod scripted;
mod world;

use std::path::Path as FsPath;
use std::process::Command;
use std::sync::Arc;

use aver::ci::{self, summary, Ci, FailOn, Spec, EXIT_FAIL, EXIT_NEEDS_MAINTAINER};
use aver::env::StaticResolver;
use aver::exec::{ExecConfig, Executor};
use aver::keys::{self, Home};
use aver_core::*;
use git_support::git_cmd;
use tempfile::TempDir;
use world::PINNED_IMAGE;

struct Repo {
    dir: TempDir,
}

impl Repo {
    fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    fn path(&self) -> &FsPath {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = git_cmd(self.path()).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Writes `files` (`None` deletes one) and commits; returns the SHA.
    fn commit(&self, files: &[(&str, Option<&str>)], message: &str) -> String {
        for (path, content) in files {
            let full = self.path().join(path);
            match content {
                Some(content) => {
                    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                    std::fs::write(full, content).unwrap();
                }
                None => std::fs::remove_file(full).unwrap(),
            }
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

struct Harness {
    _dir: TempDir,
    server: keys::AgentKey,
    ci: Ci,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::new(dir.path().join("home"));
    let init = keys::init(&home).unwrap();
    let store = Arc::new(home.open_store().unwrap());
    let executor = Executor::new(
        store.clone(),
        Arc::new(scripted::Scripted::default()),
        ExecConfig::default(),
    );
    let ci = Ci::new(
        store,
        Arc::new(init.server.signing_key.clone()),
        &init.admin,
        Arc::new(StaticResolver::default()),
        executor,
    );
    Harness {
        _dir: dir,
        server: init.server,
        ci,
    }
}

async fn run(repo: &Repo, base: &str, head: &str) -> (Harness, ci::Outcome) {
    let h = harness();
    let request = ci::Request::new(
        repo.path().display().to_string(),
        base.to_owned(),
        head.to_owned(),
        Spec::regression_only(),
        None,
    );
    let outcome = h.ci.run(request).await.unwrap();
    (h, outcome)
}

fn markdown(h: &Harness, outcome: &ci::Outcome) -> String {
    summary::render(&summary::Context {
        outcome,
        public_key: &h.server.to_file().public_key,
        executor: "scripted-v0",
        throwaway_key: true,
        fail_on: FailOn::Unexplained,
        viewer_url: None,
        reproduce: "aver ci --base a --head b",
    })
}

/// What the agent report says about a check whose base ran no test.
fn agent_cause(outcome: &ci::Outcome) -> String {
    let mut out = String::new();
    aver::run::report::no_tests_on_base(Some(&outcome.diagnostics), &mut out);
    out
}

fn config(check: &str) -> String {
    format!("[env]\nimage = \"{PINNED_IMAGE}\"\n\n{check}")
}

/// A check whose every run prints `line` and exits 1, on both sides: the
/// toolchain refused to start the tests. The entry is `ENV`, never
/// explained by the base, so the default exits 1, and the run log's fix is
/// the cause in the summary and the agent report.
async fn refused(check: &str, line: &str, fix: &str) {
    let repo = Repo::new();
    let base = repo.commit(
        &[
            ("aver.toml", Some(&config(check))),
            ("refusal.txt", Some(line)),
            ("calc.src", Some("add = a + b\n")),
        ],
        "base",
    );
    let head = repo.commit(&[("calc.src", Some("add = a - b\n"))], "break add");
    let (h, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    let entry = body.checks.iter().next().expect("one entry");
    assert_eq!(entry.cause, Some(Cause::Env), "{entry:?}");
    assert_eq!(body.verdict, Verdict::BlockedNotOwn);
    assert_eq!(FailOn::Unexplained.exit_code(body), EXIT_FAIL);
    assert_eq!(FailOn::FailOwn.exit_code(body), 0, "fail-own stays lenient");
    let failure = &outcome.diagnostics.failures[0];
    let note = failure.note.as_deref().unwrap();
    assert!(note.contains(fix), "{note}");
    let md = markdown(&h, &outcome);
    assert!(md.contains(fix), "{md}");
    assert!(md.contains(line.trim()), "{md}");
    assert!(
        !md.contains("has no per-test results"),
        "the missing report is not the cause: {md}"
    );
    assert!(
        md.contains("The check fails: `--fail-on unexplained`"),
        "{md}"
    );
    let report = agent_cause(&outcome);
    assert!(report.contains(fix), "{report}");
    assert!(report.contains(line.trim()), "{report}");
    assert!(
        report.contains("````") || report.contains("```"),
        "fenced: {report}"
    );
}

#[tokio::test]
async fn a_go_older_than_go_mod_exits_1_with_the_fix_crit_ci_trust_go_old() {
    refused(
        "[[check]]\nid = \"go\"\nrun = [\"say\", \"refusal.txt\", \"1\"]\nreport = { format = \"go-json\", path = \"/aver/out/go.json\" }\n",
        "go: go.mod requires go >= 1.99 (running go 1.24.7; GOTOOLCHAIN=local)\n",
        "install the Go version go.mod asks for",
    )
    .await;
}

#[tokio::test]
async fn a_rust_older_than_rust_version_exits_1_with_the_fix_crit_ci_trust_rust_old() {
    refused(
        "[[check]]\nid = \"rust\"\nrun = [\"say\", \"refusal.txt\", \"101\"]\n",
        "error: package `calc v0.1.0 (/work)` cannot be built because it requires rustc 1.99 or newer, while the currently active rustc version is 1.97.0\n",
        "rust-version asks for",
    )
    .await;
}

#[tokio::test]
async fn no_test_script_on_either_side_exits_1_crit_ci_trust_no_test_script() {
    refused(
        "[[check]]\nid = \"npm-test\"\nrun = [\"say\", \"refusal.txt\", \"1\"]\n",
        "npm error Missing script: \"test\"\n",
        "add a \"test\" script to package.json",
    )
    .await;
}

/// Any runner error before the first test, with no toolchain line Aver
/// knows: a check that declares a report and writes none on any base run
/// ran no test there, so it explains nothing either.
#[tokio::test]
async fn a_runner_that_fails_before_its_first_test_exits_1_crit_ci_trust_no_test_script() {
    let repo = Repo::new();
    let check = "[[check]]\nid = \"unit\"\nrun = [\"junit\", \"unit.xml\", \"missing.json\"]\nreport = { format = \"junit\", path = \"/aver/out/unit.xml\" }\n";
    let base = repo.commit(
        &[
            ("aver.toml", Some(&config(check))),
            ("calc.src", Some("add = a + b\n")),
        ],
        "base",
    );
    let head = repo.commit(&[("calc.src", Some("add = a - b\n"))], "break add");
    let (_, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    assert_eq!(body.checks.get("unit").unwrap().cause, Some(Cause::Env));
    assert_eq!(FailOn::Unexplained.exit_code(body), EXIT_FAIL);
    assert_eq!(
        outcome.diagnostics.failures[0].note.as_deref(),
        Some(aver::diagnostics::NO_TESTS_NOTE)
    );
}

fn unit_dir() -> String {
    config("[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = { format = \"junit\", path = \"/aver/out/unit.xml\" }\ncovers = [\"src/**\", \"test/**\"]\n")
}

/// The base: `adds` passes while `src/add.js` exists, `keeps` always.
fn calc_like() -> (Repo, String) {
    let repo = Repo::new();
    let base = repo.commit(
        &[
            ("aver.toml", Some(&unit_dir())),
            (
                "test/math.test.json",
                Some(r#"{"adds": "pass-if:src/add.js", "keeps": "pass"}"#),
            ),
            ("src/add.js", Some("module.exports = (a, b) => a + b;")),
        ],
        "base",
    );
    (repo, base)
}

#[tokio::test]
async fn skipping_a_test_the_change_breaks_exits_1_crit_ci_trust_skip_test() {
    let (repo, base) = calc_like();
    let head = repo.commit(
        &[
            ("src/add.js", None),
            (
                "test/math.test.json",
                Some(r#"{"adds": "skip", "keeps": "pass"}"#),
            ),
        ],
        "break add, skip its test",
    );
    let (h, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    assert_eq!(
        body.checks.get("unit::adds").unwrap().cause,
        Some(Cause::New)
    );
    assert_eq!(body.verdict, Verdict::FailOwn);
    assert_eq!(FailOn::Unexplained.exit_code(body), EXIT_FAIL);
    assert_eq!(FailOn::FailOwn.exit_code(body), EXIT_FAIL);
    let md = markdown(&h, &outcome);
    assert!(md.starts_with("## Aver: FAIL_OWN"), "{md}");
}

/// An ava project read through its TAP stream (JS-9): two test files, so
/// each title carries its file, as `ava --tap` prints it.
fn ava_like() -> (Repo, String) {
    let repo = Repo::new();
    let check = "[[check]]\nid = \"unit\"\nrun = [\"tap-dir\", \"unit.tap\", \"test\"]\nreport = { format = \"tap\", path = \"/aver/out/unit.tap\" }\ncovers = [\"src/**\", \"test/**\"]\n";
    let base = repo.commit(
        &[
            ("aver.toml", Some(&config(check))),
            (
                "test/math.test.json",
                Some(r#"{"adds": "pass-if:src/add.js", "later": "todo"}"#),
            ),
            ("test/other.test.json", Some(r#"{"keeps": "pass"}"#)),
            ("src/add.js", Some("module.exports = (a, b) => a + b;")),
        ],
        "base",
    );
    (repo, base)
}

#[tokio::test]
async fn skipping_a_broken_ava_test_is_fail_own_crit_ci_trust_skip_test() {
    let (repo, base) = ava_like();
    let head = repo.commit(
        &[
            ("src/add.js", None),
            (
                "test/math.test.json",
                Some(r#"{"adds": "skip", "later": "todo"}"#),
            ),
        ],
        "break add, test.skip its test",
    );
    let (h, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    assert_eq!(
        body.checks.get("unit::math > adds").map(|e| e.cause),
        Some(Some(Cause::New)),
        "{:?}",
        body.checks
    );
    assert_eq!(body.verdict, Verdict::FailOwn);
    assert_eq!(FailOn::Unexplained.exit_code(body), EXIT_FAIL);
    let md = markdown(&h, &outcome);
    assert!(md.starts_with("## Aver: FAIL_OWN"), "{md}");
}

#[tokio::test]
async fn a_good_change_to_an_ava_project_passes() {
    let (repo, base) = ava_like();
    let head = repo.commit(
        &[
            ("src/sub.js", Some("module.exports = (a, b) => a - b;")),
            (
                "test/math.test.json",
                Some(r#"{"adds": "pass-if:src/add.js", "later": "todo", "subtracts": "pass-if:src/sub.js"}"#),
            ),
        ],
        "add sub",
    );
    let (h, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    assert_eq!(body.verdict, Verdict::Pass, "{}", markdown(&h, &outcome));
    assert!(
        body.checks.get("unit::math > subtracts").is_some(),
        "read per test: {:?}",
        body.checks
    );
    assert_eq!(FailOn::Unexplained.exit_code(body), 0);
}

/// JS-8: a check read by its exit code alone that fails on the base runs
/// the head's own versions of the tests once. Its own result is `fail`
/// from that first failing run whatever later runs say, so reruns there
/// could not change the verdict; the judged head still reruns.
#[tokio::test]
async fn an_exit_only_check_red_on_the_base_runs_the_head_tests_once() {
    let repo = Repo::new();
    let check = "[[check]]\nid = \"lint-and-test\"\nrun = [\"exit\", \"1\"]\n";
    let base = repo.commit(
        &[
            ("aver.toml", Some(&config(check))),
            ("test/a.test.js", Some("old")),
            ("src/a.js", Some("a")),
        ],
        "base",
    );
    let head = repo.commit(
        &[("test/a.test.js", Some("new")), ("src/a.js", Some("b"))],
        "change a test and the code",
    );
    let (_, outcome) = run(&repo, &base, &head).await;
    let count = |side| outcome.runs.iter().filter(|r| r.side == side).count();
    assert_eq!(
        count(aver::verify::Side::HeadTests),
        1,
        "{:?}",
        outcome.runs
    );
    assert_eq!(count(aver::verify::Side::Head), 3, "{:?}", outcome.runs);
    assert_eq!(outcome.receipt.body.verdict, Verdict::BlockedNotOwn);
}

#[tokio::test]
async fn deleting_the_test_file_of_a_broken_test_exits_1_crit_ci_trust_delete_test() {
    let (repo, base) = calc_like();
    let head = repo.commit(
        &[("src/add.js", None), ("test/math.test.json", None)],
        "break add, delete its tests",
    );
    let (_, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    let adds = body.checks.get("unit::adds").unwrap();
    assert_eq!(adds.cause, Some(Cause::New));
    assert_eq!(body.verdict, Verdict::FailOwn);
    assert_eq!(FailOn::Unexplained.exit_code(body), EXIT_FAIL);
    let failure = outcome
        .diagnostics
        .failures
        .iter()
        .find(|f| f.id == adds.id)
        .unwrap();
    assert_eq!(
        failure.note.as_deref(),
        Some(aver::diagnostics::HIDDEN_REGRESSION_NOTE)
    );
}

/// An intended behaviour change with its rewritten expectation: the base's
/// copy fails on the new code, the head's version passes. `CHANGED`,
/// `REVIEW`: nothing broke, but a maintainer approves the spec change, so
/// the default exits 3.
#[tokio::test]
async fn an_edited_expectation_is_review_and_exits_3_crit_ci_trust_review_exit3() {
    let (repo, base) = calc_like();
    let head = repo.commit(
        &[
            ("src/add.js", None),
            ("src/sum.js", Some("module.exports = (a, b) => a + b;")),
            (
                "test/math.test.json",
                Some(r#"{"adds": "pass-if:src/sum.js", "keeps": "pass"}"#),
            ),
        ],
        "rename add to sum",
    );
    let (h, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    assert_eq!(
        body.checks.get("unit::adds").unwrap().cause,
        Some(Cause::Changed)
    );
    assert_eq!(body.verdict, Verdict::Review);
    assert_eq!(FailOn::Unexplained.exit_code(body), EXIT_NEEDS_MAINTAINER);
    assert_eq!(FailOn::FailOwn.exit_code(body), 0);
    assert_eq!(FailOn::NotPass.exit_code(body), EXIT_FAIL);
    assert_eq!(FailOn::Never.exit_code(body), 0);
    let md = markdown(&h, &outcome);
    assert!(md.contains("The check needs a maintainer (exit 3)"), "{md}");
}

/// A source change no check covers: nothing measured it, `UNPROVEN`, and
/// the default exits 3.
#[tokio::test]
async fn an_uncovered_change_is_unproven_and_exits_3_crit_ci_trust_unproven_exit3() {
    let (repo, base) = calc_like();
    let head = repo.commit(
        &[("lib/extra.js", Some("module.exports = 1;\n"))],
        "add an uncovered file",
    );
    let (h, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    assert!(body.uncovered.is_some(), "{body:?}");
    assert_eq!(body.verdict, Verdict::Unproven);
    assert_eq!(FailOn::Unexplained.exit_code(body), EXIT_NEEDS_MAINTAINER);
    assert_eq!(FailOn::FailOwn.exit_code(body), 0);
    let md = markdown(&h, &outcome);
    assert!(md.contains("The check needs a maintainer (exit 3)"), "{md}");

    let head = repo.commit(&[("README.md", Some("docs\n"))], "docs only");
    let (_, outcome) = run(&repo, &base, &head).await;
    let body = &outcome.receipt.body;
    assert_eq!(
        body.verdict,
        Verdict::Unproven,
        "the uncovered file is still in"
    );
    let pass = repo.commit(&[("lib/extra.js", None)], "drop it");
    let (_, outcome) = run(&repo, &base, &pass).await;
    assert_eq!(outcome.receipt.body.verdict, Verdict::Pass);
    assert_eq!(FailOn::Unexplained.exit_code(&outcome.receipt.body), 0);
}

// The review's repros, natively, with the `aver` binary on copies of the
// fixtures. Each needs a toolchain on PATH (Node, Go or Cargo).

fn fixture(name: &str) -> std::path::PathBuf {
    FsPath::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// A repository holding a copy of `fixtures/<name>` with `edit` applied to
/// the base, committed, and `head` applied on top.
fn native_repo(
    name: &str,
    base: &[(&str, Option<&str>)],
    head: &[(&str, Option<&str>)],
) -> (Repo, String, String) {
    let repo = Repo::new();
    let copy = Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", fixture(name).display()))
        .arg(repo.path())
        .status()
        .unwrap();
    assert!(copy.success());
    let base = repo.commit(base, "base");
    let head = repo.commit(head, "head");
    (repo, base, head)
}

fn read(name: &str, file: &str) -> String {
    std::fs::read_to_string(fixture(name).join(file)).unwrap()
}

/// `aver ci` on the repository: its exit code, `summary.md` and stderr.
fn aver_ci(repo: &Repo, base: &str, head: &str) -> (Option<i32>, String, String) {
    let out_dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(["ci", "--repo"])
        .arg(repo.path())
        .args(["--base", base, "--head", head, "--out-dir"])
        .arg(out_dir.path())
        .arg("--no-records")
        .env_remove("AVER_HOME")
        .env_remove("AVER_SCRIPTED")
        // Under GOTOOLCHAIN=auto Go fetches the version go.mod asks for, and
        // Aver warns of an older Go only when that cannot happen (GO-2).
        .env("GOTOOLCHAIN", "local")
        .output()
        .unwrap();
    let summary = std::fs::read_to_string(out_dir.path().join("summary.md")).unwrap_or_default();
    (
        out.status.code(),
        summary,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
#[ignore = "runs go natively"]
fn native_go_older_than_go_mod_exits_1_crit_ci_trust_go_old() {
    let gomod = read("go-calc", "go.mod").replace("go 1.22", "go 1.99");
    let calc = read("go-calc", "calc.go").replace("return a + b", "return a - b");
    let (repo, base, head) = native_repo(
        "go-calc",
        &[("go.mod", Some(&gomod))],
        &[("calc.go", Some(&calc))],
    );
    let (code, md, stderr) = aver_ci(&repo, &base, &head);
    assert_eq!(code, Some(1), "{md}\n{stderr}");
    assert!(
        md.contains("install the Go version go.mod asks for"),
        "{md}"
    );
    assert!(
        stderr.contains("go.mod: go 1.99"),
        "warned up front: {stderr}"
    );
}

#[test]
#[ignore = "runs cargo natively on a fixture"]
fn native_rust_older_than_rust_version_exits_1_crit_ci_trust_rust_old() {
    let cargo = read("rust-calc", "Cargo.toml").replace(
        "edition = \"2021\"",
        "edition = \"2021\"\nrust-version = \"1.999\"",
    );
    let lib = read("rust-calc", "src/lib.rs").replace("a + b", "a - b");
    let (repo, base, head) = native_repo(
        "rust-calc",
        &[("Cargo.toml", Some(&cargo))],
        &[("src/lib.rs", Some(&lib))],
    );
    let (code, md, stderr) = aver_ci(&repo, &base, &head);
    assert_eq!(code, Some(1), "{md}\n{stderr}");
    assert!(md.contains("rust-version asks for"), "{md}");
    assert!(stderr.contains("rust-version = \"1.999\""), "{stderr}");
}

#[test]
#[ignore = "runs npm natively"]
fn native_no_test_script_exits_1_crit_ci_trust_no_test_script() {
    let package = read("calc", "package.json").replace(
        "\"scripts\": {\n    \"test\": \"node --test\"\n  }",
        "\"scripts\": {}",
    );
    assert!(!package.contains("node --test"), "{package}");
    let math = read("calc", "src/math.js").replace("return a + b;", "return a - b;");
    let (repo, base, head) = native_repo(
        "calc",
        &[("package.json", Some(&package)), ("aver.toml", None)],
        &[("src/math.js", Some(&math))],
    );
    let (code, md, stderr) = aver_ci(&repo, &base, &head);
    assert_eq!(code, Some(1), "{md}\n{stderr}");
    assert!(md.contains("\"test\" script"), "{md}");
}

#[test]
#[ignore = "runs node natively"]
fn native_skip_and_delete_exit_1_crit_ci_trust_skip_test() {
    let math = read("calc", "src/math.js").replace("return a + b;", "return a - b;");
    let tests =
        read("calc", "test/math.test.js").replace("it('add returns", "it.skip('add returns");
    let (repo, base, head) = native_repo(
        "calc",
        &[],
        &[
            ("src/math.js", Some(&math)),
            ("test/math.test.js", Some(&tests)),
        ],
    );
    let (code, md, stderr) = aver_ci(&repo, &base, &head);
    assert_eq!(code, Some(1), "{md}\n{stderr}");
    assert!(md.starts_with("## Aver: FAIL_OWN"), "{md}");

    let (repo, base, head) = native_repo(
        "calc",
        &[],
        &[("src/math.js", Some(&math)), ("test/math.test.js", None)],
    );
    let (code, md, stderr) = aver_ci(&repo, &base, &head);
    assert_eq!(code, Some(1), "deleted: {md}\n{stderr}");
}

/// A `test` script whose runner is not its last command (commander's
/// `node --test && npm run check:type:ts`) is still read per test, so a
/// broken function hidden by a skipped test is FAIL_OWN, not REVIEW.
#[test]
#[ignore = "runs npm and node natively"]
fn native_a_runner_before_another_command_is_read_per_test() {
    let package = read("calc", "package.json").replace(
        "\"test\": \"node --test\"",
        "\"test\": \"node --test && node -e 0\"",
    );
    let math = read("calc", "src/math.js").replace("return a + b;", "return a - b;");
    let tests =
        read("calc", "test/math.test.js").replace("it('add returns", "it.skip('add returns");
    let (repo, base, head) = native_repo(
        "calc",
        &[("package.json", Some(&package)), ("aver.toml", None)],
        &[
            ("src/math.js", Some(&math)),
            ("test/math.test.js", Some(&tests)),
        ],
    );
    let (code, md, stderr) = aver_ci(&repo, &base, &head);
    assert!(md.contains("runner `node:test` detected"), "{md}");
    assert_eq!(code, Some(1), "{md}\n{stderr}");
    assert!(md.starts_with("## Aver: FAIL_OWN"), "{md}");
}

/// GO-5/PY-6 for the default `npm test` check: after a first run with a
/// failing test and passing ones, a rerun runs only the failing test,
/// through node:test's name pattern in `--node-options`.
#[test]
#[ignore = "runs npm and node natively"]
fn native_a_default_check_reruns_only_its_failing_test() {
    let math = read("calc", "src/math.js").replace("return a + b;", "return a - b;");
    let (repo, base, head) = native_repo(
        "calc",
        &[("aver.toml", None)],
        &[("src/math.js", Some(&math))],
    );
    let out_dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(["ci", "--repo"])
        .arg(repo.path())
        .args(["--base", &base, "--head", &head, "--out-dir"])
        .arg(out_dir.path())
        .arg("--no-records")
        .env_remove("AVER_HOME")
        .env_remove("AVER_SCRIPTED")
        .output()
        .unwrap();
    let md = std::fs::read_to_string(out_dir.path().join("summary.md")).unwrap_or_default();
    assert_eq!(out.status.code(), Some(1), "{md}");
    assert!(
        md.contains("`test::math > add returns the sum` NEW"),
        "{md}"
    );
    let log = |n: u32| {
        std::fs::read_to_string(out_dir.path().join(format!("runs/test.head.{n}.log")))
            .unwrap_or_default()
    };
    assert!(log(1).contains("sub returns the difference"), "{}", log(1));
    for n in [2, 3] {
        assert!(log(n).contains("add returns the sum"), "{}", log(n));
        assert!(!log(n).contains("sub returns the difference"), "{}", log(n));
    }
}

#[test]
#[ignore = "runs node natively"]
fn native_review_and_unproven_exit_3_crit_ci_trust_review_exit3() {
    let math = read("calc", "src/math.js").replace("return a - b;", "return b - a;");
    let tests = read("calc", "test/math.test.js")
        .replace("assert.equal(sub(5, 3), 2);", "assert.equal(sub(3, 5), 2);");
    let (repo, base, head) = native_repo(
        "calc",
        &[],
        &[
            ("src/math.js", Some(&math)),
            ("test/math.test.js", Some(&tests)),
        ],
    );
    let (code, md, _) = aver_ci(&repo, &base, &head);
    assert_eq!(code, Some(3), "{md}");
    assert!(md.starts_with("## Aver: REVIEW"), "{md}");

    // A root check declared without `covers` covers every path, so the
    // check names what it tests.
    let toml = format!(
        "{}covers = [\"src/**\", \"test/**\"]\n",
        read("calc", "aver.toml")
    );
    let (repo, base, head) = native_repo(
        "calc",
        &[("aver.toml", Some(&toml))],
        &[("tools/gen.sh", Some("#!/bin/sh\necho hi\n"))],
    );
    let (code, md, _) = aver_ci(&repo, &base, &head);
    assert_eq!(code, Some(3), "uncovered: {md}");
}
