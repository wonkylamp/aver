//! What a head may do to the tests and the harness (ARCHITECTURE.md §2.5.2)
//! over the scripted sandbox: existing tests judge regressions from the
//! base's copies while the head's own versions prove criteria, dependency
//! changes install, frozen `package.json` fields fail, and the receipt's
//! `test_changes` reports all of it.

mod common;
mod scripted;
mod world;

use std::sync::Arc;

use aver::verify::Verified;
use aver_core::*;
use common::assert_matches_schema;
use scripted::Scripted;
use world::*;

/// One JUnit check `unit` that runs every `test/**/*.test.json` file, plus
/// `extra` checks.
fn config(extra: &str) -> String {
    format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n{extra}"
    )
}

struct Case {
    w: World,
    base: SnapshotId,
    task: TaskId,
    config: String,
}

fn case(sandbox: Arc<Scripted>, extra: &str, base: &[(&str, &str)], criteria: &[&str]) -> Case {
    let w = World::with_sandbox(sandbox);
    let config = config(extra);
    let mut files = vec![("aver.toml", config.as_str())];
    files.extend_from_slice(base);
    let base = w.snapshot(&files);
    let task = w.create_task(w.task_body(&w.admin, base, criteria));
    Case {
        w,
        base,
        task,
        config,
    }
}

impl Case {
    async fn verify(&self, head: &[(&str, &str)]) -> Verified {
        let mut files = vec![("aver.toml", self.config.as_str())];
        files.extend_from_slice(head);
        let head = self.w.snapshot(&files);
        let verified = self
            .w
            .verify(&self.w.admin, self.task, head, self.base)
            .await;
        assert_matches_schema("receipt", &verified.receipt);
        verified
    }
}

fn changes(v: &Verified) -> &TestChanges {
    v.receipt.body.test_changes.as_ref().unwrap()
}

fn pushback(v: &Verified) -> Option<String> {
    aver::run::report::pushback_line(&v.receipt.body, Some(&v.diagnostics))
}

fn names<T: SetItem + std::fmt::Display>(l: &Listed<T>) -> Vec<String> {
    l.items.iter().map(ToString::to_string).collect()
}

fn no_judging(receipt: &Receipt) {
    let judging: Vec<&str> = receipt
        .body
        .checks
        .iter()
        .map(|e| e.id.as_str())
        .filter(|id| id.starts_with("judging::"))
        .collect();
    assert!(judging.is_empty(), "{judging:?}");
}

/// A tagged case added to an existing test file is absent from the judged
/// head (which runs the base's copy) but runs in the head-tests tree, fails
/// on the transplant without the implementation, and proves its criterion.
#[tokio::test]
async fn a_case_added_to_an_existing_test_file_proves_its_criterion() {
    let c = case(
        Arc::default(),
        "",
        &[("test/math.test.json", r#"{"adds": "pass"}"#)],
        &["div"],
    );
    let v = c
        .verify(&[
            (
                "test/math.test.json",
                r#"{"adds": "pass", "[crit:div] divides": "pass-if:src/divide.js"}"#,
            ),
            ("src/divide.js", "module.exports = (a, b) => a / b;"),
        ])
        .await;
    let receipt = &v.receipt;
    no_judging(receipt);
    let div = entry(receipt, "unit::[crit:div] divides");
    assert_eq!(
        (div.base, div.head, div.cause),
        (BaseStatus::NotApplicable, HeadStatus::Pass, None)
    );
    assert_eq!(
        (div.transplant, div.head_tests),
        (Some(Transplant::Fail), Some(Transplant::Pass))
    );
    assert_eq!(
        div.runs.head, div.runs.head_tests,
        "its head result is the head-tests run"
    );
    let run: CheckRun = c.w.store.get(&div.runs.head_tests[0]).unwrap();
    assert_ne!(
        Some(&run.snapshot),
        receipt
            .body
            .checks
            .get("unit::adds")
            .map(|e| &e.runs.head[0])
            .map(|id| c.w.store.get::<CheckRun>(id).unwrap().snapshot)
            .as_ref(),
        "the head-tests tree is not the judged head"
    );
    assert_eq!(receipt.body.verdict, Verdict::Pass);
    assert_eq!(names(&changes(&v).files_modified), ["test/math.test.json"]);
    assert!(changes(&v).tests_removed.is_empty());
    assert_eq!(
        receipt.body.executor.as_deref(),
        Some("scripted-v0"),
        "the receipt names the executor"
    );

    let v = c
        .verify(&[(
            "test/math.test.json",
            r#"{"adds": "pass", "[crit:div] divides": "fail"}"#,
        )])
        .await;
    let div = entry(&v.receipt, "unit::[crit:div] divides");
    assert_eq!(
        (div.head, div.cause),
        (HeadStatus::Fail, Some(Cause::New)),
        "a case the head adds that fails on its own code is its failure"
    );
}

/// An existing test the head's version no longer has still runs from the
/// base's copy: a test that passed on the base and now fails is the
/// change's regression (`NEW`, `FAIL_OWN`), deleting it hides nothing, and
/// the report lists it. Before, a gone own version counted as a rewritten
/// expectation: `CHANGED`, `REVIEW`, and exit 0 in CI.
#[tokio::test]
async fn deleting_an_existing_failing_test_still_fails_from_the_base_copy() {
    let c = case(
        Arc::default(),
        "",
        &[
            (
                "test/math.test.json",
                r#"{"adds": "pass-if:src/add.js", "keeps": "pass"}"#,
            ),
            ("test/old.test.json", r#"{"old": "pass-if:src/add.js"}"#),
            ("src/add.js", "module.exports = (a, b) => a + b;"),
        ],
        &[],
    );
    let v = c
        .verify(&[("test/math.test.json", r#"{"keeps": "pass"}"#)])
        .await;
    let receipt = &v.receipt;
    no_judging(receipt);
    for id in ["unit::adds", "unit::old"] {
        let e = entry(receipt, id);
        assert_eq!(
            (e.base, e.head, e.cause),
            (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::New)),
            "{id}"
        );
        let failure = v
            .diagnostics
            .failures
            .iter()
            .find(|f| f.id == e.id)
            .unwrap();
        assert_eq!(
            failure.note.as_deref(),
            Some(aver::diagnostics::HIDDEN_REGRESSION_NOTE),
            "{id}"
        );
    }
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
    let changes = changes(&v);
    assert_eq!(names(&changes.files_modified), ["test/math.test.json"]);
    assert_eq!(names(&changes.files_deleted), ["test/old.test.json"]);
    assert_eq!(names(&changes.tests_removed), ["unit::adds", "unit::old"]);
    let lists = v.diagnostics.test_changes.as_ref().unwrap();
    assert_eq!(lists.tests_removed.items, ["unit::adds", "unit::old"]);
}

/// A head that skips an existing test is still judged by the base's copy:
/// a test the change breaks and then skips is `NEW` (`FAIL_OWN`, never
/// `REVIEW` or `PASS`), and the report lists the test as newly skipped.
#[tokio::test]
async fn skipping_an_existing_test_is_judged_by_the_base_copy_and_reported() {
    let c = case(
        Arc::default(),
        "",
        &[
            ("test/math.test.json", r#"{"adds": "pass-if:src/add.js"}"#),
            ("src/add.js", "x"),
        ],
        &[],
    );
    let v = c
        .verify(&[("test/math.test.json", r#"{"adds": "skip"}"#)])
        .await;
    let adds = entry(&v.receipt, "unit::adds");
    assert_eq!(
        (adds.head, adds.cause),
        (HeadStatus::Fail, Some(Cause::New))
    );
    assert_eq!(names(&changes(&v).tests_skipped), ["unit::adds"]);
    assert!(changes(&v).tests_removed.is_empty());
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn);
    assert_eq!(
        pushback(&v).as_deref(),
        Some(
            "You skipped `adds`, which fails on your change; un-skip it and fix the code it tests."
        )
    );
}

/// The pushback names what the head did to a test it removed: a test whose
/// file is gone is deleted, and one whose file is still there but no run
/// reports was deleted from it or no longer runs, which the reports do not
/// tell apart. (A skipped one is in the test above.)
#[tokio::test]
async fn the_pushback_tells_a_deleted_test_from_one_that_no_longer_runs() {
    let c = case(
        Arc::default(),
        "",
        &[
            (
                "test/math.test.json",
                r#"{"test/math.test.json > adds": "pass-if:src/add.js", "keeps": "pass"}"#,
            ),
            (
                "test/old.test.json",
                r#"{"test/old.test.json > old": "pass-if:src/add.js"}"#,
            ),
            ("src/add.js", "x"),
        ],
        &[],
    );
    let math = r#"{"test/math.test.json > adds": "pass-if:src/add.js", "keeps": "pass"}"#;
    let deleted = c
        .verify(&[("test/math.test.json", math), ("src/add.js", "x")])
        .await;
    assert_eq!(
        names(&changes(&deleted).files_deleted),
        ["test/old.test.json"]
    );
    assert_eq!(
        pushback(&deleted).as_deref(),
        Some("You deleted `test/old.test.json > old`, which passes on the original code; restore it, or a reviewer must accept the deletion.")
    );
    let gone = c
        .verify(&[
            ("test/math.test.json", r#"{"keeps": "pass"}"#),
            (
                "test/old.test.json",
                r#"{"test/old.test.json > old": "pass-if:src/add.js"}"#,
            ),
            ("src/add.js", "x"),
        ])
        .await;
    assert!(changes(&gone).files_deleted.is_empty());
    assert_eq!(
        pushback(&gone).as_deref(),
        Some("You deleted or no longer run `test/math.test.json > adds`, which passes on the original code; restore it, or a reviewer must accept the removal.")
    );
}

/// A head that rewrites an existing test's expectation, so the base's copy
/// fails on the new code and its own version passes, gets `CHANGED` and the
/// diagnostics say so; when its own version fails too, the test stays `NEW`.
#[tokio::test]
async fn a_rewritten_expectation_is_changed_and_a_failing_one_stays_new() {
    let base: &[(&str, &str)] = &[
        (
            "test/math.test.json",
            r#"{"adds": "pass-if:src/add.js", "keeps": "pass"}"#,
        ),
        ("src/add.js", "x"),
    ];
    let c = case(Arc::default(), "", base, &[]);
    let v = c
        .verify(&[
            (
                "test/math.test.json",
                r#"{"adds": "pass-if:src/sum.js", "keeps": "pass"}"#,
            ),
            ("src/sum.js", "x"),
        ])
        .await;
    let adds = entry(&v.receipt, "unit::adds");
    assert_eq!(
        (adds.head, adds.cause),
        (HeadStatus::Fail, Some(Cause::Changed))
    );
    assert_eq!(v.receipt.body.verdict, Verdict::Review);
    let failure = v
        .diagnostics
        .failures
        .iter()
        .find(|f| f.id == adds.id)
        .unwrap();
    assert_eq!(failure.own, Some(Transplant::Pass));
    assert_eq!(
        failure.note.as_deref(),
        Some(aver::diagnostics::CHANGED_NOTE)
    );

    let c = case(Arc::default(), "", base, &[]);
    let v = c
        .verify(&[(
            "test/math.test.json",
            r#"{"adds": "pass-if:src/sum.js", "keeps": "pass"}"#,
        )])
        .await;
    let adds = entry(&v.receipt, "unit::adds");
    assert_eq!(
        (adds.head, adds.cause),
        (HeadStatus::Fail, Some(Cause::New))
    );
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn);
    let failure = v
        .diagnostics
        .failures
        .iter()
        .find(|f| f.id == adds.id)
        .unwrap();
    assert_eq!(failure.own, Some(Transplant::Fail));
}

/// The reviewer's c2 scenario: both divide tests already fail on the base.
/// The head implements `divide` and rewrites the zero-division test so that
/// its own version passes. The base's copy of that test still fails on the
/// new code, so it was rewritten, not fixed: `CHANGED` (not `PREEXISTING`),
/// verdict `REVIEW` with a note saying so, and the summary's review section
/// names it. The honest head, which implements `divide` and leaves the test
/// alone (adding a case to its file), keeps it `PREEXISTING` and passes.
#[tokio::test]
async fn rewriting_an_already_failing_test_is_changed_not_fixed() {
    let base: &[(&str, &str)] = &[(
        "test/divide.test.json",
        r#"{"div-basic": "pass-if:src/divide.js", "div-zero": "pass-if:src/throws.js", "div-one": "pass"}"#,
    )];
    let c = case(Arc::default(), "", base, &[]);
    let v = c
        .verify(&[
            (
                "test/divide.test.json",
                r#"{"div-basic": "pass-if:src/divide.js", "div-zero": "pass-if:src/divide.js", "div-one": "pass"}"#,
            ),
            ("src/divide.js", "module.exports = (a, b) => a / b;"),
        ])
        .await;
    let basic = entry(&v.receipt, "unit::div-basic");
    assert_eq!(
        (basic.base, basic.head, basic.cause),
        (BaseStatus::Fail, HeadStatus::Pass, None),
        "the base's copy of div-basic passes on the head: fixed"
    );
    let zero = entry(&v.receipt, "unit::div-zero");
    assert_eq!(
        (zero.base, zero.head, zero.cause),
        (BaseStatus::Fail, HeadStatus::Fail, Some(Cause::Changed))
    );
    assert_eq!(v.receipt.body.verdict, Verdict::Review);
    let failure = v
        .diagnostics
        .failures
        .iter()
        .find(|f| f.id == zero.id)
        .unwrap();
    assert_eq!(failure.own, Some(Transplant::Pass));
    assert_eq!(
        failure.note.as_deref(),
        Some(aver::diagnostics::CHANGED_FAILING_NOTE)
    );
    let evidence = v.diagnostics.evidence.as_ref().unwrap();
    assert!(
        !evidence.fixed.items.iter().any(|t| t.contains("div-zero")),
        "{:?}",
        evidence.fixed
    );

    let c = case(Arc::default(), "", base, &[]);
    let v = c
        .verify(&[
            (
                "test/divide.test.json",
                r#"{"div-basic": "pass-if:src/divide.js", "div-zero": "pass-if:src/throws.js", "div-neg": "pass-if:src/divide.js", "div-one": "pass"}"#,
            ),
            ("src/divide.js", "module.exports = (a, b) => a / b;"),
        ])
        .await;
    let zero = entry(&v.receipt, "unit::div-zero");
    assert_eq!(zero.cause, Some(Cause::Preexisting));
    assert_eq!(v.receipt.body.verdict, Verdict::Pass);
}

/// A head that changes `scripts.test` gets `judging::package.json#scripts`,
/// `CHANGED`, for review,
/// and runs with the base's scripts; a dependency-only change is no judging
/// change.
#[tokio::test]
async fn changing_a_frozen_package_json_field_is_changed() {
    let c = case(
        Arc::default(),
        "\n[[check]]\nid = \"scripts\"\nrun = [\"contains\", \"package.json\", \"node --test\"]\n",
        &[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            (
                "package.json",
                r#"{"name": "calc", "scripts": {"test": "node --test"}}"#,
            ),
        ],
        &[],
    );
    let v = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            (
                "package.json",
                r#"{"name": "calc", "scripts": {"test": "exit 0"}, "devDependencies": {"x": "1"}}"#,
            ),
        ])
        .await;
    let receipt = &v.receipt;
    let scripts = entry(receipt, "judging::package.json#scripts");
    assert_eq!(scripts.cause, Some(Cause::Changed));
    assert!(scripts
        .detail
        .as_ref()
        .unwrap()
        .as_str()
        .starts_with("modified frozen field scripts of package.json"));
    assert!(receipt.body.checks.get("judging::package.json").is_none());
    assert_eq!(
        entry(receipt, "scripts").cause,
        None,
        "head runs read the base's scripts"
    );
    assert_eq!(receipt.body.verdict, Verdict::Review);
    assert_eq!(
        names(&changes(&v).harness_changed),
        ["judging::package.json#scripts"]
    );

    let v = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            (
                "package.json",
                r#"{"name": "calc", "version": "2.0.0", "scripts": {"test": "node --test"}, "devDependencies": {"x": "1"}}"#,
            ),
        ])
        .await;
    no_judging(&v.receipt);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass);

    let v = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            ("package.json", "not json"),
        ])
        .await;
    assert_eq!(
        entry(&v.receipt, "judging::package.json").cause,
        Some(Cause::Changed),
        "a manifest that does not parse is judged whole"
    );
}
