//! A change that removes a public function and deletes the test file that
//! called it, verified natively: the base's copy of that file, restored onto
//! the judged head, no longer compiles, so cargo would build no test target
//! at all. The file is one `CHANGED` entry naming it and its first compiler
//! error (REVIEW), every other test is still judged one by one, and a
//! restored test that compiles and fails stays `NEW` (ARCHITECTURE.md
//! §1.9.3).

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
use git_support::git_cmd;
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

/// The entries whose id ends with `suffix`.
fn ending<'a>(v: &'a Verified, suffix: &str) -> Vec<&'a ReceiptCheck> {
    v.receipt
        .body
        .checks
        .iter()
        .filter(|e| e.id.as_str().ends_with(suffix))
        .collect()
}

const ROOT: &str = "[workspace]\nmembers = [\"crates/calc\"]\nresolver = \"2\"\n";
const CRATE: &str =
    "[package]\nname = \"calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n";
const ADD: &str = "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n";
const ADD_BROKEN: &str = "pub fn add(a: i64, b: i64) -> i64 {\n    a + b + 1\n}\n";
const SUB: &str = "\npub fn sub(a: i64, b: i64) -> i64 {\n    a - b\n}\n";
const SUB_BROKEN: &str = "\npub fn sub(a: i64, b: i64) -> i64 {\n    a - b + 1\n}\n";
const API: &str = "#[test]\nfn adds() {\n    assert_eq!(calc::add(2, 3), 5);\n}\n";
const GONE: &str = "#[test]\nfn subtracts() {\n    assert_eq!(calc::sub(5, 3), 2);\n}\n";
const GONE_PATH: &str = "crates/calc/tests/gone.rs";
const UNBUILT_ID: &str = "::(did not compile) crates/calc/tests/gone.rs";
const AGENT_LINE: &str = "1 test file you changed or deleted did not compile against your code in its original version, so its tests did not run; the remaining tests were judged one by one.";

fn base_files() -> Vec<(&'static str, String)> {
    vec![
        ("Cargo.toml", ROOT.to_owned()),
        ("crates/calc/Cargo.toml", CRATE.to_owned()),
        ("crates/calc/src/lib.rs", format!("{ADD}{SUB}")),
        ("crates/calc/tests/api.rs", API.to_owned()),
        (GONE_PATH, GONE.to_owned()),
    ]
}

/// The base without `gone.rs`, with `lib` as the library.
fn head_files(lib: &str) -> Vec<(&'static str, String)> {
    base_files()
        .into_iter()
        .filter(|(p, _)| *p != GONE_PATH)
        .map(|(p, t)| match p {
            "crates/calc/src/lib.rs" => (p, lib.to_owned()),
            _ => (p, t),
        })
        .collect()
}

fn snap(w: &World, files: &[(&str, String)]) -> SnapshotId {
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    w.snapshot_under(CARGO_CONTRACT, &refs)
}

#[tokio::test]
async fn a_removed_function_with_its_deleted_test_file_needs_review_crit_unbuilt_review() {
    let w = World::new();
    let base = snap(&w, &base_files());
    let head = snap(&w, &head_files(ADD));
    let v = verify(&w, base, head).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
    let changed: Vec<&ReceiptCheck> = v
        .receipt
        .body
        .checks
        .iter()
        .filter(|e| e.cause.is_some())
        .collect();
    assert_eq!(changed.len(), 1, "{report}");
    let entry = changed[0];
    assert!(entry.id.as_str().ends_with(UNBUILT_ID), "{report}");
    assert_eq!(entry.cause, Some(Cause::Changed));
    let detail = entry.detail.as_ref().unwrap().as_str();
    assert!(
        detail.contains("cannot find function `sub`"),
        "the first compiler error: {detail}"
    );
    let adds = ending(&v, " > adds");
    assert_eq!(adds.len(), 1, "{report}");
    assert_eq!((adds[0].head, adds[0].cause), (HeadStatus::Pass, None));
    assert!(ending(&v, " > subtracts").is_empty(), "{report}");
    assert!(report.contains(AGENT_LINE), "{report}");
    assert!(
        report.contains("cannot find function `sub`"),
        "the first error, fenced: {report}"
    );
}

#[tokio::test]
async fn a_deleted_test_that_compiles_and_fails_stays_new_crit_unbuilt_runtime() {
    let w = World::new();
    let base = snap(&w, &base_files());
    let head = snap(&w, &head_files(&format!("{ADD}{SUB_BROKEN}")));
    let v = verify(&w, base, head).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn, "{report}");
    let subtracts = ending(&v, " > subtracts");
    assert_eq!(subtracts.len(), 1, "{report}");
    assert_eq!(subtracts[0].cause, Some(Cause::New), "{report}");
    assert!(ending(&v, UNBUILT_ID).is_empty(), "{report}");
    assert!(!report.contains(AGENT_LINE), "{report}");
}

#[tokio::test]
async fn a_regression_beside_an_unbuilt_deleted_file_is_new_crit_unbuilt_regression() {
    let w = World::new();
    let base = snap(&w, &base_files());
    let head = snap(&w, &head_files(ADD_BROKEN));
    let v = verify(&w, base, head).await;
    let report = text(&v);
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn, "{report}");
    let adds = ending(&v, " > adds");
    assert_eq!(adds.len(), 1, "{report}");
    assert_eq!(adds[0].cause, Some(Cause::New), "{report}");
    let unbuilt = ending(&v, UNBUILT_ID);
    assert_eq!(unbuilt.len(), 1, "{report}");
    assert_eq!(unbuilt[0].cause, Some(Cause::Changed));
    let news: Vec<&str> = v
        .receipt
        .body
        .checks
        .iter()
        .filter(|e| e.cause == Some(Cause::New))
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(
        news,
        [adds[0].id.as_str()],
        "no command-level NEW: {report}"
    );
}

/// `aver ci` on a Git repository with the same change: `summary.md` says
/// the deleted file's tests did not run, not that they still ran.
#[test]
fn summary_md_says_the_deleted_file_did_not_compile_crit_unbuilt_wording() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let write = |files: &[(&str, String)]| {
        for (path, text) in files {
            let path = repo.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    };
    let git = |args: &[&str]| -> String {
        let out = git_cmd(repo.path()).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    write(&base_files());
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "calc"]);
    let base = git(&["rev-parse", "HEAD"]);
    write(&head_files(ADD));
    std::fs::remove_file(repo.path().join(GONE_PATH)).unwrap();
    git(&["commit", "-q", "-am", "remove sub"]);
    let head = git(&["rev-parse", "HEAD"]);
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_aver"))
        .env("AVER_HOME", home.path())
        .arg("ci")
        .arg("--repo")
        .arg(repo.path())
        .args(["--base", &base, "--head", &head, "--out-dir"])
        .arg(out.path())
        .output()
        .unwrap();
    let summary = std::fs::read_to_string(out.path().join("summary.md")).unwrap_or_default();
    assert_eq!(
        run.status.code(),
        Some(3),
        "REVIEW exits 3: {run:?}\n{summary}"
    );
    assert!(
        summary.contains("- Deleted 1 existing test file: `crates/calc/tests/gone.rs`. The base version of 1 of them did not compile against the new code (CHANGED, needs review), so its tests did not run; the remaining tests were judged one by one."),
        "{summary}"
    );
    assert!(
        summary.contains("- Deleted 1 existing test (1 test file did not compile against the new code in its base version (CHANGED, needs review), so its tests did not run; the remaining tests were judged one by one, and none of them fails because of this change)"),
        "{summary}"
    );
    assert!(
        !summary.contains("still ran from the base version"),
        "{summary}"
    );
    assert!(summary.contains("cannot find function `sub`"), "{summary}");
}

/// A `cargo` that answers like a real one would on a workspace of three
/// test targets, except that the rerun over the targets that built reports
/// only `api`: the probe built `other` too, and the base ran its test.
const LOSING_CARGO: &str = r#"d=$PWD
art() {
  printf '{"reason":"compiler-artifact","package_id":"path+file://%s/crates/calc#0.1.0","manifest_path":"%s/crates/calc/Cargo.toml","target":{"kind":["test"],"crate_types":["bin"],"name":"%s","src_path":"%s/crates/calc/tests/%s.rs","edition":"2021","doc":false,"doctest":false,"test":true},"profile":{"test":true},"fresh":true}\n' "$d" "$d" "$1" "$d" "$1"
}
ran() {
  printf '     Running tests/%s.rs (target/debug/deps/%s-0123abcd)\n\nrunning 1 test\ntest %s ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n' "$1" "$1" "$2"
}
case " $* " in
" build "*)
  art api
  art other
  printf '{"reason":"compiler-message","package_id":"path+file://%s/crates/calc#0.1.0","manifest_path":"%s/crates/calc/Cargo.toml","target":{"kind":["test"],"crate_types":["bin"],"name":"gone","src_path":"%s/crates/calc/tests/gone.rs","edition":"2021","doc":false,"doctest":false,"test":true},"message":{"rendered":"crates/calc/tests/gone.rs:2:22: error[E0425]: cannot find function `sub` in crate `calc`\\n","level":"error","spans":[]}}\n' "$d" "$d" "$d"
  echo '{"reason":"build-finished","success":false}'
  exit 101 ;;
*" -p "*)
  ran api adds
  exit 0 ;;
esac
if [ -f crates/calc/src/head.rs ] && [ -f crates/calc/tests/gone.rs ]; then
  echo 'error[E0425]: cannot find function `sub` in crate `calc`'
  exit 101
fi
ran api adds
ran other others
if [ -f crates/calc/tests/gone.rs ]; then ran gone subtracts; fi
"#;

/// The safety rule: a test target that built on the judged head but that
/// the rerun did not report is Aver's loss, never the change's failure.
/// Its tests the base ran are one `ENV` entry, not `NEW` each.
#[tokio::test]
async fn a_target_the_rerun_left_out_is_never_new_crit_unbuilt_not_rerun() {
    let w = World::new();
    let config = r#"[[check]]
id = "rust"
run = ["sh", "-c", "cargo() { sh ./fake.sh \"$@\"; }; cargo test --workspace --no-fail-fast >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s"]
report = { format = "libtest", path = "/aver/out/rust.txt" }
covers = ["**"]
"#;
    let base_files = vec![
        ("aver.toml", config.to_owned()),
        ("fake.sh", LOSING_CARGO.to_owned()),
        ("crates/calc/src/lib.rs", ADD.to_owned()),
        ("crates/calc/tests/api.rs", API.to_owned()),
        ("crates/calc/tests/other.rs", API.replace("adds", "others")),
        (GONE_PATH, GONE.to_owned()),
    ];
    let mut head_files: Vec<(&str, String)> = base_files
        .iter()
        .filter(|(p, _)| *p != GONE_PATH)
        .cloned()
        .collect();
    head_files.push(("crates/calc/src/head.rs", "// the change\n".to_owned()));
    let base = snap(&w, &base_files);
    let head = snap(&w, &head_files);
    let v = verify(&w, base, head).await;
    let report = text(&v);
    let news: Vec<&str> = v
        .receipt
        .body
        .checks
        .iter()
        .filter(|e| e.cause == Some(Cause::New))
        .map(|e| e.id.as_str())
        .collect();
    assert!(
        news.is_empty(),
        "nothing Aver did not run is NEW: {news:?}\n{report}"
    );
    assert_ne!(v.receipt.body.verdict, Verdict::FailOwn, "{report}");
    let lost = ending(&v, "::(not run again) other tests/other.rs");
    assert_eq!(lost.len(), 1, "{report}");
    assert_eq!(lost[0].cause, Some(Cause::Env), "{report}");
    assert!(ending(&v, " > others").is_empty(), "{report}");
    let adds = ending(&v, "api tests/api.rs > adds");
    assert_eq!(adds.len(), 1, "{report}");
    assert_eq!((adds[0].head, adds[0].cause), (HeadStatus::Pass, None));
    assert_eq!(ending(&v, UNBUILT_ID).len(), 1, "{report}");
}
