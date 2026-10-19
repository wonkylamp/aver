//! The cross-check at the end of `aver run` (§2.10) with scripted agents on
//! the scripted calc fixture: each top attempt's tests run on the others'
//! code in a dry-run verify; a test that fails there counts against that
//! code, and one that cannot load there (a different API) does not.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use aver::keys::AgentKey;
use aver::run::agent::Template;
use aver::run::compare;
use aver::run::cross::{CrossMode, CrossReport, Outcome};
use aver::run::state::Budget;
use aver::run::supervisor::{RunOutcome, RunSpec, Supervisor};
use aver::run::view::Lines;
use aver_core::Verdict;
use run_support::*;

fn admin(d: &TestHome) -> AgentKey {
    AgentKey::read(&d.home.key_path(&"admin".parse().unwrap())).unwrap()
}

fn spec(repo: &Path, script: &Path) -> RunSpec {
    RunSpec {
        repo: repo.to_owned(),
        rev: "HEAD".into(),
        prompt: "Add negate".into(),
        title: None,
        criteria: vec![("neg".into(), "negate(x) returns -x".into())],
        protected: Vec::new(),
        agents: (0..2)
            .map(|_| Template::Custom {
                spawn: format!("/bin/sh {}", script.display()),
                resume: None,
            })
            .collect(),
        budget: Budget {
            timeout_ms: 60_000,
            idle_ms: 60_000,
            max_verifies: 4,
            max_usd: None,
        },
        deps: false,
        git_view: false,
        aver_bin: aver_bin(),
        ports: 10,
        leases: Vec::new(),
        id: None,
        batch: None,
        siblings: Vec::new(),
        stacked_on: None,
        context: None,
    }
}

fn run(d: &TestHome, spec: RunSpec, cross: CrossMode) -> (RunOutcome, String) {
    let stop = AtomicBool::new(false);
    let mut view = Lines::new(Vec::new());
    let outcome = Supervisor::new(&d.home, d.api(), admin(d), &mut view, &stop)
        .unwrap()
        .cross(cross)
        .run(spec)
        .unwrap();
    (outcome, String::from_utf8(view.into_inner()).unwrap())
}

fn order(out: &RunOutcome) -> Vec<(u32, Option<Verdict>)> {
    compare::ranked(&out.compared)
        .iter()
        .map(|(_, c)| (c.agent.index, c.verdict))
        .collect()
}

/// #1 negates with unary minus and tests for it; #2 writes `0 - x`, a
/// smaller diff, with a test that only checks the file exists. Both pass
/// with one proof each, so diff size alone would pick #2.
const UNARY_VS_SUBTRACT: &str = r#"mkdir -p src test
case "$AVER_AGENT_INDEX" in
1)
  printf '%s\n' '// Negates x.' '// Uses unary minus, as the docs ask.' 'module.exports = (x) => -x;' > src/neg.js
  echo '{"[crit:neg] negates with unary minus": "contains:src/neg.js:-x"}' > test/neg1.test.json
  ;;
2)
  echo 'module.exports = (x) => 0 - x;' > src/neg.js
  echo '{"[crit:neg] negates": "pass-if:src/neg.js"}' > test/neg2.test.json
  ;;
esac
aver verify || true
"#;

#[test]
fn a_test_failing_on_the_others_code_decides_the_pick_and_is_named() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("agent.sh", UNARY_VS_SUBTRACT);

    // Without the cross-check, the two tie on evidence and the smaller diff
    // wins, and the table says so.
    let (plain, transcript) = run(&d, spec(&repo, &script), CrossMode::Off);
    assert_eq!(
        order(&plain),
        vec![(2, Some(Verdict::Pass)), (1, Some(Verdict::Pass))],
        "{transcript}"
    );
    let table = compare::table(&plain.compared);
    assert!(
        table.contains(
            "Tie on evidence between #2 and #1, decided by diff size: review with `aver diff 2 1`"
        ),
        "{table}"
    );
    assert!(CrossReport::read(&d.home, &plain.state.id).is_none());

    let (out, transcript) = run(&d, spec(&repo, &script), CrossMode::Auto);
    assert!(transcript.contains("cross-checking"), "{transcript}");
    assert_eq!(
        order(&out),
        vec![(1, Some(Verdict::Pass)), (2, Some(Verdict::Pass))],
        "{transcript}"
    );
    let report = CrossReport::read(&d.home, &out.state.id).expect("cross.json is written");
    assert_eq!(report.failed_on(2), 1, "{report:#?}");
    assert_eq!(report.failed_on(1), 0, "{report:#?}");
    let table = compare::table(&out.compared);
    assert!(table.contains("fails 1 of #1's test:"), "{table}");
    assert!(
        table.contains("[crit:neg] negates with unary minus"),
        "{table}"
    );
    assert!(!table.contains("Tie on evidence"), "{table}");
    let (r, _) = &compare::ranked(&out.compared)[1];
    assert!(r.why.contains("fails 1 of the others' tests"), "{}", r.why);

    // `aver status` and `aver accept` gather again and read cross.json, so
    // they rank the same way.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let again = rt.block_on(compare::gather(
        &d.api(),
        &d.home,
        &admin(&d).signing_key,
        &out.state,
    ));
    let indexes: Vec<u32> = compare::ranked(&again)
        .iter()
        .map(|(_, c)| c.agent.index)
        .collect();
    assert_eq!(indexes, vec![1, 2]);
}

/// Each attempt puts the code in a different module and tests by loading
/// it, so neither's test can load on the other's code.
const DIFFERENT_MODULES: &str = r#"mkdir -p src test
case "$AVER_AGENT_INDEX" in
1)
  printf '%s\n' '// Negates x.' 'module.exports = (x) => -x;' > src/negate.js
  echo '{"[crit:neg] negate.js negates": "load-if:src/negate.js"}' > test/neg1.test.json
  ;;
2)
  echo 'module.exports = (x) => -x;' > src/neg.js
  echo '{"[crit:neg] neg.js negates": "load-if:src/neg.js"}' > test/neg2.test.json
  ;;
esac
aver verify || true
"#;

#[test]
fn a_test_that_cannot_load_on_the_others_code_is_a_different_api_not_a_failure() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("agent.sh", DIFFERENT_MODULES);
    let (out, transcript) = run(&d, spec(&repo, &script), CrossMode::Auto);
    // Each proof only loads the attempt's own module: a weak proof, which
    // alone never proves (§1.9.4). Cross-checks still compare them.
    assert_eq!(
        order(&out),
        vec![(2, Some(Verdict::Unproven)), (1, Some(Verdict::Unproven))],
        "{transcript}"
    );
    let report = CrossReport::read(&d.home, &out.state.id).expect("cross.json is written");
    assert_eq!(report.pairs.len(), 2, "{report:#?}");
    for pair in &report.pairs {
        match &pair.outcome {
            Outcome::Ran {
                failed,
                different_api,
                ..
            } => {
                assert!(failed.is_empty(), "{report:#?}");
                assert_eq!(different_api.len(), 1, "{report:#?}");
            }
            Outcome::Skipped { why } => panic!("skipped: {why}"),
        }
    }
    assert!(out.compared.iter().all(|c| c.cross_failed == 0));
    let table = compare::table(&out.compared);
    assert!(!table.contains("fails"), "{table}");
    assert!(
        table.contains("Tie on evidence between #2 and #1"),
        "{table}"
    );
}

/// Both attempts add `test/new.test.json`: #1 negates wrongly with a test
/// that only checks the file exists (the smaller diff), #2 tests unary
/// minus. #2's file goes into #1's tree as `test/new.aver-cross.test.json`
/// beside #1's own, fails there, and #2 ranks first.
const SAME_NEW_TEST_FILE: &str = r#"mkdir -p src test
case "$AVER_AGENT_INDEX" in
1)
  echo 'module.exports = (x) => x;' > src/neg.js
  echo '{"[crit:neg] negates": "pass-if:src/neg.js"}' > test/new.test.json
  ;;
2)
  printf '%s\n' '// Negates x.' '// Uses unary minus, as the docs ask.' 'module.exports = (x) => -x;' > src/neg.js
  echo '{"[crit:neg] negates with unary minus": "contains:src/neg.js:-x"}' > test/new.test.json
  ;;
esac
aver verify || true
"#;

#[test]
fn a_test_file_both_added_runs_under_another_name() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("agent.sh", SAME_NEW_TEST_FILE);
    let (out, transcript) = run(&d, spec(&repo, &script), CrossMode::Auto);
    assert_eq!(
        order(&out),
        vec![(2, Some(Verdict::Pass)), (1, Some(Verdict::Pass))],
        "{transcript}"
    );
    let report = CrossReport::read(&d.home, &out.state.id).expect("cross.json is written");
    assert!(
        report
            .pairs
            .iter()
            .all(|p| matches!(p.outcome, Outcome::Ran { .. })),
        "{report:#?}"
    );
    assert_eq!(report.failed_on(1), 1, "{report:#?}");
    assert_eq!(report.failed_on(2), 0, "{report:#?}");
    let (_, failed) = &report.against(1)[0];
    assert!(
        failed[0].contains("negates with unary minus"),
        "{report:#?}"
    );
}
