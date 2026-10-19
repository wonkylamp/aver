//! Who lost the result decides `ENV` (ARCHITECTURE.md §1.9.3): a report
//! Aver dropped for size and a cargo probe whose stream Aver could not read
//! whole are Aver's loss, never the change's `NEW`. The probe's stream is
//! read from its own file, so its size does not depend on the run's output
//! budget. A report the runner left malformed stays `NEW`
//! (`judging.rs`, `an_unnamed_test_makes_the_check_a_command_level_failure`).

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

fn news(checks: &Set<ReceiptCheck>) -> Vec<&str> {
    checks
        .iter()
        .filter(|e| e.cause == Some(Cause::New))
        .map(|e| e.id.as_str())
        .collect()
}

fn detail(entry: &ReceiptCheck) -> &str {
    entry.detail.as_ref().map_or("", |d| d.as_str())
}

#[tokio::test]
async fn a_report_over_the_size_limit_is_env_not_new_crit_report_dropped() {
    let w = World::new();
    let config = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-oversized-if\", \"unit.xml\", \"tests.json\", \"big\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n"
    );
    let tests = r#"{"t": "pass"}"#;
    let base = w.snapshot(&[("aver.toml", &config), ("tests.json", tests)]);
    let head = w.snapshot(&[("aver.toml", &config), ("tests.json", tests), ("big", "")]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let checks = &receipt.body.checks;
    assert!(news(checks).is_empty(), "{checks:?}");
    let unit = entry(&receipt, "unit");
    assert_eq!(unit.cause, Some(Cause::Env), "{checks:?}");
    assert!(detail(unit).contains("Aver dropped it"), "{}", detail(unit));
}

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

async fn verify_natively(w: &World, base: SnapshotId, head: SnapshotId) -> Verified {
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

fn ending<'a>(v: &'a Verified, suffix: &str) -> Vec<&'a ReceiptCheck> {
    v.receipt
        .body
        .checks
        .iter()
        .filter(|e| e.id.as_str().ends_with(suffix))
        .collect()
}

const ADD: &str = "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n";
const API: &str = "#[test]\nfn adds() {\n    assert_eq!(calc::add(2, 3), 5);\n}\n";
const GONE: &str = "#[test]\nfn subtracts() {\n    assert_eq!(calc::sub(5, 3), 2);\n}\n";
const GONE_PATH: &str = "crates/calc/tests/gone.rs";
const UNBUILT_ID: &str = "::(did not compile) crates/calc/tests/gone.rs";

/// A `cargo` for a workspace of the test targets `api`, `other` and `gone`
/// whose `build` prints `stream` (shell lines calling `art <target>` for a
/// built test target and `gone` for `gone.rs`'s compile error), and whose
/// `test` fails to build while the base's `gone.rs` sits beside the head's
/// `head.rs`. Its rerun over the targets that built reports both.
fn cargo(stream: &str) -> String {
    format!(
        r#"d=$PWD
art() {{
  printf '{{"reason":"compiler-artifact","package_id":"path+file://%s/crates/calc#0.1.0","manifest_path":"%s/crates/calc/Cargo.toml","target":{{"kind":["test"],"crate_types":["bin"],"name":"%s","src_path":"%s/crates/calc/tests/%s.rs","edition":"2021","doc":false,"doctest":false,"test":true}},"profile":{{"test":true}},"fresh":true}}\n' "$d" "$d" "$1" "$d" "$1"
}}
gone() {{
  printf '{{"reason":"compiler-message","package_id":"path+file://%s/crates/calc#0.1.0","manifest_path":"%s/crates/calc/Cargo.toml","target":{{"kind":["test"],"crate_types":["bin"],"name":"gone","src_path":"%s/crates/calc/tests/gone.rs","edition":"2021","doc":false,"doctest":false,"test":true}},"message":{{"rendered":"crates/calc/tests/gone.rs:2:22: error[E0425]: cannot find function `sub` in crate `calc`\\n","level":"error","spans":[]}}}}\n' "$d" "$d" "$d"
}}
ran() {{
  printf '     Running tests/%s.rs (target/debug/deps/%s-0123abcd)\n\nrunning 1 test\ntest %s ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n' "$1" "$1" "$2"
}}
case " $* " in
" build "*)
{stream}
  exit 101 ;;
*" -p "*)
  ran api adds
  ran other others
  exit 0 ;;
esac
if [ -f crates/calc/src/head.rs ] && [ -f crates/calc/tests/gone.rs ]; then
  echo 'error[E0425]: cannot find function `sub` in crate `calc`'
  exit 101
fi
ran api adds
ran other others
if [ -f crates/calc/tests/gone.rs ]; then ran gone subtracts; fi
"#
    )
}

/// The base, and a head that deletes `gone.rs` and adds `head.rs`, with
/// `fake` as `cargo`.
fn workspace(w: &World, fake: &str) -> (SnapshotId, SnapshotId) {
    let config = r#"[[check]]
id = "rust"
run = ["sh", "-c", "cargo() { sh ./fake.sh \"$@\"; }; cargo test --workspace --no-fail-fast >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s"]
report = { format = "libtest", path = "/aver/out/rust.txt" }
covers = ["**"]
"#;
    let base: Vec<(&str, String)> = vec![
        ("aver.toml", config.to_owned()),
        ("fake.sh", fake.to_owned()),
        ("crates/calc/src/lib.rs", ADD.to_owned()),
        ("crates/calc/tests/api.rs", API.to_owned()),
        ("crates/calc/tests/other.rs", API.replace("adds", "others")),
        (GONE_PATH, GONE.to_owned()),
    ];
    let mut head: Vec<(&str, String)> = base
        .iter()
        .filter(|(p, _)| *p != GONE_PATH)
        .cloned()
        .collect();
    head.push(("crates/calc/src/head.rs", "// the change\n".to_owned()));
    let snap = |files: &[(&str, String)]| {
        let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
        w.snapshot(&refs)
    };
    (snap(&base), snap(&head))
}

/// A probe stream past a megabyte (a workspace of many test targets) is
/// read whole: the restored file is one `CHANGED` entry and every other
/// test is judged one by one.
#[tokio::test]
async fn a_probe_stream_over_a_megabyte_is_read_whole_crit_probe_lossless() {
    let w = World::new();
    let stream = "  i=0; while [ $i -lt 5000 ]; do art api; i=$((i+1)); done\n  art other\n  gone\n  echo '{\"reason\":\"build-finished\",\"success\":false}'";
    let (base, head) = workspace(&w, &cargo(stream));
    let v = verify_natively(&w, base, head).await;
    let report = text(&v);
    let checks = &v.receipt.body.checks;
    assert!(news(checks).is_empty(), "{:?}\n{report}", news(checks));
    assert_eq!(v.receipt.body.verdict, Verdict::Review, "{report}");
    assert_eq!(ending(&v, UNBUILT_ID).len(), 1, "{report}");
    for test in ["api tests/api.rs > adds", "other tests/other.rs > others"] {
        let found = ending(&v, test);
        assert_eq!(found.len(), 1, "{test}: {report}");
        assert_eq!((found[0].head, found[0].cause), (HeadStatus::Pass, None));
    }
}

/// A probe stream that stops before cargo's `build-finished` lost its
/// result to Aver: the check is `ENV`, never `NEW`.
#[tokio::test]
async fn an_incomplete_probe_is_env_not_new_crit_probe_lost() {
    let w = World::new();
    let (base, head) = workspace(&w, &cargo("  art api\n  gone"));
    let v = verify_natively(&w, base, head).await;
    let report = text(&v);
    let checks = &v.receipt.body.checks;
    assert!(news(checks).is_empty(), "{:?}\n{report}", news(checks));
    let rust = entry(&v.receipt, "rust");
    assert_eq!(rust.cause, Some(Cause::Env), "{report}");
    assert!(detail(rust).contains("Aver lost"), "{}", detail(rust));
}
