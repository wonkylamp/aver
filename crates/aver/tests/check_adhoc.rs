//! `aver check -- <cmd>` (ARCHITECTURE.md §2.7.1): an agent's own test
//! command, rewritten so Aver reads its per-test results, runs natively as
//! a verify dry run on the base, the head and the transplant, and says
//! which tests show the change, which failures are new, which were already
//! failing and which new tests pass without the change.
//!
//! Each case needs its toolchain on the host (Node, Python with pip, Go),
//! so those are `#[ignore]`d and run under `make test-native`; the Cargo
//! one needs only `cargo`.

mod common;
mod scripted;
mod world;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path as FsPath;
use std::sync::Arc;

use aver::adhoc::{self, Outcome};
use aver::exec::{ExecConfig, Executor};
use aver::place::runner::{RunnerConfig, SlotRunner};
use aver::verify::{Head, Request, Side, Verifier};
use aver_core::*;
use world::*;

/// Every file of fixture `name`, by repository path.
fn fixture(name: &str) -> BTreeMap<String, String> {
    fn walk(root: &FsPath, dir: &FsPath, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, fs::read_to_string(&path).unwrap());
            }
        }
    }
    let root = common::fixture(name);
    let mut out = BTreeMap::new();
    walk(&root, &root, &mut out);
    out
}

/// What `aver check -- <command>` shows for `head` against `base`.
async fn check(
    base: &BTreeMap<String, String>,
    head: &BTreeMap<String, String>,
    command: &str,
) -> Outcome {
    let w = World::new();
    let snap = |files: &BTreeMap<String, String>| {
        let list: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        w.snapshot(&list)
    };
    let (base_id, head_id) = (snap(base), snap(head));
    let task = w.create_task(w.task_body(&w.admin, base_id, &[]));
    let exec = Executor::slots(
        w.store.clone(),
        SlotRunner::new(w.store.clone(), {
            // The agent's environment for a `src/` layout: Aver installs
            // nothing, so the package is importable as the agent has it.
            let mut config = RunnerConfig::for_process(w.home.root(), ProvenanceMode::Local);
            config.vars.insert("PYTHONPATH".into(), "src".into());
            config
        }),
        ExecConfig::default(),
    );
    let verifier = Verifier::new(
        w.store.clone(),
        w.workspaces.clone(),
        exec,
        Arc::new(SigningKey::from_bytes(&[7; 32])),
    );
    let argv: Vec<String> = command.split(' ').map(str::to_owned).collect();
    let made = adhoc::check_for(
        &argv,
        None,
        head.get("package.json").map(|s| s.as_bytes()),
        head.get("package-lock.json").map(|s| s.as_bytes()),
    )
    .unwrap();
    assert!(made.check.report.is_some(), "{command} has no report");
    let dry = verifier
        .dry_run(
            w.admin.id(),
            Request {
                task,
                head: Head::Snapshot {
                    head: head_id,
                    base: Some(base_id),
                },
                attempt: None,
                fresh: false,
                all: false,
                progress: None,
                ci: None,
                adhoc: Some(made.check),
                reason: None,
            },
        )
        .await
        .unwrap();
    assert!(
        w.store.get::<Receipt>(&dry.id).is_err(),
        "an ad-hoc dry run stored its receipt"
    );
    let timings = dry.diagnostics.timings.as_ref().expect("ad-hoc timings");
    let runs: Vec<CheckRun> = timings
        .runs
        .iter()
        .filter(|r| r.side == Side::Transplant)
        .map(|r| w.store.get(&r.run).unwrap())
        .collect();
    let outcome = Outcome::of(
        command.to_owned(),
        made.runner,
        &dry.receipt.body,
        Some(&dry.diagnostics),
        &adhoc::TransplantRuns::of(&runs),
    );
    eprintln!("{}", adhoc::text(&outcome));
    outcome
}

fn has(list: &[String], name: &str) -> bool {
    list.iter().any(|t| t.contains(name))
}

fn with(mut files: BTreeMap<String, String>, changes: &[(&str, &str)]) -> BTreeMap<String, String> {
    for (p, c) in changes {
        files.insert((*p).to_owned(), (*c).to_owned());
    }
    files
}

#[tokio::test]
#[ignore = "runs node --test natively: needs Node 22+"]
async fn calc_shows_the_change_and_labels_an_already_failing_test() {
    let broken = "const test = require('node:test');\nconst assert = require('node:assert/strict');\ntest('always broken', () => assert.equal(1, 2));\n";
    let base = with(fixture("calc"), &[("test/broken.test.js", broken)]);
    let math = base["src/math.js"].replace("throw new Error('not implemented');", "return a / b;");
    let div = "const test = require('node:test');\nconst assert = require('node:assert/strict');\nconst { add, divide } = require('../src/math');\ntest('divide halves', () => assert.equal(divide(6, 3), 2));\ntest('add still adds', () => assert.equal(add(1, 1), 2));\n";
    let head = with(
        base.clone(),
        &[("src/math.js", &math), ("test/div.test.js", div)],
    );
    let o = check(
        &base,
        &head,
        "node --test test/div.test.js test/broken.test.js",
    )
    .await;
    assert!(has(&o.shows, "divide halves"), "{o:?}");
    assert!(has(&o.already, "always broken"), "{o:?}");
    assert!(has(&o.pass_without, "add still adds"), "{o:?}");
    assert!(o.new.is_empty(), "{o:?}");
    assert!(o
        .summary()
        .starts_with("Your command: 1 test fails on the original"));
}

#[tokio::test]
async fn rust_calc_cargo_test_with_a_filter_shows_the_change() {
    let base = fixture("rust-calc");
    let lib = format!(
        "{}\n/// `a` divided by `b`.\npub fn div(a: i64, b: i64) -> i64 {{\n    a / b\n}}\n",
        base["src/lib.rs"]
    );
    let head = with(
        base.clone(),
        &[
            ("src/lib.rs", &lib),
            (
                "tests/div.rs",
                "#[test]\nfn div_divides() {\n    assert_eq!(calc::div(6, 3), 2);\n}\n",
            ),
        ],
    );
    let o = check(&base, &head, "cargo test div").await;
    assert!(has(&o.shows, "div_divides"), "{o:?}");
    assert!(
        o.unbuilt,
        "the original plus tests/div.rs does not build: {o:?}"
    );
    assert!(o.new.is_empty(), "{o:?}");
    assert_eq!(o.runner.as_deref(), Some("cargo test"));
}

#[tokio::test]
#[ignore = "runs pytest natively: needs Python 3 and pytest on PATH"]
async fn py_calc_pytest_with_k_shows_the_change() {
    let base = fixture("py-calc");
    let ops =
        base["src/calc/ops.py"].replace("raise NotImplementedError(\"divide\")", "return a / b");
    let head = with(
        base.clone(),
        &[
            ("src/calc/ops.py", &ops),
            (
                "tests/test_div.py",
                "from calc.ops import divide\n\n\ndef test_divide_halves():\n    assert divide(6, 3) == 2\n",
            ),
        ],
    );
    let o = check(&base, &head, "pytest -k divide").await;
    assert!(has(&o.shows, "test_divide_halves"), "{o:?}");
    assert!(o.new.is_empty(), "{o:?}");
}

#[tokio::test]
#[ignore = "runs go test natively: needs Go"]
async fn go_calc_go_test_run_shows_the_change() {
    let base = with(
        fixture("go-calc"),
        &[("go.mod", "module example.com/calc\n\ngo 1.21\n")],
    );
    let calc = base["calc.go"].replace("return 0, ErrNotImplemented", "return a / b, nil");
    let head = with(
        base.clone(),
        &[
            ("calc.go", &calc),
            // The fixture asks for Go 1.22; any Go with -json will do here.
            ("go.mod", "module example.com/calc\n\ngo 1.21\n"),
            (
                "div_test.go",
                "package calc\n\nimport \"testing\"\n\nfunc TestDivide(t *testing.T) {\n\tif got, err := Divide(6, 3); err != nil || got != 2 {\n\t\tt.Fatalf(\"Divide(6, 3) = %d, %v\", got, err)\n\t}\n}\n",
            ),
        ],
    );
    let o = check(&base, &head, "go test -run Divide ./...").await;
    assert!(has(&o.shows, "TestDivide"), "{o:?}");
    assert!(o.new.is_empty(), "{o:?}");
}
