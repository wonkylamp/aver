//! Verdict tests that nothing else covers, run natively in slots,
//! `#[ignore]`d where they need a toolchain beyond the shell, or scripted
//! where the invariant is the judge's alone.

mod common;
mod scripted;
mod world;

use std::collections::BTreeMap;
use std::sync::Arc;

use aver::exec::{ExecConfig, Executor};
use aver::place::runner::{RunnerConfig, SlotRunner};
use aver::run::report::{agent_text, ReportInput};
use aver::verify::{Head, Request, Verified, Verifier};
use aver_core::*;
use world::*;

fn native(w: &World) -> Verifier {
    Verifier::new(
        w.store.clone(),
        w.workspaces.clone(),
        Executor::slots(
            w.store.clone(),
            SlotRunner::new(
                w.store.clone(),
                RunnerConfig::for_process(w.home.root(), ProvenanceMode::Local),
            ),
            ExecConfig::default(),
        ),
        Arc::new(SigningKey::from_bytes(&[7; 32])),
    )
}

async fn verify(
    w: &World,
    verifier: &Verifier,
    base: SnapshotId,
    head: SnapshotId,
    criteria: &[&str],
) -> Verified {
    let task = w.create_task(w.task_body(&w.admin, base, criteria));
    verifier
        .verify(
            w.admin.id(),
            Request {
                task,
                head: Head::Snapshot {
                    head,
                    base: Some(base),
                },
                attempt: None,
                fresh: false,
                all: false,
                progress: None,
                ci: None,
                adhoc: None,
                reason: None,
            },
        )
        .await
        .unwrap()
}

fn report(v: &Verified) -> String {
    agent_text(&ReportInput {
        receipt_id: &v.id,
        receipt: &v.receipt,
        diagnostics: Some(&v.diagnostics),
        sync: None,
        took: None,
        budget: None,
        notices: None,
        timing: None,
    })
}

fn named<'a>(v: &'a Verified, suffix: &str) -> &'a ReceiptCheck {
    v.receipt
        .body
        .checks
        .iter()
        .find(|e| e.id.as_str().ends_with(suffix))
        .unwrap_or_else(|| panic!("no entry ending {suffix:?}\n{}", report(v)))
}

/// A fixture's files, with `changes` written over them.
fn fixture(name: &str, changes: &[(&str, &str)]) -> BTreeMap<String, String> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy();
                out.insert(rel.into_owned(), std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let root = common::fixture(name);
    let mut files = BTreeMap::new();
    walk(&root, &root, &mut files);
    files.retain(|p, _| !p.contains("__pycache__"));
    for (p, c) in changes {
        files.insert((*p).to_owned(), (*c).to_owned());
    }
    files
}

fn snapshot(w: &World, contract: &str, files: &BTreeMap<String, String>) -> SnapshotId {
    let mut files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    files.retain(|(p, _)| *p != "aver.toml");
    w.snapshot_under(contract, &files)
}

const PYTEST: &str = "[[check]]\nid = \"py\"\nrun = \"PYTHONPATH=src python3 -m pytest -p no:cacheprovider --junitxml=$AVER_OUT/py.xml\"\nreport = \"junit\"\nmin_tests = 1\n";

/// `generic_docker.rs`, `python_detects_pytest_and_judges_test_by_test`:
/// pytest's JUnit report is read test by test, parameterized cases
/// included, so a broken function is one `NEW` entry and the rest stay
/// clean; a tagged test of the new behaviour proves its criterion.
#[tokio::test]
#[ignore = "runs pytest natively: needs Python 3 and pytest on PATH"]
async fn pytest_is_judged_test_by_test() {
    let w = World::new();
    let verifier = native(&w);
    let base_files = fixture("py-calc", &[]);
    let base = snapshot(&w, PYTEST, &base_files);
    let ops = &base_files["src/calc/ops.py"];

    let broken = fixture(
        "py-calc",
        &[(
            "src/calc/ops.py",
            &ops.replace("return a - b", "return a + b"),
        )],
    );
    let v = verify(&w, &verifier, base, snapshot(&w, PYTEST, &broken), &[]).await;
    let text = report(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn, "{text}");
    let new: Vec<&str> = v
        .receipt
        .body
        .checks
        .iter()
        .filter(|e| e.cause == Some(Cause::New))
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(new.len(), 1, "{text}");
    assert!(
        new[0].ends_with("test_sub_returns_the_difference"),
        "{text}"
    );
    let params = v
        .receipt
        .body
        .checks
        .iter()
        .filter(|e| e.id.as_str().contains("test_mul_returns_the_product["))
        .count();
    assert_eq!(params, 2, "{text}");

    let fixed = fixture(
        "py-calc",
        &[
            (
                "src/calc/ops.py",
                &ops.replace("raise NotImplementedError(\"divide\")", "return a / b"),
            ),
            (
                "tests/test_div.py",
                "from calc.ops import divide\n\n\ndef test_divide_halves_crit_div():\n    assert divide(6, 3) == 2\n",
            ),
        ],
    );
    let v = verify(&w, &verifier, base, snapshot(&w, PYTEST, &fixed), &["div"]).await;
    let text = report(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{text}");
    assert_eq!(
        named(&v, "test_divide_halves_crit_div").transplant,
        Some(Transplant::Fail),
        "{text}"
    );
}

/// `test_changes.rs`, `adding_a_dependency_and_code_that_uses_it_passes`:
/// the judged head runs with the head's lockfile, the base and the
/// transplant with the base's, so a test of code using a dependency the
/// head adds passes on the head, fails on the transplant, and proves.
#[tokio::test]
async fn a_dependency_the_head_adds_is_on_its_side_only() {
    let w = World::new();
    let toml = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\nlockfile = \"package-lock.json\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n"
    );
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("package-lock.json", r#"{"packages": {}}"#),
        ("test/math.test.json", r#"{"adds": "pass"}"#),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "package-lock.json",
            r#"{"packages": {"node_modules/left-pad": {}}}"#,
        ),
        ("test/math.test.json", r#"{"adds": "pass"}"#),
        ("src/pad.js", "require('left-pad')"),
        (
            "test/pad.test.json",
            r#"{"[crit:pad] pads": "contains:package-lock.json:left-pad"}"#,
        ),
    ]);
    let v = verify(&w, &w.verifier, base, head, &["pad"]).await;
    let text = report(&v);
    let pads = named(&v, "unit::[crit:pad] pads");
    assert_eq!(
        (pads.head, pads.transplant),
        (HeadStatus::Pass, Some(Transplant::Fail)),
        "{text}"
    );
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{text}");
}

/// `verify_docker.rs`, `a_test_keyed_to_the_run_index_is_stable_on_both_sides`:
/// no rerun index reaches the code under test (`AVER_RUN`, the reaping
/// tag, is new for every run and names none), so a check that fails when
/// one does passes on both sides, natively. The CI job's own
/// `GITHUB_RUN_ATTEMPT` is the same on both sides, so it is left out.
#[tokio::test]
async fn no_run_index_reaches_the_code_under_test() {
    let w = World::new();
    let toml = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"index\"\nrun = [\"sh\", \"-c\", \"! env | grep -i -e attempt -e run_index | grep -v ^GITHUB_\"]\n"
    );
    let base = w.snapshot(&[("aver.toml", &toml), ("a", "1")]);
    let head = w.snapshot(&[("aver.toml", &toml), ("a", "2")]);
    let v = verify(&w, &native(&w), base, head, &[]).await;
    let text = report(&v);
    let index = named(&v, "index");
    assert_eq!(
        (index.base, index.head, index.cause),
        (BaseStatus::Pass, HeadStatus::Pass, None),
        "{text}"
    );
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{text}");
}

/// `verify_docker.rs`, `a_check_that_exceeds_its_timeout_is_env_and_blocked_not_own`:
/// natively, a check past its timeout is `ENV`, so the verdict is
/// `BLOCKED_NOT_OWN`, never the change's failure.
#[tokio::test]
async fn a_check_past_its_timeout_is_env_and_blocked_not_own() {
    let w = World::new();
    let toml = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[limits]\ntimeout_s = 1\n\n[[check]]\nid = \"slow\"\nrun = [\"sh\", \"-c\", \"sleep 30\"]\n"
    );
    let base = w.snapshot(&[("aver.toml", &toml), ("a", "1")]);
    let head = w.snapshot(&[("aver.toml", &toml), ("a", "2")]);
    let started = std::time::Instant::now();
    let v = verify(&w, &native(&w), base, head, &[]).await;
    let text = report(&v);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(25),
        "the timeout did not stop the run"
    );
    assert_eq!(named(&v, "slow").cause, Some(Cause::Env), "{text}");
    assert_eq!(v.receipt.body.verdict, Verdict::BlockedNotOwn, "{text}");
}

const NODE_TEST: &str = "[[check]]\nid = \"unit\"\nrun = \"node --test --test-reporter=spec --test-reporter-destination=stdout --test-reporter=junit --test-reporter-destination=$AVER_OUT/unit.xml\"\nreport = \"junit\"\nmin_tests = 1\n";

/// `ci_docker.rs`, `a_regression_only_pr_that_breaks_a_test_fails_own_with_exit_1`:
/// natively, the `NEW` failure keeps the test file it came from in its
/// diagnostics' `location`.
#[tokio::test]
#[ignore = "runs node natively"]
async fn a_new_failure_keeps_its_test_files_location() {
    let w = World::new();
    let base_files = fixture("calc", &[]);
    let base = snapshot(&w, NODE_TEST, &base_files);
    let math = base_files["src/math.js"].replace("return a + b;", "return a - b;");
    let head = snapshot(&w, NODE_TEST, &fixture("calc", &[("src/math.js", &math)]));
    let v = verify(&w, &native(&w), base, head, &[]).await;
    let text = report(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn, "{text}");
    let add = named(&v, "add returns the sum");
    assert_eq!(add.cause, Some(Cause::New), "{text}");
    let failure = v
        .diagnostics
        .failures
        .iter()
        .find(|f| f.id == add.id)
        .unwrap();
    assert_eq!(
        failure.location.as_ref().map(|l| l.file.as_str()),
        Some("test/math.test.js"),
        "{failure:?}"
    );
}

/// `ci_docker.rs`, `existing_test_files_prove_with_the_heads_cases_and_judge_with_the_bases`:
/// natively, a tagged case added to an existing test file proves its
/// criterion through the head-tests run while the file's other tests are
/// judged by the base's copy.
#[tokio::test]
#[ignore = "runs node natively"]
async fn a_case_added_to_an_existing_test_file_proves_natively() {
    let w = World::new();
    let base_files = fixture("calc", &[]);
    let base = snapshot(&w, NODE_TEST, &base_files);
    let math =
        base_files["src/math.js"].replace("throw new Error('not implemented');", "return a / b;");
    let tests = base_files["test/math.test.js"]
        .replace(
            "const { add, sub, mul } = require('../src/math');",
            "const { add, sub, mul, divide } = require('../src/math');",
        )
        .replace(
            "  it('mul returns the product', () => {",
            "  it('[crit:div] divide returns the quotient', () => {\n    assert.equal(divide(6, 3), 2);\n  });\n\n  it('mul returns the product', () => {",
        );
    let head = snapshot(
        &w,
        NODE_TEST,
        &fixture(
            "calc",
            &[("src/math.js", &math), ("test/math.test.js", &tests)],
        ),
    );
    let v = verify(&w, &native(&w), base, head, &["div"]).await;
    let text = report(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{text}");
    let div = named(&v, "[crit:div] divide returns the quotient");
    assert_eq!(div.transplant, Some(Transplant::Fail), "{text}");
}
