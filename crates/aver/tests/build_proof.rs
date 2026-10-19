//! Proofs by build failure (ARCHITECTURE.md §1.9.3, §1.9.4), natively on
//! Go and Rust: a test of a new API does not build on the transplant, which
//! over a base that builds is a `build_error` weak proof in the new-API
//! words: weak proofs alone never prove, so the criterion is unproven and
//! the dry run agrees. It proves nothing either over a base
//! that does not build (BLOCKED_NOT_OWN, with the one message and no
//! "would prove").

mod common;
mod scripted;
mod world;

use std::sync::Arc;

use aver::diagnostics::WEAK_BUILD_ERROR;
use aver::exec::{ExecConfig, Executor};
use aver::place::runner::{RunnerConfig, SlotRunner};
use aver::run::report::{agent_text, ReportInput};
use aver::verify::{Head, Request, Verified, Verifier};
use aver_core::*;
use world::*;

fn native(w: &World) -> Executor {
    Executor::slots(
        w.store.clone(),
        SlotRunner::new(
            w.store.clone(),
            RunnerConfig::for_process(w.home.root(), ProvenanceMode::Local),
        ),
        ExecConfig::default(),
    )
}

/// The dry run's and the verify's results for `head` against `base`.
async fn verify(w: &World, base: SnapshotId, head: SnapshotId) -> (Verified, Verified) {
    let exec = native(w);
    let verifier = Verifier::new(
        w.store.clone(),
        w.workspaces.clone(),
        exec,
        Arc::new(SigningKey::from_bytes(&[7; 32])),
    );
    let task = w.create_task(w.task_body(&w.admin, base, &["mul"]));
    let request = || Request {
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
    };
    let dry = verifier.dry_run(w.admin.id(), request()).await.unwrap();
    let real = verifier.verify(w.admin.id(), request()).await.unwrap();
    (dry, real)
}

fn text(v: &Verified) -> String {
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

fn assert_new_api_proof(v: &Verified) {
    let text = text(v);
    assert_eq!(v.receipt.body.verdict, Verdict::Unproven, "{text}");
    let weak = &v.diagnostics.weak_proofs;
    assert_eq!(weak.len(), 1, "{weak:?}\n{text}");
    assert_eq!(weak[0].reason, WEAK_BUILD_ERROR);
    assert!(weak[0].new_api, "{weak:?}");
    assert_eq!(
        v.diagnostics.unproven[0].reason,
        aver_core::rules::UnprovenReason::WeakProof
    );
    assert!(
        text.contains("only because they call what the change adds"),
        "{text}"
    );
}

fn assert_original_unbuilt(v: &Verified, check: &str) {
    let text = text(v);
    assert_eq!(v.receipt.body.verdict, Verdict::BlockedNotOwn, "{text}");
    assert!(
        text.contains(&format!(
            "The original code doesn't build for check {check}"
        )),
        "{text}"
    );
    assert!(!text.to_lowercase().contains("would prove"), "{text}");
    assert!(!text.contains("Fixed:"), "{text}");
    assert!(
        !v.receipt
            .body
            .checks
            .iter()
            .any(|e| e.id.as_str().contains("::(evidence) ")),
        "{text}"
    );
}

const GO_MOD: &str = "module example.com/calc\n\ngo 1.21\n";
const GO_ADD: &str = "package calc\n\nfunc Add(a, b int) int { return a + b }\n";
const GO_ADD_TEST: &str = "package calc\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tif Add(2, 3) != 5 {\n\t\tt.Fatal(\"Add\")\n\t}\n}\n";
const GO_MUL: &str = "package calc\n\nfunc Add(a, b int) int { return a + b }\n\nfunc Mul(a, b int) int { return a * b }\n";
const GO_MUL_TEST: &str = "package calc\n\nimport \"testing\"\n\nfunc TestMul_crit_mul(t *testing.T) {\n\tif Mul(4, 3) != 12 {\n\t\tt.Fatal(\"Mul\")\n\t}\n}\n";

#[tokio::test]
#[ignore = "runs go natively"]
async fn go_a_test_of_a_new_function_proves_weakly_over_a_base_that_builds() {
    let w = World::new();
    let base = w.snapshot_under(
        GO_CONTRACT,
        &[
            ("go.mod", GO_MOD),
            ("calc.go", GO_ADD),
            ("calc_test.go", GO_ADD_TEST),
        ],
    );
    let head = w.snapshot_under(
        GO_CONTRACT,
        &[
            ("go.mod", GO_MOD),
            ("calc.go", GO_MUL),
            ("calc_test.go", GO_ADD_TEST),
            ("mul_test.go", GO_MUL_TEST),
        ],
    );
    let (dry, real) = verify(&w, base, head).await;
    assert_new_api_proof(&real);
    assert_new_api_proof(&dry);
}

#[tokio::test]
#[ignore = "runs go natively"]
async fn go_a_base_that_does_not_compile_blocks_with_one_message() {
    let w = World::new();
    let base = w.snapshot_under(
        GO_CONTRACT,
        &[
            ("go.mod", GO_MOD),
            (
                "calc.go",
                "package calc\n\nfunc Add(a, b int) int { return a + }\n",
            ),
            ("calc_test.go", GO_ADD_TEST),
        ],
    );
    let head = w.snapshot_under(
        GO_CONTRACT,
        &[
            ("go.mod", GO_MOD),
            ("calc.go", GO_MUL),
            ("calc_test.go", GO_ADD_TEST),
            ("mul_test.go", GO_MUL_TEST),
        ],
    );
    let (dry, real) = verify(&w, base, head).await;
    assert_original_unbuilt(&real, "go");
    assert_original_unbuilt(&dry, "go");
}

const CARGO_TOML: &str =
    "[package]\nname = \"calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n";
const RS_ADD: &str = "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n";
const RS_ADD_TEST: &str = "#[test]\nfn adds() {\n    assert_eq!(calc::add(2, 3), 5);\n}\n";
const RS_MUL: &str = "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n\npub fn mul(a: i64, b: i64) -> i64 {\n    a * b\n}\n";
const RS_MUL_TEST: &str =
    "#[test]\nfn multiplies_crit_mul() {\n    assert_eq!(calc::mul(4, 3), 12);\n}\n";

#[tokio::test]
#[ignore = "runs cargo natively"]
async fn rust_a_test_of_a_new_function_proves_weakly_over_a_base_that_builds() {
    let w = World::new();
    let base = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", CARGO_TOML),
            ("src/lib.rs", RS_ADD),
            ("tests/api.rs", RS_ADD_TEST),
        ],
    );
    let head = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", CARGO_TOML),
            ("src/lib.rs", RS_MUL),
            ("tests/api.rs", RS_ADD_TEST),
            ("tests/mul.rs", RS_MUL_TEST),
        ],
    );
    let (dry, real) = verify(&w, base, head).await;
    assert_new_api_proof(&real);
    assert_new_api_proof(&dry);
}

#[tokio::test]
#[ignore = "runs cargo natively"]
async fn rust_a_base_that_does_not_compile_blocks_with_one_message() {
    let w = World::new();
    let base = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", CARGO_TOML),
            (
                "src/lib.rs",
                "pub fn add(a: i64, b: i64) -> i64 {\n    a +\n}\n",
            ),
            ("tests/api.rs", RS_ADD_TEST),
        ],
    );
    let head = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", CARGO_TOML),
            ("src/lib.rs", RS_MUL),
            ("tests/api.rs", RS_ADD_TEST),
            ("tests/mul.rs", RS_MUL_TEST),
        ],
    );
    let (dry, real) = verify(&w, base, head).await;
    assert_original_unbuilt(&real, "cargo");
    assert_original_unbuilt(&dry, "cargo");
}
