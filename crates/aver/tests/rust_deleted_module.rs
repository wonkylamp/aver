//! A change that deletes a Rust source module whose base version holds
//! inline tests and doc-tests, verified natively: those tests went with
//! the code they tested, so the file is one `CHANGED` entry (REVIEW), not
//! a `NEW` per test. A module kept with only its tests deleted gets the
//! base's tests back, and one that fails there stays `NEW` (H2;
//! ARCHITECTURE.md §1.9.3).

mod common;
mod git_support;
mod scripted;
mod world;

use std::sync::Arc;

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

async fn verify(w: &World, base: SnapshotId, head: SnapshotId) -> Verified {
    let verifier = Verifier::new(
        w.store.clone(),
        w.workspaces.clone(),
        native(w),
        Arc::new(SigningKey::from_bytes(&[7; 32])),
    );
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
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

fn caused(v: &Verified) -> Vec<(&str, Option<Cause>)> {
    v.receipt
        .body
        .checks
        .iter()
        .filter(|e| e.cause.is_some())
        .map(|e| (e.id.as_str(), e.cause))
        .collect()
}

const ROOT: &str = "[workspace]\nmembers = [\"crates/calc\"]\nresolver = \"2\"\n";
const CRATE: &str =
    "[package]\nname = \"calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n";
const LIB: &str = "pub mod sub;\n\npub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() {\n        assert_eq!(super::add(2, 3), 5);\n    }\n}\n";
const SUB_PATH: &str = "crates/calc/src/sub.rs";
const SUB_CODE: &str =
    "/// ```\n/// assert_eq!(calc::sub::sub(5, 3), 2);\n/// ```\npub fn sub(a: i64, b: i64) -> i64 {\n    a - b\n}\n";
const SUB_TESTS: &str = "\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn subtracts() {\n        assert_eq!(super::sub(5, 3), 2);\n    }\n}\n";
const DELETED_ID: &str = "::(deleted with its code) crates/calc/src/sub.rs";

fn snap(w: &World, files: &[(&str, String)]) -> SnapshotId {
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    w.snapshot_under(CARGO_CONTRACT, &refs)
}

fn base(w: &World) -> SnapshotId {
    snap(
        w,
        &[
            ("Cargo.toml", ROOT.to_owned()),
            ("crates/calc/Cargo.toml", CRATE.to_owned()),
            ("crates/calc/src/lib.rs", LIB.to_owned()),
            (SUB_PATH, format!("{SUB_CODE}{SUB_TESTS}")),
        ],
    )
}

#[tokio::test]
async fn a_deleted_module_with_inline_tests_needs_review_crit_deleted_module_review() {
    let w = World::new();
    let base = base(&w);
    let head = snap(
        &w,
        &[
            ("Cargo.toml", ROOT.to_owned()),
            ("crates/calc/Cargo.toml", CRATE.to_owned()),
            (
                "crates/calc/src/lib.rs",
                LIB.replace("pub mod sub;\n\n", ""),
            ),
        ],
    );
    let v = verify(&w, base, head).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
    let caused = caused(&v);
    assert_eq!(
        caused.len(),
        1,
        "one entry for the file: {caused:?}\n{report}"
    );
    let (id, cause) = caused[0];
    assert!(id.ends_with(DELETED_ID), "{report}");
    assert_eq!(cause, Some(Cause::Changed));
    let entry = v.receipt.body.checks.get(id).unwrap();
    let detail = entry.detail.as_ref().unwrap().as_str();
    assert!(
        detail.contains("2 tests the base ran inside it"),
        "the inline test and the doc-test: {detail}"
    );
    let adds: Vec<&ReceiptCheck> = v
        .receipt
        .body
        .checks
        .iter()
        .filter(|e| e.id.as_str().ends_with("tests::adds"))
        .collect();
    assert_eq!(adds.len(), 1, "{report}");
    assert_eq!((adds[0].head, adds[0].cause), (HeadStatus::Pass, None));
}

#[tokio::test]
async fn a_kept_module_whose_tests_were_deleted_is_judged_by_them_crit_deleted_module_kept() {
    let w = World::new();
    let base = base(&w);
    let broken = SUB_CODE.replace("a - b", "a - b + 1");
    let head = snap(
        &w,
        &[
            ("Cargo.toml", ROOT.to_owned()),
            ("crates/calc/Cargo.toml", CRATE.to_owned()),
            ("crates/calc/src/lib.rs", LIB.to_owned()),
            (SUB_PATH, broken),
        ],
    );
    let v = verify(&w, base, head).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn, "{report}");
    let caused = caused(&v);
    assert!(
        caused
            .iter()
            .all(|(id, _)| !id.contains("(deleted with its code)")),
        "{report}"
    );
    let subtracts: Vec<_> = caused
        .iter()
        .filter(|(id, _)| id.ends_with("sub::tests::subtracts"))
        .collect();
    assert_eq!(subtracts.len(), 1, "{caused:?}\n{report}");
    assert_eq!(subtracts[0].1, Some(Cause::New), "{report}");
}
