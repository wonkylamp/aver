//! The Rust problems the compatibility test on real repositories found
//! (anyhow, serde_json, ripgrep, clap), each verified natively on a small
//! Cargo package: rewritten doc-tests, inline proofs that pass without the
//! change, test modules under a crate root not named `main.rs`, and
//! feature-gated code no check builds.

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

/// The receipt entry whose id ends with `suffix`.
fn named<'a>(v: &'a Verified, suffix: &str) -> &'a ReceiptCheck {
    v.receipt
        .body
        .checks
        .iter()
        .find(|e| e.id.as_str().ends_with(suffix))
        .unwrap_or_else(|| panic!("no entry ending {suffix:?}\n{}", text(v)))
}

const CRATE: &str =
    "[package]\nname = \"calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n";

fn snap(w: &World, files: &[(&str, String)]) -> SnapshotId {
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    w.snapshot_under(CARGO_CONTRACT, &refs)
}

/// serde_json, problem J2: the head breaks `double` and rewrites the
/// doc-test that caught it. The judged head runs the base's doc-test, and
/// the rewrite is a person's call.
#[tokio::test]
async fn a_rewritten_doctest_is_judged_from_the_base_crit_rust_doctest_frozen() {
    let w = World::new();
    let lib = |body: &str, expected: &str| {
        format!("/// Doubles.\n///\n/// ```\n/// assert_eq!(calc::double(2), {expected});\n/// ```\npub fn double(a: i64) -> i64 {{\n    {body}\n}}\n\npub fn one() -> i64 {{\n    1\n}}\n")
    };
    let base = snap(
        &w,
        &[
            ("Cargo.toml", CRATE.into()),
            ("src/lib.rs", lib("a * 2", "4")),
        ],
    );
    let head = snap(
        &w,
        &[
            ("Cargo.toml", CRATE.into()),
            ("src/lib.rs", lib("a * 3", "6")),
        ],
    );
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    assert_ne!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    let doc = named(&v, "src/lib.rs - double");
    assert_eq!(doc.head, HeadStatus::Fail, "{report}");
    assert_eq!(doc.cause, Some(Cause::Changed), "{report}");
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");

    // Without the rewrite the doc-test is the head's failure.
    let broken = snap(
        &w,
        &[
            ("Cargo.toml", CRATE.into()),
            ("src/lib.rs", lib("a * 3", "4")),
        ],
    );
    let v = verify(&w, base, broken, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn, "{report}");
    assert_eq!(
        named(&v, "src/lib.rs - double").cause,
        Some(Cause::New),
        "{report}"
    );
}

/// ripgrep, problem R1: a tagged inline test of what the base already does
/// passes on the transplant (the base's code with the head's tests), so it
/// proves nothing.
#[tokio::test]
async fn an_inline_test_passing_without_the_change_proves_nothing_crit_rust_inline_proof_transplant(
) {
    let w = World::new();
    let module = |code: &str, tests: &str| {
        format!("pub fn parse(s: &str) -> u64 {{\n{code}}}\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn upper() {{\n        assert_eq!(parse(\"K\"), 1024);\n    }}\n{tests}}}\n")
    };
    let base_code = "    match s {\n        \"K\" => 1024,\n        _ => 0,\n    }\n";
    let head_code = "    match s {\n        \"K\" | \"k\" => 1024,\n        _ => 0,\n    }\n";
    let base = snap(
        &w,
        &[
            ("Cargo.toml", CRATE.into()),
            ("src/lib.rs", module(base_code, "")),
        ],
    );
    // The tagged test checks only what the original already does.
    let idle = "\n    #[test]\n    fn suffix_crit_lower() {\n        assert_eq!(parse(\"K\"), 1024);\n    }\n";
    let head = snap(
        &w,
        &[
            ("Cargo.toml", CRATE.into()),
            ("src/lib.rs", module(head_code, idle)),
        ],
    );
    let v = verify(&w, base, head, &["lower"]).await;
    let report = text(&v);
    assert_ne!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    let proof = named(&v, "tests::suffix_crit_lower");
    assert_eq!(proof.transplant, Some(Transplant::Pass), "{report}");
    assert_eq!(v.receipt.body.unproven.len(), 1, "{report}");
    let unproven = &v.diagnostics.unproven;
    assert_eq!(unproven.len(), 1, "{report}");
    assert_eq!(
        unproven[0].reason,
        rules::UnprovenReason::PassesWithoutChange,
        "{report}"
    );

    // A test of the new behaviour fails on the transplant and proves it.
    let real = "\n    #[test]\n    fn suffix_crit_lower() {\n        assert_eq!(parse(\"k\"), 1024);\n    }\n";
    let head = snap(
        &w,
        &[
            ("Cargo.toml", CRATE.into()),
            ("src/lib.rs", module(head_code, real)),
        ],
    );
    let v = verify(&w, base, head, &["lower"]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    assert_eq!(
        named(&v, "tests::suffix_crit_lower").transplant,
        Some(Transplant::Fail),
        "{report}"
    );
}

/// ripgrep and clap, problem C2: `tests/tests.rs` declares `mod misc;`,
/// which lives in `tests/misc.rs`. An edited expectation there is the
/// head's rewrite of an existing test: CHANGED, not NEW.
#[tokio::test]
async fn an_edited_test_in_a_module_of_a_crate_root_is_changed_crit_rust_submodule_changed() {
    let w = World::new();
    let lib = |body: &str| format!("pub fn size(s: &str) -> u64 {{\n    {body}\n}}\n");
    let misc = |expected: &str| {
        format!("#[test]\nfn max_filesize_parse_k_suffix() {{\n    assert_eq!(calc::size(\"1K\"), {expected});\n}}\n")
    };
    let files = |body: &str, expected: &str| {
        vec![
            ("Cargo.toml", CRATE.to_owned()),
            ("src/lib.rs", lib(body)),
            ("tests/tests.rs", "mod misc;\n".to_owned()),
            ("tests/misc.rs", misc(expected)),
        ]
    };
    let base = snap(&w, &files("if s == \"1K\" { 1024 } else { 0 }", "1024"));
    let head = snap(&w, &files("if s == \"1K\" { 2048 } else { 0 }", "2048"));
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    let entry = named(&v, "misc::max_filesize_parse_k_suffix");
    assert_eq!(entry.cause, Some(Cause::Changed), "{report}");
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
}

/// clap, RUST-3: a regression inside `#[cfg(feature = "env")]` code, under
/// a check that builds default features only, ran no test, so it was a
/// PASS. The file is uncovered, the verdict UNPROVEN, and the report names
/// the feature and a command that builds it.
#[tokio::test]
async fn a_change_under_a_feature_no_check_builds_is_unproven_crit_rust_feature_gated() {
    let w = World::new();
    let (base, head) = feature_pair(&w, "", "set", "!set");
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Unproven, "{report}");
    let uncovered: Vec<&str> = v
        .receipt
        .body
        .uncovered
        .as_ref()
        .map(|u| u.items.iter().map(|p| p.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(uncovered, vec!["src/lib.rs"], "{report}");
    let unbuilt = &v.diagnostics.unbuilt;
    assert_eq!(unbuilt.len(), 1, "{report}");
    assert_eq!(unbuilt[0].features, vec!["env".to_owned()], "{report}");
    assert!(
        report.contains("cargo test --workspace --features calc/env"),
        "{report}"
    );
    assert_schema(&v);
}

/// The diagnostics against their published schema.
fn assert_schema(v: &Verified) {
    let load = |file: &str| -> serde_json::Value {
        let path = format!("{}/../../schemas/{file}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let defs = load("aver.defs.v0.json");
    let registry = jsonschema::Registry::new()
        .add("https://aver.build/schemas/defs/v0", &defs)
        .unwrap()
        .prepare()
        .unwrap();
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&load("aver.diagnostics.v0.json"))
        .unwrap();
    let instance = serde_json::to_value(&v.diagnostics).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}

/// The same regression with `env` among the default features: the check
/// builds the code, its test catches the break, and nothing is uncovered.
#[tokio::test]
async fn a_change_under_a_default_feature_is_judged_as_usual_crit_rust_feature_gated() {
    let w = World::new();
    let (base, head) = feature_pair(&w, "\"env\"", "set", "!set");
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn, "{report}");
    assert!(v.receipt.body.uncovered.is_none(), "{report}");
}

/// A change outside every gate of a crate with features no check enables is
/// covered: no false UNPROVEN on ungated code.
#[tokio::test]
async fn an_ungated_change_beside_a_gated_one_is_covered_crit_rust_feature_gated() {
    let w = World::new();
    let (base, _) = feature_pair(&w, "", "set", "set");
    let head = snap(&w, &feature_files("", "set", "2 - 1"));
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    assert!(v.receipt.body.uncovered.is_none(), "{report}");
}

/// A crate with feature `env` (in `default` when `default` says so), an
/// ungated `plain` returning `plain`, and a gated `from_env` returning
/// `env`, each with its test.
fn feature_files(default: &str, env: &str, plain: &str) -> Vec<(&'static str, String)> {
    let manifest = format!("{CRATE}\n[features]\ndefault = [{default}]\nenv = []\n");
    let lib = format!("pub fn plain() -> u8 {{\n    {plain}\n}}\n\n#[cfg(feature = \"env\")]\npub fn from_env(set: bool) -> bool {{\n    {env}\n}}\n");
    let test = "#[test]\nfn plain() {\n    assert_eq!(calc::plain(), 1);\n}\n\n#[cfg(feature = \"env\")]\n#[test]\nfn env() {\n    assert!(calc::from_env(true));\n}\n";
    vec![
        ("Cargo.toml", manifest),
        ("src/lib.rs", lib),
        ("tests/api.rs", test.to_owned()),
    ]
}

fn feature_pair(w: &World, default: &str, base: &str, head: &str) -> (SnapshotId, SnapshotId) {
    (
        snap(w, &feature_files(default, base, "1")),
        snap(w, &feature_files(default, head, "1")),
    )
}

/// Verdict-integrity scenario B7(b): the head adds a field to a public struct and updates
/// the struct literal in the inline test that builds one. The base's copy
/// of that test no longer compiles on the head, which is the API change,
/// not a regression: the test is CHANGED and the verdict REVIEW, never a
/// command-level FAIL_OWN.
#[tokio::test]
#[ignore = "runs cargo test natively on a scratch package"]
async fn a_struct_field_added_with_its_inline_test_literal_is_changed_crit_rust_struct_change() {
    let w = World::new();
    let lib = |fields: &str, norm: &str, literal: &str| {
        format!("pub struct Point {{\n{fields}}}\n\npub fn norm(p: &Point) -> i64 {{\n    {norm}\n}}\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn norm_of_negative() {{\n        assert_eq!(norm(&Point {{ {literal} }}), 2);\n    }}\n}}\n")
    };
    let base = snap(
        &w,
        &[
            ("Cargo.toml", CRATE.into()),
            ("src/lib.rs", lib("    pub x: i64,\n", "p.x.abs()", "x: -2")),
        ],
    );
    let head = snap(
        &w,
        &[
            ("Cargo.toml", CRATE.into()),
            (
                "src/lib.rs",
                lib(
                    "    pub x: i64,\n    pub y: i64,\n",
                    "p.x.abs() + p.y.abs()",
                    "x: -2, y: 0",
                ),
            ),
        ],
    );
    let v = verify(&w, base, head, &[]).await;
    let report = text(&v);
    let entries = &v.receipt.body.checks;
    assert!(
        entries.iter().any(|e| e.cause == Some(Cause::Changed)
            && (e.id.as_str().ends_with("tests::norm_of_negative")
                || e.id.as_str().ends_with("src/lib.rs"))),
        "{report}"
    );
    assert!(
        entries
            .iter()
            .all(|e| !matches!(e.cause, Some(Cause::New | Cause::Env))),
        "{report}"
    );
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
}
