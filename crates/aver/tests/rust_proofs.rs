//! Rust tests in a Cargo workspace whose crate sits below the root, verified
//! natively: a new test fn in an existing integration-test file is the
//! head's own test (evidence, and a proof when tagged), while the file's
//! existing fns stay judged from the base; a head that merged a newer
//! commit whose `Cargo.toml` gives a test target `required-features` is not
//! blamed for that target failing under the base's frozen manifest; and an
//! agent's own `[[test]]` edit is still a person's call.

mod common;
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

async fn verify(w: &World, base: SnapshotId, head: SnapshotId, criteria: &[&str]) -> Verified {
    let verifier = Verifier::new(
        w.store.clone(),
        w.workspaces.clone(),
        native(w),
        Arc::new(SigningKey::from_bytes(&[7; 32])),
    );
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

/// The receipt entry of the test fn `name`.
fn named<'a>(v: &'a Verified, name: &str) -> &'a ReceiptCheck {
    let suffix = format!(" > {name}");
    v.receipt
        .body
        .checks
        .iter()
        .find(|e| e.id.as_str().ends_with(&suffix))
        .unwrap_or_else(|| panic!("no entry for {name}\n{}", text(v)))
}

fn shown(v: &Verified) -> Vec<String> {
    v.diagnostics
        .evidence
        .as_ref()
        .map(|e| {
            e.shows_change
                .iter()
                .map(|s| s.id.as_str().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

const ROOT: &str = "[workspace]\nmembers = [\"crates/calc\"]\nresolver = \"2\"\n";
const CRATE: &str =
    "[package]\nname = \"calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n";
const LIB: &str = "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n\npub fn sub(a: i64, b: i64) -> i64 {\n    a - b + 1\n}\n";
const LIB_FIXED: &str = "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n\npub fn sub(a: i64, b: i64) -> i64 {\n    a - b\n}\n";
const API: &str = "#[test]\nfn adds() {\n    assert_eq!(calc::add(2, 3), 5);\n}\n";

fn base(w: &World) -> SnapshotId {
    w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", ROOT),
            ("crates/calc/Cargo.toml", CRATE),
            ("crates/calc/src/lib.rs", LIB),
            ("crates/calc/tests/api.rs", API),
        ],
    )
}

/// The head fixes `sub` and adds a test of it to the existing file.
fn fixed(w: &World, test_name: &str) -> SnapshotId {
    let api =
        format!("{API}\n#[test]\nfn {test_name}() {{\n    assert_eq!(calc::sub(5, 3), 2);\n}}\n");
    w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", ROOT),
            ("crates/calc/Cargo.toml", CRATE),
            ("crates/calc/src/lib.rs", LIB_FIXED),
            ("crates/calc/tests/api.rs", &api),
        ],
    )
}

#[tokio::test]
async fn a_new_fn_in_an_existing_test_file_shows_the_change_crit_rust_new_test_evidence() {
    let w = World::new();
    let base = base(&w);
    let head = fixed(&w, "subtracts");
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    let subtracts = named(&v, "subtracts");
    assert!(
        shown(&v).iter().any(|id| id == subtracts.id.as_str()),
        "{:?}\n{report}",
        shown(&v)
    );
    assert!(
        !report.contains("Nothing shows the change works yet"),
        "{report}"
    );
}

/// A new test of a function the change adds does not compile on the
/// original code, and cargo then runs no test there at all. The head
/// changes nothing else, so hunk necessity has no untested part to flag.
#[tokio::test]
async fn a_new_fn_testing_a_new_function_shows_the_change_crit_rust_new_test_evidence() {
    let w = World::new();
    let base = base(&w);
    let lib = format!("{LIB}\npub fn neg(a: i64) -> i64 {{\n    -a\n}}\n");
    let api = format!("{API}\n#[test]\nfn negates() {{\n    assert_eq!(calc::neg(2), -2);\n}}\n");
    let head = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", ROOT),
            ("crates/calc/Cargo.toml", CRATE),
            ("crates/calc/src/lib.rs", &lib),
            ("crates/calc/tests/api.rs", &api),
        ],
    );
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    let shows = shown(&v);
    assert!(
        shows.len() == 1 && shows[0].ends_with(" > negates"),
        "{shows:?}\n{report}"
    );
    // A done PASS reports counts only; the transplant outcome is in the
    // diagnostics (`aver verify --json`).
    let original = v
        .diagnostics
        .evidence
        .as_ref()
        .map(|e| e.shows_change[0].original);
    assert_eq!(original, Some(aver_core::Transplant::Absent), "{report}");
}

/// A tagged test fn added to the tests module of an existing source file
/// runs on the transplant with the base's code, where it fails on its
/// assertion: a full proof, not one by a missing test.
#[tokio::test]
async fn a_new_inline_test_fn_proves_crit_rust_new_test_proof() {
    let w = World::new();
    let module = |lib: &str, tests: &str| {
        format!("{lib}\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn adds() {{\n        assert_eq!(add(2, 3), 5);\n    }}\n{tests}}}\n")
    };
    let files = |lib: &str| {
        vec![
            ("Cargo.toml", ROOT.to_owned()),
            ("crates/calc/Cargo.toml", CRATE.to_owned()),
            ("crates/calc/src/lib.rs", lib.to_owned()),
        ]
    };
    let snap = |files: Vec<(&str, String)>| {
        let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
        w.snapshot_under(CARGO_CONTRACT, &refs)
    };
    let base = snap(files(&module(LIB, "")));
    let head = snap(files(&module(
        LIB_FIXED,
        "\n    #[test]\n    fn subtracts_crit_sub() {\n        assert_eq!(sub(5, 3), 2);\n    }\n",
    )));
    let v = verify(&w, base, head, &["sub"]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    let proof = named(&v, "tests::subtracts_crit_sub");
    assert_eq!(proof.transplant, Some(Transplant::Fail), "{report}");
    assert!(v.diagnostics.weak_proofs.is_empty(), "{report}");
}

#[tokio::test]
async fn a_tagged_new_fn_in_an_existing_test_file_proves_crit_rust_new_test_proof() {
    let w = World::new();
    let base = base(&w);
    let head = fixed(&w, "subtracts_crit_sub");
    let v = verify(&w, base, head, &["sub"]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    assert!(v.receipt.body.unproven.is_empty(), "{report}");
    let proof = named(&v, "subtracts_crit_sub");
    assert_eq!(proof.transplant, Some(Transplant::Fail), "{report}");
    assert!(v.diagnostics.weak_proofs.is_empty(), "{report}");
}

#[tokio::test]
async fn an_edited_existing_fn_runs_as_the_base_has_it_crit_rust_existing_still_frozen() {
    let w = World::new();
    let base = base(&w);
    // The head breaks `add` and rewrites the existing test to expect it,
    // next to a new test of `sub`.
    let api = "#[test]\nfn adds() {\n    assert_eq!(calc::add(2, 3), 6);\n}\n\n#[test]\nfn subtracts() {\n    assert_eq!(calc::sub(5, 3), 2);\n}\n";
    let head = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", ROOT),
            ("crates/calc/Cargo.toml", CRATE),
            (
                "crates/calc/src/lib.rs",
                &LIB_FIXED.replace("    a + b\n", "    a + b + 1\n"),
            ),
            ("crates/calc/tests/api.rs", api),
        ],
    );
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    let adds = named(&v, "adds");
    // Judged with the base's `adds`, which fails on the head's code.
    assert_eq!(adds.head, HeadStatus::Fail, "{report}");
    assert_eq!(adds.cause, Some(Cause::Changed), "{report}");
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
    assert!(
        !shown(&v).iter().any(|id| id == adds.id.as_str()),
        "{report}"
    );
}

const CRATE_EXTRA: &str = "[package]\nname = \"calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[features]\nextra = []\n\n[[test]]\nname = \"extra\"\npath = \"tests/extra/main.rs\"\nrequired-features = [\"extra\"]\n";
const EXTRA_TEST: &str = "#[test]\nfn needs_extra() {\n    assert!(calc::extra_on());\n}\n";

/// A head that merged a newer commit of the repository: it gives a new test
/// target `required-features`, which the base's frozen manifest lacks.
async fn merged_main() -> (Verified, String) {
    let w = World::new();
    let base = base(&w);
    let lib =
        format!("{LIB_FIXED}\npub fn extra_on() -> bool {{\n    cfg!(feature = \"extra\")\n}}\n");
    let head = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", ROOT),
            ("crates/calc/Cargo.toml", CRATE_EXTRA),
            ("crates/calc/src/lib.rs", &lib),
            ("crates/calc/tests/api.rs", API),
            ("crates/calc/tests/extra/main.rs", EXTRA_TEST),
        ],
    );
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    (v, report)
}

#[tokio::test]
async fn a_new_target_needing_a_feature_is_not_new_crit_merged_main_not_own() {
    let (v, report) = merged_main().await;
    assert_ne!(v.receipt.body.verdict, Verdict::FailOwn, "{report}");
    assert!(
        v.receipt
            .body
            .checks
            .iter()
            .all(|e| e.cause != Some(Cause::New)),
        "{report}"
    );
    assert_eq!(
        named(&v, "needs_extra").cause,
        Some(Cause::Changed),
        "{report}"
    );
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
}

#[tokio::test]
async fn the_report_names_workspace_rebase_crit_merged_main_hint() {
    let (_, report) = merged_main().await;
    assert!(report.contains("aver workspace rebase"), "{report}");
}

#[tokio::test]
async fn an_agents_own_test_target_edit_crit_cargo_edit_still_review() {
    let w = World::new();
    let base = base(&w);
    // The agent keeps its existing test target from building on the head.
    let gamed = format!("{CRATE}\n[features]\nnever = []\n\n[[test]]\nname = \"api\"\npath = \"tests/api.rs\"\nrequired-features = [\"never\"]\n");
    let head = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", ROOT),
            ("crates/calc/Cargo.toml", &gamed),
            ("crates/calc/src/lib.rs", LIB_FIXED),
            ("crates/calc/tests/api.rs", API),
        ],
    );
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
    // With a real regression under it, the edit hides nothing.
    let broken = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", ROOT),
            ("crates/calc/Cargo.toml", &gamed),
            (
                "crates/calc/src/lib.rs",
                &LIB_FIXED.replace("    a + b\n", "    a + b + 1\n"),
            ),
            ("crates/calc/tests/api.rs", API),
        ],
    );
    let v = verify(&w, base, broken, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn, "{report}");
    assert_eq!(named(&v, "adds").cause, Some(Cause::New), "{report}");
}

/// The agent's own new test target with `required-features`: its test
/// fails under the base's frozen manifest, which is not shown to be the
/// change's failure, and the edit is a person's call: REVIEW, never PASS.
#[tokio::test]
async fn an_agents_new_test_target_is_reviewed_crit_cargo_edit_still_review() {
    let w = World::new();
    let base = base(&w);
    let lib =
        format!("{LIB_FIXED}\npub fn fast_on() -> bool {{\n    cfg!(feature = \"fast\")\n}}\n");
    let manifest = format!("{CRATE}\n[features]\nfast = []\n\n[[test]]\nname = \"fast\"\nrequired-features = [\"fast\"]\n");
    let head = w.snapshot_under(
        CARGO_CONTRACT,
        &[
            ("Cargo.toml", ROOT),
            ("crates/calc/Cargo.toml", &manifest),
            ("crates/calc/src/lib.rs", &lib),
            ("crates/calc/tests/api.rs", API),
            (
                "crates/calc/tests/fast.rs",
                "#[test]\nfn is_fast() {\n    assert!(calc::fast_on());\n}\n",
            ),
        ],
    );
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
    assert_eq!(named(&v, "is_fast").cause, Some(Cause::Changed), "{report}");
}

/// Aver's own check: the examples are built too, so the command is more
/// than a plain `cargo test`.
const EXAMPLES_CONTRACT: &str = "[[check]]\nid = \"cargo\"\nrun = \"CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --lib --bins --tests --examples --no-fail-fast\"\nreport = \"libtest\"\nmin_tests = 1\n";
const EXAMPLE: &str = "fn main() {\n    println!(\"{}\", calc::sub(5, 3));\n}\n";
const OTHER: &str = "#[test]\nfn subtracts() {\n    assert_eq!(calc::sub(5, 5), 1);\n}\n";

/// The head changes `add`'s signature on purpose and updates the existing
/// test file that calls it. The base's copy of that file, which judges the
/// existing tests, no longer compiles against the new code: that is the
/// API change, not a regression, so a person reviews it.
#[tokio::test]
async fn a_base_test_file_broken_by_a_signature_change_is_changed_crit_rust_signature_review() {
    for contract in [CARGO_CONTRACT, EXAMPLES_CONTRACT] {
        let w = World::new();
        let files = |lib: &str, api: &str| {
            vec![
                ("Cargo.toml", ROOT.to_owned()),
                ("crates/calc/Cargo.toml", CRATE.to_owned()),
                ("crates/calc/src/lib.rs", lib.to_owned()),
                ("crates/calc/tests/api.rs", api.to_owned()),
                ("crates/calc/tests/other.rs", OTHER.to_owned()),
                ("crates/calc/examples/show.rs", EXAMPLE.to_owned()),
            ]
        };
        let snap = |files: Vec<(&str, String)>| {
            let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
            w.snapshot_under(contract, &refs)
        };
        let base = snap(files(LIB, API));
        let lib = LIB.replace(
            "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}",
            "pub fn add(a: i64, b: i64, c: i64) -> i64 {\n    a + b + c\n}",
        );
        let head = snap(files(
            &lib,
            &API.replace("calc::add(2, 3)", "calc::add(2, 3, 0)"),
        ));
        let v = verify(&w, base, head, &[]).await;
        let report = text(&v);
        assert!(
            v.receipt
                .body
                .checks
                .iter()
                .all(|e| e.cause != Some(Cause::New)),
            "{contract}\n{report}"
        );
        assert_eq!(
            v.receipt.body.verdict,
            Verdict::Review,
            "{contract}\n{report}"
        );
        assert!(report.contains("crates/calc/tests/api.rs"), "{report}");
        assert!(
            report.contains("did not compile against your code in its original version"),
            "{report}"
        );
        // The other test file built and ran.
        assert_eq!(named(&v, "subtracts").head, HeadStatus::Pass, "{report}");
    }
}
