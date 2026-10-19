//! The transplant proof rule and diagnostics (ARCHITECTURE.md §1.9.3,
//! §1.9.4) over the scripted sandbox: a test proves a criterion only if it
//! fails on the base plus the head's new test files, and every unproven
//! criterion says why.

mod common;
mod scripted;
mod world;

use aver::diagnostics::Diagnostics;
use aver::verify::{Head, Request, Verified};
use aver_core::rules::UnprovenReason;
use aver_core::*;
use common::{assert_matches_schema, assert_value_matches_schema};
use world::*;

/// One JUnit check `unit` that runs every `test/**/*.test.json` file.
fn config() -> String {
    format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n"
    )
}

struct Case {
    w: World,
    base: SnapshotId,
    task: TaskId,
}

/// A base with one passing test, and a task with criteria `div` (divide
/// works) and `docs` (README mentions divide).
fn case() -> Case {
    let w = World::new();
    let base = w.snapshot(&[
        ("aver.toml", &config()),
        ("test/math.test.json", r#"{"adds": "pass"}"#),
    ]);
    let mut body = w.task_body(&w.admin, base, &["div", "docs"]);
    body.criteria = Criteria::new(vec![
        Criterion {
            id: "div".parse().unwrap(),
            text: "divide returns the quotient".into(),
        },
        Criterion {
            id: "docs".parse().unwrap(),
            text: "README mentions divide".into(),
        },
    ])
    .unwrap();
    let task = w.create_task(body);
    Case { w, base, task }
}

impl Case {
    async fn verify(&self, files: &[(&str, &str)]) -> Verified {
        let config = config();
        let mut all = vec![("aver.toml", config.as_str())];
        all.extend_from_slice(files);
        let head = self.w.snapshot(&all);
        self.w
            .verify(&self.w.admin, self.task, head, self.base)
            .await
    }
}

fn reason(d: &Diagnostics, id: &str) -> UnprovenReason {
    d.unproven
        .iter()
        .find(|u| u.criterion.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is not unproven: {d:?}"))
        .reason
}

/// A new test that cannot pass without the head's implementation proves
/// its criterion; one that passes on the transplant proves nothing.
#[tokio::test]
async fn a_test_proves_only_if_it_fails_without_the_change() {
    let c = case();
    let verified = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            ("src/divide.js", "module.exports = (a, b) => a / b;"),
            (
                "test/divide.test.json",
                r#"{"[crit:div] divides": "pass-if:src/divide.js", "[crit:docs] mentions": "pass"}"#,
            ),
        ])
        .await;
    let receipt = &verified.receipt;
    assert_eq!(receipt.body.schema, ReceiptSchema::V3);
    assert_matches_schema("receipt", receipt);
    let div = entry(receipt, "unit::[crit:div] divides");
    assert_eq!(
        (div.head, div.cause, div.transplant),
        (HeadStatus::Pass, None, Some(Transplant::Fail))
    );
    assert_eq!(
        div.runs.transplant.len(),
        1,
        "an expected failure on the transplant is not rerun"
    );
    let docs = entry(receipt, "unit::[crit:docs] mentions");
    assert_eq!(docs.transplant, Some(Transplant::Pass));
    assert_eq!(
        entry(receipt, "unit::adds").transplant,
        None,
        "proves nothing, so no result"
    );
    assert_eq!(
        receipt.body.unproven,
        ["docs".parse::<CriterionId>().unwrap()]
    );
    assert_eq!(receipt.body.verdict, Verdict::Unproven);
    assert_eq!(
        reason(&verified.diagnostics, "docs"),
        UnprovenReason::PassesWithoutChange
    );
    let transplant: CheckRun = c.w.store.get(&div.runs.transplant[0]).unwrap();
    let snapshot: Snapshot = c.w.store.get(&transplant.snapshot).unwrap();
    let base: Snapshot = c.w.store.get(&c.base).unwrap();
    assert_eq!(
        snapshot.env, base.env,
        "the transplant runs in the base environment"
    );
    let root_has = |path: &str| {
        c.w.workspaces
            .file_in_snapshot(&transplant.snapshot, &path.parse().unwrap())
            .unwrap()
            .is_some()
    };
    assert!(root_has("test/divide.test.json") && !root_has("src/divide.js"));
}

/// A test that is absent from the transplant's report (its file cannot
/// load without the head's implementation) counts as failing there, so its
/// entry proves by §1.9.3's rule; but that is a weak proof, which alone
/// never proves (§1.9.4), so the criterion is unproven as `weak_proof`. A
/// tag in a test file that no runner reads proves nothing and is
/// `claimed_test_not_found`.
#[tokio::test]
async fn a_test_absent_from_the_transplant_proves() {
    let c = case();
    let verified = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            ("src/divide.js", "x"),
            (
                "test/divide.test.json",
                r#"{"[crit:div] divides": "only-if:src/divide.js"}"#,
            ),
            ("lib/docs.test.json", r#"{"[crit:docs] mentions": "pass"}"#),
        ])
        .await;
    let receipt = &verified.receipt;
    let div = entry(receipt, "unit::[crit:div] divides");
    assert_eq!(
        (div.base, div.transplant),
        (BaseStatus::NotApplicable, Some(Transplant::Absent))
    );
    assert!(aver_core::rules::proves(div, ProofRule::Transplant));
    assert_eq!(
        reason(&verified.diagnostics, "div"),
        aver_core::rules::UnprovenReason::WeakProof
    );
    assert!(receipt
        .body
        .checks
        .get("unit::[crit:docs] mentions")
        .is_none());
    assert_eq!(
        reason(&verified.diagnostics, "docs"),
        UnprovenReason::ClaimedTestNotFound
    );
}

/// A proof that fails on the original code only because a name the change
/// adds is missing there is weak (ARCHITECTURE.md §1.9.4), whatever words the
/// compiler uses; one that names the added type in an assertion is not.
#[tokio::test]
async fn a_proof_failing_only_on_a_name_the_change_adds_is_weak() {
    let c = case();
    let verified = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            ("src/ratio.rs", "pub struct Ratio(i32, i32);\n"),
            (
                "test/divide.test.json",
                r#"{"[crit:div] divides": "say-if:src/ratio.rs:error[E0412]: cannot find type `Ratio` in this scope", "[crit:docs] halves": "say-if:src/ratio.rs:assertion failed: Ratio(1, 2).half() == 0.5"}"#,
            ),
        ])
        .await;
    assert_eq!(
        reason(&verified.diagnostics, "div"),
        UnprovenReason::WeakProof
    );
    let weak = &verified.diagnostics.weak_proofs;
    assert_eq!(weak.len(), 1, "{weak:?}");
    assert_eq!(weak[0].name.as_deref(), Some("Ratio"));
    assert!(!verified
        .receipt
        .body
        .unproven
        .iter()
        .any(|u| u.as_str() == "docs"));
    let json = serde_json::to_value(&verified.diagnostics).unwrap();
    assert_value_matches_schema("diagnostics", &json);
    let text = aver::run::report::agent_text(&aver::run::report::ReportInput {
        receipt_id: &verified.id,
        receipt: &verified.receipt,
        diagnostics: Some(&verified.diagnostics),
        sync: None,
        took: None,
        budget: None,
        notices: None,
        timing: None,
    });
    let pushback = text.lines().nth(2).unwrap_or_default();
    assert_eq!(
        pushback,
        "Your test `[crit:div] divides` fails on the original code only because `Ratio`, which your change adds, is missing there; make it fail there on a result the original code gets wrong.",
        "{text}"
    );
}

/// A check that reports only its exit code names no tests, so a tagged
/// file it runs is not a test that went missing: the criterion is
/// `no_test_names`, never `claimed_test_not_found`.
#[tokio::test]
async fn an_exit_only_check_never_reports_a_tagged_test_not_found() {
    let w = World::new();
    let config = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\n"
    );
    let base = w.snapshot(&[
        ("aver.toml", &config),
        ("test/math.test.json", r#"{"adds": "pass"}"#),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &["div"]));
    let head = w.snapshot(&[
        ("aver.toml", &config),
        ("test/math.test.json", r#"{"adds": "pass"}"#),
        ("test/divide.test.json", r#"{"[crit:div] divides": "pass"}"#),
    ]);
    let verified = w.verify(&w.admin, task, head, base).await;
    assert_eq!(
        reason(&verified.diagnostics, "div"),
        UnprovenReason::NoTestNames
    );
}

/// A transplant that ends in infra cannot show the test needs the change:
/// the entry is ENV and the verdict BLOCKED_NOT_OWN. Causes still compare the
/// head with the plain base.
#[tokio::test]
async fn a_transplant_that_ends_in_infra_blocks() {
    let c = case();
    let verified = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            ("src/divide.js", "x"),
            (
                "test/divide.test.json",
                r#"{"[crit:div] divides": "pass", "hang": "timeout-unless:src/divide.js", "[crit:docs] mentions": "pass-if:src/divide.js"}"#,
            ),
        ])
        .await;
    let receipt = &verified.receipt;
    let div = entry(receipt, "unit::[crit:div] divides");
    assert_eq!(div.transplant, Some(Transplant::Infra));
    assert_eq!(div.cause, Some(Cause::Env));
    assert_eq!(
        div.base,
        BaseStatus::NotApplicable,
        "compared with the base, not the transplant"
    );
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
    assert_eq!(
        reason(&verified.diagnostics, "div"),
        UnprovenReason::Blocked
    );
}

/// Changing a frozen `package.json` field is a judging change: no transplant
/// runs, nothing is proven, and diagnostics say why.
#[tokio::test]
async fn a_judging_change_proves_nothing() {
    let c = case();
    let verified = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            (
                "test/divide.test.json",
                r#"{"[crit:div] divides": "pass-if:src/divide.js"}"#,
            ),
            ("src/divide.js", "x"),
            ("package.json", r#"{"scripts": {"test": "exit 0"}}"#),
        ])
        .await;
    let receipt = &verified.receipt;
    assert_eq!(
        entry(receipt, "judging::package.json#scripts").cause,
        Some(Cause::Changed)
    );
    assert!(receipt.body.checks.iter().all(|e| e.transplant.is_none()));
    assert_eq!(receipt.body.verdict, Verdict::Unproven);
    assert_eq!(
        reason(&verified.diagnostics, "div"),
        UnprovenReason::JudgingFilesChanged
    );
}

/// Unclaimed criteria list the closest test names; a criterion whose tag is
/// in a test file but in no report is `claimed_test_not_found`; a failing
/// prover is `fails_at_head`. Failures carry location and output.
#[tokio::test]
async fn diagnostics_explain_every_failure_and_unproven_criterion() {
    let c = case();
    let verified = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            (
                "test/divide.test.json",
                r#"{"divide returns the quotient": "pass", "[crit:div] divides": "fail", "mentions [crit:docs]": "skip"}"#,
            ),
        ])
        .await;
    let d = &verified.diagnostics;
    assert_eq!(d.schema, "aver.diagnostics/v0");
    assert_eq!(d.receipt, verified.id);
    assert_eq!(reason(d, "div"), UnprovenReason::FailsAtHead);
    let div = d
        .unproven
        .iter()
        .find(|u| u.criterion.as_str() == "div")
        .unwrap();
    assert_eq!(div.entries.len(), 1);
    assert!(div.closest.is_empty());
    let docs = d
        .unproven
        .iter()
        .find(|u| u.criterion.as_str() == "docs")
        .unwrap();
    assert_eq!(docs.reason, UnprovenReason::ClaimedTestNotFound);
    assert_eq!(docs.closest.len(), 3);
    let failure = d
        .failures
        .iter()
        .find(|f| f.id.as_str() == "unit::[crit:div] divides")
        .unwrap();
    assert_eq!(failure.cause, Some(Cause::New));
    assert_eq!(
        failure.message.as_deref(),
        Some("[crit:div] divides failed")
    );
    assert!(failure
        .output
        .as_deref()
        .unwrap()
        .contains("ran the test files"));

    let stored = aver::diagnostics::load(&c.w.store, &verified.id).unwrap();
    assert_eq!(stored.as_ref(), Some(d));
    let json = serde_json::to_value(d).unwrap();
    assert_eq!(json["unproven"][0]["reason"], "fails_at_head");
    assert_value_matches_schema("diagnostics", &json);

    let verified = c
        .verify(&[
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            (
                "test/divide.test.json",
                r#"{"divide returns the quotient": "pass", "quotient of zero": "pass"}"#,
            ),
        ])
        .await;
    let div = verified
        .diagnostics
        .unproven
        .iter()
        .find(|u| u.criterion.as_str() == "div")
        .unwrap();
    assert_eq!(div.reason, UnprovenReason::NotClaimed);
    assert_eq!(div.closest[0], "divide returns the quotient");
}

/// A task check proves through the transplant too: its command must fail
/// on the base plus the head's new test files.
#[tokio::test]
async fn task_checks_prove_through_the_transplant() {
    let w = World::new();
    let base = w.snapshot(&[
        ("aver.toml", &config()),
        ("test/math.test.json", r#"{"adds": "pass"}"#),
    ]);
    let mut body = w.task_body(&w.admin, base, &["done"]);
    body.checks = Set::new(vec![TaskCheckDef {
        id: "accept".parse().unwrap(),
        argv: vec!["exit-if".into(), "TODO".into()],
        cwd: None,
        timeout_ms: None,
        report: None,
        proves: Set::new(vec!["done".parse().unwrap()]).unwrap(),
    }])
    .unwrap();
    let task = w.create_task(body);
    let with_todo = w.snapshot(&[
        ("aver.toml", &config()),
        ("test/math.test.json", r#"{"adds": "pass"}"#),
        ("TODO", "x"),
    ]);
    let receipt = w.verify(&w.admin, task, base, with_todo).await.receipt;
    let accept = entry(&receipt, "accept");
    assert_eq!(accept.transplant, Some(Transplant::Fail));
    assert_eq!(receipt.body.verdict, Verdict::Pass);

    let receipt = w
        .verifier
        .verify(
            w.admin.id(),
            Request {
                task,
                head: Head::Snapshot {
                    head: base,
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
        .receipt;
    assert_eq!(entry(&receipt, "accept").transplant, Some(Transplant::Pass));
    assert_eq!(receipt.body.verdict, Verdict::Unproven);
}

/// The transplant's snapshot carries the base environment, so its runs are
/// keyed by the base and the head's test files alone: a later head with the
/// same test files but another implementation and another lockfile reads
/// them from the cache instead of running them again.
#[tokio::test]
async fn the_transplant_is_reused_while_the_test_files_stay_the_same() {
    let w = World::new();
    let base = w.snapshot(&[
        ("aver.toml", &config()),
        ("test/math.test.json", r#"{"adds": "pass"}"#),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &["div"]));
    let test = r#"{"[crit:div] divides": "pass-if:src/divide.js"}"#;
    let mut transplants = Vec::new();
    for (code, lockfile) in [("one", r#"{"v": 1}"#), ("two", r#"{"v": 2}"#)] {
        let head = w.snapshot(&[
            ("aver.toml", &config()),
            ("test/math.test.json", r#"{"adds": "pass"}"#),
            ("test/divide.test.json", test),
            ("src/divide.js", code),
            ("package-lock.json", lockfile),
        ]);
        let receipt = w.verify(&w.admin, task, head, base).await.receipt;
        assert_eq!(receipt.body.verdict, Verdict::Pass);
        let div = entry(&receipt, "unit::[crit:div] divides");
        transplants.push(div.runs.transplant.clone());
    }
    assert_eq!(transplants[0].len(), 1);
    assert_eq!(transplants[0], transplants[1]);
}
