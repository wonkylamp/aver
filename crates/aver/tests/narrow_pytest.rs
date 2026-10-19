//! Narrowed pytest runs, natively with real pytest: the transplant and the
//! head-tests run of a pytest check run only the test modules the head
//! added or modified, take the other modules' results from the base and
//! the judged head, and give the receipt a full run would give; every
//! change that cannot be shown to collect the same runs in full.

mod common;
mod scripted;
mod world;

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

async fn verify_with(w: &World, base: SnapshotId, head: SnapshotId, criteria: &[&str]) -> Verified {
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

const PYTEST: &str = "[[check]]\nid = \"py\"\nrun = \"PYTHONPATH=src python3 -m pytest -p no:cacheprovider -o junit_family=xunit1 --junitxml=$AVER_OUT/py.xml\"\nreport = \"junit\"\nmin_tests = 1\n";

/// Collects exactly what pytest collects by default, but names a chooser,
/// so every side runs in full: the reference receipt.
const FULL_INI: &str = "[pytest]\npython_files = test_*.py *_test.py\n";

const CALC: &str = "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b + 1\n";
const API: &str = "from calc import add\n\n\ndef test_adds():\n    assert add(2, 3) == 5\n";
/// A module the head leaves alone, with a failure on the base.
const OTHER: &str = "from calc import add, sub\n\n\ndef test_other_adds():\n    assert add(0, 3) == 3\n\n\ndef test_sub_zero():\n    assert sub(1, 1) == 0\n";
const SUB: &str =
    "from calc import sub\n\n\ndef test_subtracts_crit_sub():\n    assert sub(5, 3) == 2\n";

fn files(w: &World, extra: &[(&str, &str)], calc: &str) -> SnapshotId {
    let mut all = vec![
        ("src/calc.py", calc),
        ("tests/test_api.py", API),
        ("tests/test_other.py", OTHER),
    ];
    for (path, text) in extra {
        match all.iter_mut().find(|(p, _)| p == path) {
            Some(slot) => slot.1 = text,
            None => all.push((path, text)),
        }
    }
    all.retain(|(_, text)| !text.is_empty());
    w.snapshot_under(PYTEST, &all)
}

fn fixed() -> String {
    CALC.replace("a - b + 1", "a - b")
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

fn narrowed<'a>(v: &'a Verified, side: &str) -> Option<&'a aver::diagnostics::Narrowed> {
    v.diagnostics
        .narrowed
        .iter()
        .find(|n| serde_json::to_value(n.side).unwrap() == side)
}

/// The head adds a test module, or adds a test to a module the base has:
/// the transplant and the head-tests run each run that module alone, and
/// the receipt is the full runs', also when the head breaks a test of a
/// module it left alone.
#[tokio::test]
#[ignore = "runs pytest natively: needs Python 3 and pytest on PATH"]
async fn a_changed_test_module_runs_alone_and_the_receipt_is_the_full_runs() {
    let fixed = fixed();
    let broken = fixed.replace("return a + b", "return a + b + 1");
    let grown = format!(
        "{OTHER}\n\n{}",
        SUB.replace("from calc import sub\n\n\n", "")
    );
    let shapes: [(&str, &str); 2] = [("tests/test_sub.py", SUB), ("tests/test_other.py", &grown)];
    for (module, text) in shapes {
        for (calc, verdict) in [(&fixed, Verdict::Pass), (&broken, Verdict::FailOwn)] {
            let w = World::new();
            let base = files(&w, &[], CALC);
            let head = files(&w, &[(module, text)], calc);
            let v = verify_with(&w, base, head, &["sub"]).await;
            let report = format!("{module}: {:#?}", v.diagnostics.narrowed);
            for side in ["transplant", "head_tests"] {
                let n = narrowed(&v, side).unwrap_or_else(|| panic!("{side}: {report}"));
                assert_eq!(n.ran, [module], "{side}: {report}");
                assert_eq!(n.full, None, "{report}");
            }
            let full_files = [(module, text), ("pytest.ini", FULL_INI)];
            let w2 = World::new();
            let base = files(&w2, &[("pytest.ini", FULL_INI)], CALC);
            let head = files(&w2, &full_files, calc);
            let full = verify_with(&w2, base, head, &["sub"]).await;
            assert!(
                full.diagnostics
                    .narrowed
                    .iter()
                    .all(|n| n.ran.is_empty() && n.full.is_some()),
                "{:#?}",
                full.diagnostics.narrowed
            );
            assert_eq!(v.receipt.body.verdict, verdict, "{report}");
            assert_eq!(v.receipt.body.verdict, full.receipt.body.verdict);
            assert_eq!(v.receipt.body.unproven, full.receipt.body.unproven);
            assert_eq!(entries(&v), entries(&full), "{report}");
            assert_eq!(v.receipt.body.test_changes, full.receipt.body.test_changes);
            assert_eq!(v.diagnostics.evidence, full.diagnostics.evidence);
        }
    }
}

/// Each change Aver cannot show collects the same runs that side in
/// full, and says why.
#[tokio::test]
#[ignore = "runs pytest natively: needs Python 3 and pytest on PATH"]
async fn changes_that_cannot_be_narrowed_run_in_full() {
    let w = World::new();
    let calc = fixed();
    let helper = "FIVE = 5\n";
    let uses_helper = "from calc import sub\nfrom tests.helpers import FIVE\n\n\ndef test_subtracts_crit_sub():\n    assert sub(FIVE, 3) == 2\n";
    let imports = "from tests.test_sub import *  # noqa\n";
    let conftest = "collect_ignore = [\"tests/test_api.py\"]\n";
    type Case<'a> = (
        &'a str,
        Vec<(&'a str, &'a str)>,
        Vec<(&'a str, &'a str)>,
        &'a str,
    );
    let cases: Vec<Case> = vec![
        (
            "a helper module",
            vec![],
            vec![
                ("tests/test_sub.py", uses_helper),
                ("tests/helpers.py", helper),
            ],
            "a changed file is not a test module",
        ),
        (
            "a deleted test module",
            vec![],
            vec![("tests/test_sub.py", SUB), ("tests/test_api.py", "")],
            "a file was deleted",
        ),
        (
            "a module another imports",
            vec![
                ("tests/test_reexport.py", imports),
                ("tests/test_sub.py", ""),
            ],
            vec![
                ("tests/test_reexport.py", imports),
                ("tests/test_sub.py", SUB),
            ],
            "another module imports a changed test module",
        ),
        (
            "a conftest choosing what is collected",
            vec![("conftest.py", conftest)],
            vec![("conftest.py", conftest), ("tests/test_sub.py", SUB)],
            "a conftest.py chooses which files pytest collects",
        ),
        (
            "testpaths elsewhere",
            vec![("pytest.ini", "[pytest]\ntestpaths = other\n")],
            vec![
                ("pytest.ini", "[pytest]\ntestpaths = other\n"),
                ("tests/test_sub.py", SUB),
            ],
            "the pytest configuration's testpaths may not collect a changed module",
        ),
        (
            "a module in a directory pytest skips",
            vec![],
            vec![("tests/test_sub.py", SUB), ("build/test_x.py", SUB)],
            "a changed test module is in a directory pytest skips",
        ),
    ];
    for (case, base_extra, head_extra, why) in cases {
        let base = files(&w, &base_extra, CALC);
        let head = files(&w, &head_extra, &calc);
        let v = verify_with(&w, base, head, &["sub"]).await;
        let all = format!("{case}: {:#?}", v.diagnostics.narrowed);
        let n = narrowed(&v, "head_tests").unwrap_or_else(|| panic!("{all}"));
        assert_eq!(n.full.as_deref(), Some(why), "{all}");
        assert!(n.ran.is_empty(), "{all}");
    }
}

/// Stands in for pytest-xdist: accepts `--numprocesses` and, when given,
/// sets what a test sees on a worker.
const WORKERS_CONFTEST: &str = "import os\n\n\ndef pytest_addoption(parser):\n    parser.addoption(\"--numprocesses\", default=None)\n\n\ndef pytest_configure(config):\n    if config.getoption(\"numprocesses\"):\n        os.environ[\"ON_WORKERS\"] = \"1\"\n";

/// A run on xdist workers that failed has its failing tests rerun
/// serially and read from there: a failure that happens only on workers
/// is not the change's, and one that also fails serially still is.
#[tokio::test]
#[ignore = "runs pytest natively: needs Python 3 and pytest on PATH"]
async fn a_failure_only_on_workers_is_read_from_its_serial_rerun() {
    let contract = PYTEST.replace("--junitxml", "--numprocesses 2 --junitxml");
    let on_workers = "import os\n\n\ndef add(a, b):\n    return a + b + (1 if os.environ.get(\"ON_WORKERS\") else 0)\n\n\ndef sub(a, b):\n    return a - b + 1\n";
    let base_calc = fixed();
    let on_workers = on_workers.replace("a - b + 1", "a - b");
    let on_workers = on_workers.as_str();
    let broken = base_calc.replace("return a + b", "return a + b + 1");
    for (calc, verdict) in [
        (on_workers, Verdict::Pass),
        (broken.as_str(), Verdict::FailOwn),
    ] {
        let w = World::new();
        let snap = |calc: &str| {
            w.snapshot_under(
                &contract,
                &[
                    ("conftest.py", WORKERS_CONFTEST),
                    ("src/calc.py", calc),
                    ("tests/test_api.py", API),
                    ("tests/test_other.py", OTHER),
                ],
            )
        };
        let (base, head) = (snap(&base_calc), snap(calc));
        let v = verify_with(&w, base, head, &[]).await;
        let new: Vec<&str> = v
            .receipt
            .body
            .checks
            .iter()
            .filter(|e| e.cause == Some(Cause::New))
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(v.receipt.body.verdict, verdict, "{new:?}");
        if verdict == Verdict::FailOwn {
            assert!(new.iter().any(|id| id.ends_with("test_adds")), "{new:?}");
        }
    }
}
