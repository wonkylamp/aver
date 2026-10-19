//! Zero evidence is never PASS (REDESIGN.md §4.1, ARCHITECTURE.md §1.9.3):
//! a check whose base or judged head ran no test, whose base passed no
//! test, whose pytest stopped on its own error, whose Go package a panic
//! cut off, or whose setup command failed gets an `ENV` entry
//! `<check>::(evidence) <reason>`, so the verdict is BLOCKED_NOT_OWN. So
//! is a base below the contract's `min_tests`, and an exit-only check no
//! person approved; an approved exit-only check is not blocked for running
//! no test, and a type check is held to no floor.

mod common;
mod scripted;
mod world;

use std::sync::Arc;

use aver::exec::{ExecConfig, Executor};
use aver::place::runner::{RunnerConfig, SlotRunner};
use aver::verify::{Head, Request, Verifier};
use aver_core::*;
use world::*;

fn detail(entry: &ReceiptCheck) -> &str {
    entry.detail.as_ref().map_or("", |d| d.as_str())
}

fn blockers(receipt: &Receipt) -> Vec<&str> {
    receipt
        .body
        .checks
        .iter()
        .map(|e| e.id.as_str())
        .filter(|id| id.contains("::(evidence) "))
        .collect()
}

fn config(check: &str) -> String {
    format!("[env]\nimage = \"{PINNED_IMAGE}\"\n\n{check}")
}

/// B1: both sides exit 0 having run no test.
#[tokio::test]
async fn both_sides_running_no_test_is_blocked_crit_b1() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", "{}")]);
    let head = w.snapshot(&[("aver.toml", &toml), ("tests.json", "{}"), ("lib.js", "x")]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
    let on_base = entry(&receipt, "unit::(evidence) no tests on the base");
    assert_eq!(on_base.cause, Some(Cause::Env));
    assert!(
        detail(on_base).starts_with("The base and your version both ran 0 tests."),
        "{}",
        detail(on_base)
    );
    let on_head = entry(&receipt, "unit::(evidence) no tests on your version");
    assert_eq!(on_head.cause, Some(Cause::Env));
}

/// A base whose every test fails cannot show what the change does.
#[tokio::test]
async fn a_base_with_no_passing_test_is_blocked_but_one_passing_test_is_enough() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", r#"{"a": "fail"}"#)]);
    let head = w.snapshot(&[("aver.toml", &toml), ("tests.json", r#"{"a": "pass"}"#)]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
    assert_eq!(
        blockers(&receipt),
        ["unit::(evidence) no test passed on the base"]
    );
    let red = entry(&receipt, "unit::(evidence) no test passed on the base");
    assert_eq!(red.cause, Some(Cause::Env));
    assert!(
        detail(red).contains("The base ran 1 test (0 passed, 1 failed)"),
        "{}",
        detail(red)
    );

    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"a": "fail", "b": "pass"}"#),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"a": "pass", "b": "pass"}"#),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert!(blockers(&receipt).is_empty(), "{:?}", receipt.body.checks);
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

/// pytest's exit 4 (a usage or collection error) stopped the runner, even
/// though the report it left lists passing tests.
#[tokio::test]
async fn a_pytest_runner_error_with_a_partial_report_is_blocked() {
    let w = World::new();
    let toml = config(
        "[[check]]\nid = \"py\"\nrun = [\"junit-exit\", \"py.xml\", \"tests.json\", \"code\"]\nreport = { format = \"pytest-junit\", path = \"/aver/out/py.xml\" }\n",
    );
    let tests = r#"{"test_a": "pass"}"#;
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", tests), ("code", "4")]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("code", "4"),
        ("lib.py", "x"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
    let error = entry(&receipt, "py::(evidence) runner error on the base");
    assert_eq!(error.cause, Some(Cause::Env));
    assert!(detail(error).contains("(exit 4)"), "{}", detail(error));
    assert!(blockers(&receipt).contains(&"py::(evidence) runner error on your version"));
}

/// A setup command that fails on the base blocks the verdict.
#[tokio::test]
async fn a_failed_setup_command_is_blocked() {
    let w = World::new();
    let toml = unit_toml("setup = [[\"findings\", \"vet.out\"]]\n");
    let vet = "calc.go:3:1: unreachable code\n";
    let tests = r#"{"adds": "pass"}"#;
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("vet.out", vet),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("vet.out", vet),
        ("calc.go", "x"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
    let failed = entry(&receipt, "unit::(evidence) setup failed on the base");
    assert_eq!(failed.cause, Some(Cause::Env));
    assert!(
        detail(failed).contains("The prepare command of check"),
        "{}",
        detail(failed)
    );
}

/// A setup command a shell cannot find names the line and what to install,
/// not only that setup failed.
#[tokio::test]
async fn a_setup_command_that_is_not_installed_is_named() {
    let w = World::new();
    let toml = unit_toml("setup = [[\"findings\", \"prepare.out\"]]\n");
    let out = "> pkg@1.0.0 prepare\n> del-cli dist\n\nsh: del-cli: command not found\n";
    let tests = r#"{"adds": "pass"}"#;
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("prepare.out", out),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("prepare.out", out),
        ("lib.js", "x"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
    let failed = entry(&receipt, "unit::(evidence) setup failed on the base");
    assert_eq!(failed.cause, Some(Cause::Env));
    let text = detail(failed);
    assert!(text.contains("sh: del-cli: command not found"), "{text}");
    assert!(text.contains("dependencies are not"), "{text}");
}

/// An exit-only check reports no test by design: running none blocks
/// nothing when the base's `aver.toml` names it.
#[tokio::test]
async fn an_exit_only_check_is_not_blocked_for_running_no_test() {
    let w = World::new();
    let toml = config("[[check]]\nid = \"lint\"\nrun = [\"exit\", \"0\"]\n");
    let base = w.snapshot(&[("aver.toml", &toml)]);
    let head = w.snapshot(&[("aver.toml", &toml), ("lib.js", "x")]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert!(blockers(&receipt).is_empty(), "{:?}", receipt.body.checks);
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

const GO_MOD: &str = "module example.com/calc\n\ngo 1.21\n";
const GO_CALC: &str = "package calc\n\nfunc Add(a, b int) int { return a + b }\n";
const GO_TESTS: &str = "package calc\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tif Add(2, 3) != 5 {\n\t\tt.Fatal(\"Add\")\n\t}\n}\n\nfunc TestPanics(t *testing.T) {\n\tvar m map[string]int\n\tm[\"x\"] = 1\n}\n\nfunc TestZero(t *testing.T) {\n\tif Add(0, 0) != 0 {\n\t\tt.Fatal(\"zero\")\n\t}\n}\n";

/// A panic cuts the package's test binary off on both sides, so `TestZero`
/// never reports on either: without the declared tests to compare, the
/// change would pass having run less than the base declares.
#[tokio::test]
#[ignore = "runs go natively"]
async fn a_go_package_cut_off_by_a_panic_is_blocked() {
    let w = World::new();
    let base = w.snapshot_under(
        GO_CONTRACT,
        &[
            ("go.mod", GO_MOD),
            ("calc.go", GO_CALC),
            ("calc_test.go", GO_TESTS),
        ],
    );
    let head = w.snapshot_under(
        GO_CONTRACT,
        &[
            ("go.mod", GO_MOD),
            (
                "calc.go",
                &format!("{GO_CALC}\nfunc Sub(a, b int) int {{ return a - b }}\n"),
            ),
            ("calc_test.go", GO_TESTS),
        ],
    );
    let verifier = Verifier::new(
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
    );
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = verifier
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
        .receipt;
    assert_eq!(
        receipt.body.verdict,
        Verdict::BlockedNotOwn,
        "{:?}",
        receipt.body.checks
    );
    let cut = entry(
        &receipt,
        "go::(evidence) example.com/calc cut off on the base",
    );
    assert_eq!(cut.cause, Some(Cause::Env));
    assert!(
        detail(cut).contains("with 1 test it declares never reported"),
        "{}",
        detail(cut)
    );
}

/// A contract check reading `tests.json` as JUnit, with `min_tests`.
fn floored(min_tests: u32) -> String {
    format!("[[check]]\nid = \"unit\"\nrun = \"junit unit.xml tests.json\"\nreport = \"junit\"\nmin_tests = {min_tests}\n")
}

fn passing(n: usize) -> String {
    let tests: Vec<String> = (0..n).map(|i| format!("\"t{i}\": \"pass\"")).collect();
    format!("{{{}}}", tests.join(", "))
}

fn verdict_of(
    w: &World,
    base: SnapshotId,
    head: SnapshotId,
) -> impl std::future::Future<Output = Receipt> + '_ {
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    async move { w.verify(&w.admin, task, head, base).await.receipt }
}

/// The contract's `min_tests` is the floor: a base that ran fewer tests
/// than it cannot show what the change does.
#[tokio::test]
async fn a_base_below_the_contracts_floor_is_blocked() {
    let w = World::new();
    let toml = floored(9);
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", &passing(5))]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", &passing(5)),
        ("lib.js", "x"),
    ]);
    let receipt = verdict_of(&w, base, head).await;
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
    assert_eq!(
        blockers(&receipt),
        ["unit::(evidence) below the floor on the base"]
    );
    let floor = entry(&receipt, "unit::(evidence) below the floor on the base");
    assert_eq!(floor.cause, Some(Cause::Env));
    assert!(
        detail(floor).contains("The base ran 5 tests (5 passed, 0 failed)")
            && detail(floor).contains("minimum of 9"),
        "{}",
        detail(floor)
    );
}

/// The floor is the base's: a change under which only 5 of the base's 10
/// tests ran lost the other 5 itself, each a `NEW` entry, which is a
/// stronger verdict than a blocker that is not the change's own.
#[tokio::test]
async fn tests_the_change_loses_below_the_floor_are_its_own() {
    let w = World::new();
    let toml = floored(9);
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", &passing(10))]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", &passing(5)),
        ("lib.js", "x"),
    ]);
    let receipt = verdict_of(&w, base, head).await;
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
    assert!(blockers(&receipt).is_empty(), "{:?}", blockers(&receipt));
    let lost = entry(&receipt, "unit::t9");
    assert_eq!(
        (lost.head, lost.cause),
        (HeadStatus::Fail, Some(Cause::New))
    );
}

#[tokio::test]
async fn a_floor_both_sides_meet_passes() {
    let w = World::new();
    let toml = floored(9);
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", &passing(10))]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", &passing(10)),
        ("lib.js", "x"),
    ]);
    let receipt = verdict_of(&w, base, head).await;
    assert!(blockers(&receipt).is_empty(), "{:?}", receipt.body.checks);
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

/// A type check's report lists its errors, not tests: a clean one counts
/// nothing, so neither the floor nor zero tests applies to it. Alone it
/// ran no code, so the verdict is `UNPROVEN`, not blocked.
#[tokio::test]
async fn a_type_check_is_exempt_from_the_floor() {
    let w = World::new();
    let toml = "[[check]]\nid = \"types\"\nrun = \"copy-report types.txt tsc.out\"\nreport = \"tsc\"\nreport_file = \"types.txt\"\nmin_tests = 5\n";
    let base = w.snapshot(&[("aver.toml", toml), ("tsc.out", "")]);
    let head = w.snapshot(&[("aver.toml", toml), ("tsc.out", ""), ("lib.ts", "x")]);
    let receipt = verdict_of(&w, base, head).await;
    assert!(blockers(&receipt).is_empty(), "{:?}", receipt.body.checks);
    assert_eq!(receipt.body.verdict, Verdict::Unproven);
}

/// An exit-only check the head proposes, on a base without `aver.toml`, is
/// the head vouching for itself: its exit code is no evidence.
#[tokio::test]
async fn an_unapproved_exit_only_check_is_blocked() {
    let w = World::new();
    let toml = "[[check]]\nid = \"lint\"\nrun = \"exit 0\"\nreport = \"exit\"\n";
    let base = w.snapshot(&[("lib.js", "x")]);
    let head = w.snapshot(&[("aver.toml", toml), ("lib.js", "y")]);
    let receipt = verdict_of(&w, base, head).await;
    assert_eq!(
        receipt.body.verdict,
        Verdict::BlockedNotOwn,
        "{:#?}",
        receipt.body.checks
    );
    assert_eq!(blockers(&receipt), ["lint::(evidence) exit code only"]);
    let only = entry(&receipt, "lint::(evidence) exit code only");
    assert_eq!(only.cause, Some(Cause::Env));
    assert!(
        detail(only).contains("counts only once a merged aver.toml approves it"),
        "{}",
        detail(only)
    );
}

/// The same check in the base's own `aver.toml` was approved by whoever
/// merged it, and it runs the repository's tests (Aver cannot count them).
#[tokio::test]
async fn an_approved_exit_only_check_passes() {
    let w = World::new();
    let toml = "[[check]]\nid = \"lint\"\nrun = \"exit 0\"\nreport = \"exit\"\n";
    let base = w.snapshot(&[("aver.toml", toml), ("lib.js", "x")]);
    let head = w.snapshot(&[("aver.toml", toml), ("lib.js", "y")]);
    let receipt = verdict_of(&w, base, head).await;
    assert!(blockers(&receipt).is_empty(), "{:?}", receipt.body.checks);
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}
