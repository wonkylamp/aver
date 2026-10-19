//! Narrowed runs, natively with real Cargo: the transplant and the
//! head-tests run of a per-test Cargo check run only the test targets the
//! head's test files make, take the other targets' results from the base
//! and the judged head, and give the receipt a full run would give; every
//! change that cannot be shown to narrow the same runs in full.

mod common;
mod scripted;
mod world;

use std::collections::BTreeSet;
use std::sync::Arc;

use aver::exec::{ExecConfig, Executor};
use aver::place::runner::{RunnerConfig, SlotRunner};
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

async fn verify_with(
    w: &World,
    exec: Executor,
    base: SnapshotId,
    head: SnapshotId,
    criteria: &[&str],
) -> Verified {
    let verifier = Verifier::new(
        w.store.clone(),
        w.workspaces.clone(),
        exec,
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

/// The same command as [`CARGO_CONTRACT`]'s, written so it cannot be
/// narrowed (its exit code is not cargo's alone), so it runs in full.
const FULL_CONTRACT: &str = "[[check]]\nid = \"cargo\"\nrun = \"CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-fail-fast && :\"\nreport = \"libtest\"\nmin_tests = 1\n";

const ROOT: &str = "[workspace]\nmembers = [\"crates/calc\"]\nresolver = \"2\"\n";
const CRATE: &str =
    "[package]\nname = \"calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n";
const LIB: &str = "/// ```\n/// assert_eq!(calc::add(1, 1), 2);\n/// ```\npub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n\npub fn sub(a: i64, b: i64) -> i64 {\n    a - b + 1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds_inline() {\n        assert_eq!(super::add(2, 2), 4);\n    }\n}\n";
const API: &str = "#[test]\nfn adds() {\n    assert_eq!(calc::add(2, 3), 5);\n}\n";
/// One test that passes and one that fails until `sub` is fixed: a target
/// the head leaves alone, with a failure on the base.
const OTHER: &str = "#[test]\nfn other_adds() {\n    assert_eq!(calc::add(0, 3), 3);\n}\n\n#[test]\nfn sub_zero() {\n    assert_eq!(calc::sub(1, 1), 0);\n}\n";
const SUB: &str = "#[test]\nfn subtracts_crit_sub() {\n    assert_eq!(calc::sub(5, 3), 2);\n}\n";

fn files(contract: &str, w: &World, extra: &[(&str, &str)], lib: &str) -> SnapshotId {
    let mut all = vec![
        ("Cargo.toml", ROOT),
        ("crates/calc/Cargo.toml", CRATE),
        ("crates/calc/src/lib.rs", lib),
        ("crates/calc/tests/api.rs", API),
        ("crates/calc/tests/other.rs", OTHER),
    ];
    for (path, text) in extra {
        match all.iter_mut().find(|(p, _)| p == path) {
            Some(slot) => slot.1 = text,
            None => all.push((path, text)),
        }
    }
    all.retain(|(_, text)| !text.is_empty());
    w.snapshot_under(contract, &all)
}

fn fixed() -> String {
    LIB.replace("a - b + 1", "a - b")
}

/// Each entry with what decides a verdict, run ids replaced by counts.
fn entries(v: &Verified) -> Vec<serde_json::Value> {
    v.receipt
        .body
        .checks
        .iter()
        .map(|c| {
            let mut json = serde_json::to_value(c).unwrap();
            let runs = json["runs"].as_object().unwrap().clone();
            json["runs"] = runs
                .into_iter()
                .map(|(k, v)| (k, v.as_array().map_or(0, Vec::len).into()))
                .collect::<serde_json::Map<_, _>>()
                .into();
            json.as_object_mut().unwrap().remove("detail");
            json
        })
        .collect()
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

fn narrowed<'a>(v: &'a Verified, side: &str) -> Option<&'a aver::diagnostics::Narrowed> {
    v.diagnostics
        .narrowed
        .iter()
        .find(|n| serde_json::to_value(n.side).unwrap() == side)
}

/// The head adds a test target: the transplant and the head-tests run
/// each run that target alone, and the receipt is the full runs', also
/// when the head breaks a test of a target it left alone (which the full
/// head-tests run fails, so reruns).
#[tokio::test]
async fn a_new_test_target_runs_alone_and_the_receipt_is_the_full_runs() {
    let fixed = fixed();
    let broken = fixed.replace("    a + b\n", "    a + b + 1\n");
    for (lib, verdict) in [(&fixed, Verdict::Pass), (&broken, Verdict::FailOwn)] {
        let head_files = [("crates/calc/tests/sub.rs", SUB)];
        let w = World::new();
        let base = files(CARGO_CONTRACT, &w, &[], LIB);
        let head = files(CARGO_CONTRACT, &w, &head_files, lib);
        let v = verify_with(&w, native(&w), base, head, &["sub"]).await;
        let report = format!("{:#?}", v.diagnostics.narrowed);
        for side in ["transplant", "head_tests"] {
            let n = narrowed(&v, side).unwrap_or_else(|| panic!("{side}: {report}"));
            assert_eq!(n.ran, ["sub tests/sub.rs"], "{side}: {report}");
            assert_eq!(n.of, Some(5), "lib, doc-tests, api, other, sub: {report}");
            assert_eq!(n.full, None, "{report}");
        }
        assert_schema(&v);
        let lines = aver::run::limits::scope(&v.receipt.body, Some(&v.diagnostics), None).lines();
        assert!(
            lines.last().unwrap().contains("; the transplant ran 1 of 5 test targets, the rest read from the original code's run; your tests' run ran 1 of 5 test targets, the rest read from your code's run"),
            "{lines:#?}"
        );
        let w2 = World::new();
        let base = files(FULL_CONTRACT, &w2, &[], LIB);
        let head = files(FULL_CONTRACT, &w2, &head_files, lib);
        let full = verify_with(&w2, native(&w2), base, head, &["sub"]).await;
        assert!(
            full.diagnostics
                .narrowed
                .iter()
                .all(|n| n.ran.is_empty() && n.full.is_some()),
            "{:#?}",
            full.diagnostics.narrowed
        );
        assert_eq!(v.receipt.body.verdict, verdict);
        assert_eq!(v.receipt.body.verdict, full.receipt.body.verdict);
        assert_eq!(v.receipt.body.unproven, full.receipt.body.unproven);
        assert_eq!(entries(&v), entries(&full));
        assert_eq!(v.receipt.body.test_changes, full.receipt.body.test_changes);
        assert_eq!(v.diagnostics.evidence, full.diagnostics.evidence);
    }
}

/// Defect: a narrowed transplant run is served as the full command's run
/// of the transplant tree, which never ran.
#[tokio::test]
async fn a_narrowed_run_is_never_the_full_commands_cached_run() {
    let w = World::new();
    let exec = native(&w);
    let lib = fixed();
    let base = files(CARGO_CONTRACT, &w, &[], LIB);
    let head = files(
        CARGO_CONTRACT,
        &w,
        &[("crates/calc/tests/sub.rs", SUB)],
        &lib,
    );
    let v = verify_with(&w, exec.clone(), base, head, &["sub"]).await;
    let proof = v
        .receipt
        .body
        .checks
        .iter()
        .find(|c| c.id.as_str().ends_with("subtracts_crit_sub"))
        .unwrap();
    assert_eq!(proof.transplant, Some(Transplant::Fail));
    let json = serde_json::to_value(&proof.runs).unwrap();
    let run: RunId = json["transplant"][0].as_str().unwrap().parse().unwrap();
    let run: CheckRun = w.store.get(&run).unwrap();
    let snap: Snapshot = w.store.get(&run.snapshot).unwrap();
    let env: Environment = w
        .store
        .get(&aver::store::pinned_env(&snap).unwrap())
        .unwrap();
    let full = env.checks.iter().next().unwrap().clone();
    let first = RunIndex::try_from(1u8).unwrap();
    assert!(
        exec.cached(&run.snapshot, &env, &full, first)
            .unwrap()
            .is_none(),
        "the full command has no run of the transplant tree"
    );
    let ran = exec.run(&run.snapshot, &env, &full, first).await.unwrap();
    assert!(!ran.cache_hit);
    assert!(
        ran.run
            .tests
            .iter()
            .any(|t| t.id.as_str().contains("other tests/other.rs")),
        "the full command ran every target"
    );
}

/// Each change Aver cannot show narrows the same runs that side in full,
/// and says why.
#[tokio::test]
async fn changes_that_cannot_be_narrowed_run_in_full() {
    let w = World::new();
    let lib = fixed();
    let helper = "pub fn five() -> i64 {\n    5\n}\n";
    let uses_helper = "mod common;\n\n#[test]\nfn subtracts_crit_sub() {\n    assert_eq!(calc::sub(common::five(), 3), 2);\n}\n";
    let declared = format!("{CRATE}\n[[test]]\nname = \"sub\"\npath = \"tests/sub.rs\"\n");
    let trivial = "#[test]\nfn runs() {}\n";
    let inline = lib.replace(
        "    fn adds_inline",
        "    fn subtracts_crit_sub() {\n        assert_eq!(super::sub(5, 3), 2);\n    }\n\n    #[test]\n    fn adds_inline",
    );
    type Case<'a> = (
        &'a str,
        Vec<(&'a str, &'a str)>,
        Vec<(&'a str, &'a str)>,
        &'a str,
        &'a str,
    );
    let cases: Vec<Case> =
        vec![
            (
                "a helper module",
                vec![],
                vec![
                    ("crates/calc/tests/sub.rs", uses_helper),
                    ("crates/calc/tests/common/mod.rs", helper),
                ],
                "transplant",
                "a changed file is a test helper or another kind of target",
            ),
            (
                "a module of another test target",
                vec![
                    ("crates/calc/tests/all.rs", "mod sub;\n"),
                    ("crates/calc/tests/sub.rs", trivial),
                ],
                vec![
                    ("crates/calc/tests/all.rs", "mod sub;\n"),
                    ("crates/calc/tests/sub.rs", SUB),
                ],
                "transplant",
                "a changed test file is a module of another test target",
            ),
            (
                "a deleted test file",
                vec![],
                vec![
                    ("crates/calc/tests/sub.rs", SUB),
                    ("crates/calc/tests/api.rs", ""),
                ],
                "head_tests",
                "a file was deleted",
            ),
            (
                "a snapshot",
                vec![],
                vec![
                    ("crates/calc/tests/sub.rs", SUB),
                    ("crates/calc/tests/snapshots/sub.snap", "2\n"),
                ],
                "transplant",
                "a changed file is not Rust source",
            ),
            (
                "a declared target",
                vec![
                    ("crates/calc/Cargo.toml", &declared),
                    ("crates/calc/tests/sub.rs", trivial),
                ],
                vec![
                    ("crates/calc/Cargo.toml", &declared),
                    ("crates/calc/tests/sub.rs", SUB),
                ],
                "transplant",
                "the package's manifest declares the test target",
            ),
            (
                "a changed test file another target includes by path",
                vec![
                    (
                        "crates/calc/tests/all.rs",
                        "#[path = \"sub.rs\"]\nmod sub;\n",
                    ),
                    ("crates/calc/tests/sub.rs", trivial),
                ],
                vec![
                    (
                        "crates/calc/tests/all.rs",
                        "#[path = \"sub.rs\"]\nmod sub;\n",
                    ),
                    ("crates/calc/tests/sub.rs", SUB),
                ],
                "transplant",
                "a test target includes a changed file by path",
            ),
            (
                "an include Aver cannot resolve",
                vec![(
                "crates/calc/tests/gen.rs",
                "include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/tests/support/gen.rs\"));\n",
            ), ("crates/calc/tests/support/gen.rs", trivial)],
                vec![
                (
                    "crates/calc/tests/gen.rs",
                    "include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/tests/support/gen.rs\"));\n",
                ),
                ("crates/calc/tests/support/gen.rs", trivial),
                ("crates/calc/tests/sub.rs", SUB),
            ],
                "transplant",
                "a test target includes a changed file by path",
            ),
            (
                "an inline test with doc-tests in the command",
                vec![],
                vec![("crates/calc/src/lib.rs", &inline)],
                "transplant",
                "the command cannot be narrowed",
            ),
        ];
    for (case, base_extra, head_extra, side, why) in cases {
        let base = files(CARGO_CONTRACT, &w, &base_extra, LIB);
        let head = files(CARGO_CONTRACT, &w, &head_extra, &lib);
        let v = verify_with(&w, native(&w), base, head, &["sub"]).await;
        let all = format!("{case}: {:#?}", v.diagnostics.narrowed);
        let n = narrowed(&v, side).unwrap_or_else(|| panic!("{all}"));
        assert_eq!(n.full.as_deref(), Some(why), "{all}");
        assert!(n.ran.is_empty(), "{all}");
        assert_schema(&v);
    }
}

/// A test target that includes an unchanged file by `#[path]` (Aver's own
/// `tests/custody/world.rs`) does not stop a new target from running alone,
/// and the receipt is still the full runs'.
#[tokio::test]
async fn an_unchanged_include_by_path_still_narrows() {
    let lib = fixed();
    let shared = [
        (
            "crates/calc/tests/uses.rs",
            "#[allow(dead_code)]\n#[path = \"support/five.rs\"]\nmod five;\n\n#[test]\nfn five_is_five() {\n    assert_eq!(five::FIVE, 5);\n}\n",
        ),
        ("crates/calc/tests/support/five.rs", "pub const FIVE: i64 = 5;\n"),
    ];
    let head_files = [shared[0], shared[1], ("crates/calc/tests/sub.rs", SUB)];
    let w = World::new();
    let base = files(CARGO_CONTRACT, &w, &shared, LIB);
    let head = files(CARGO_CONTRACT, &w, &head_files, &lib);
    let v = verify_with(&w, native(&w), base, head, &["sub"]).await;
    let report = format!("{:#?}", v.diagnostics.narrowed);
    for side in ["transplant", "head_tests"] {
        let n = narrowed(&v, side).unwrap_or_else(|| panic!("{side}: {report}"));
        assert_eq!(n.ran, ["sub tests/sub.rs"], "{side}: {report}");
    }
    let w2 = World::new();
    let base = files(FULL_CONTRACT, &w2, &shared, LIB);
    let head = files(FULL_CONTRACT, &w2, &head_files, &lib);
    let full = verify_with(&w2, native(&w2), base, head, &["sub"]).await;
    assert_eq!(v.receipt.body.verdict, Verdict::Pass, "{report}");
    assert_eq!(v.receipt.body.verdict, full.receipt.body.verdict);
    assert_eq!(entries(&v), entries(&full));
    assert_eq!(v.diagnostics.evidence, full.diagnostics.evidence);
}

/// The output of every rerun (index 2 and up) of the head's and the
/// base's runs the receipt names.
fn rerun_outputs(w: &World, v: &Verified) -> String {
    let mut ids = BTreeSet::new();
    for e in v.receipt.body.checks.iter() {
        ids.extend(e.runs.head.iter().chain(e.runs.base.iter()).cloned());
    }
    ids.iter()
        .map(|id| w.store.get::<CheckRun>(id).unwrap())
        .filter(|r| r.index != RunIndex::FIRST)
        .map(|r| String::from_utf8_lossy(&w.store.get_raw(&r.output).unwrap()).into_owned())
        .collect()
}

/// [`CARGO_CONTRACT`]'s command with a `--skip` no test matches, which
/// Aver does not focus, so its reruns run every target.
const UNFOCUSED_CONTRACT: &str = "[[check]]\nid = \"cargo\"\nrun = \"CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-fail-fast -- --skip no_such_test\"\nreport = \"libtest\"\nmin_tests = 1\n";

/// A rerun of failing Rust tests builds and runs only their targets
/// (`--lib --bins`, `--test <t>`) with `--exact` names, and gives the
/// verdict and every entry reruns of every target give.
#[tokio::test]
async fn a_rerun_runs_only_the_failing_tests_targets() {
    let broken = LIB.replace(
        "    a + b\n",
        "    if a == 2 { a + b + 1 } else { a + b }\n",
    );
    let run = |contract: &'static str| {
        let broken = broken.clone();
        async move {
            let w = World::new();
            let base = files(contract, &w, &[], LIB);
            let head = files(contract, &w, &[], &broken);
            let v = verify_with(&w, native(&w), base, head, &[]).await;
            let reruns = rerun_outputs(&w, &v);
            (v, reruns)
        }
    };
    let (v, reruns) = run(CARGO_CONTRACT).await;
    let (all, all_reruns) = run(UNFOCUSED_CONTRACT).await;
    assert!(!reruns.is_empty(), "the head's failures rerun");
    assert!(
        reruns.contains("tests/api.rs") && reruns.contains("src/lib.rs"),
        "{reruns}"
    );
    assert!(!reruns.contains("Doc-tests"), "{reruns}");
    assert!(all_reruns.contains("Doc-tests"), "{all_reruns}");
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn);
    assert_eq!(v.receipt.body.verdict, all.receipt.body.verdict);
    assert_eq!(entries(&v), entries(&all));
}
