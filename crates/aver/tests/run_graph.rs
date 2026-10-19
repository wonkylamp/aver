//! Task graphs in batches (`after: #n` in a tasks file), with scripted fake
//! agents and the scripted runner: a
//! dependent task starts on its prerequisite's accepted winner and is
//! accepted as a stacked branch, a prerequisite that fails blocks its
//! dependents, two prerequisites are combined and verified before a third
//! task builds on them, `--no-auto-accept` leaves dependents waiting, and a
//! cycle is refused before anything is imported. No Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};

use aver::run::batch::summary::Outcome;
use aver::run::batch::{input, BatchState};
use aver::run::state::{RunState, StackOnto};
use git_support::git_str;
use run_support::*;

const NEGATE: &str = "mkdir -p src test\n\
    echo 'module.exports = (x) => -x;' > src/neg.js\n\
    echo '{\"[crit:neg] negates\": \"pass-if:src/neg.js\"}' > test/neg.test.json\n";

/// Needs `src/neg.js`, which only task #1 adds: it passes only on #1's work.
const SUBTRACT: &str = "test -f src/neg.js || exit 3\nmkdir -p src test\n\
    echo 'module.exports = (a, b) => a + require(\"./neg\")(b);' > src/sub.js\n\
    echo '{\"[crit:sub] subtracts\": \"pass-if:src/sub.js\"}' > test/sub.test.json\n";

const DOUBLE: &str = "mkdir -p src test\n\
    echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
    echo '{\"[crit:dbl] doubles\": \"pass-if:src/dbl.js\"}' > test/dbl.test.json\n";

fn cli(d: &TestHome, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = aver(&d.home, cwd, args, b"");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn calc(d: &TestHome) -> PathBuf {
    d.calc()
}

#[test]
fn a_dependent_task_starts_on_its_prerequisites_accepted_work() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add subtract\n  after: #1\n  - sub: subtract(a, b) returns a - b\n",
    )
    .unwrap();
    let script = d.case_agent(
        "agent.sh",
        &[("Add negate", NEGATE), ("Add subtract", SUBTRACT)],
    );
    // Room for both at once: #2 still waits for #1.
    let (out, transcript) = d.run_batch(batch_spec(&repo, tasks, &script, 2));
    assert!(out.all_ready(), "{transcript}");
    let order: Vec<&str> = transcript
        .lines()
        .filter(|l| l.contains(" started as run ") || l.contains(" finished: "))
        .collect();
    assert_eq!(order.len(), 4, "{transcript}");
    assert!(order[0].contains("#1 started"), "{transcript}");
    assert!(order[1].contains("#1 finished: PASS"), "{transcript}");
    assert!(
        order[2].contains("#2 started as run") && order[2].ends_with("on #1's accepted work"),
        "{transcript}"
    );

    // #1 was accepted so #2 could build on it; #2 starts from its commit.
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    let first = b.run(&d.home, 1).unwrap();
    let decided = first.decided.as_ref().expect("#1 accepted");
    let negate = decided.branch.clone().expect("a branch");
    assert!(negate.starts_with("aver/add-negate-"), "{negate}");
    let second = b.run(&d.home, 2).unwrap();
    assert_eq!(&second.base_commit, decided.commit.as_ref().unwrap());
    let stack = second.stacked_on.as_ref().expect("stacked");
    assert_eq!(
        stack.onto,
        StackOnto::Run {
            run: first.id.clone(),
            number: Some(1)
        }
    );
    assert_eq!(Some(stack.head), decided.head);
    assert_eq!(stack.root, b.base);
    assert_eq!(b.tasks[1].after, [1]);
    let brief = std::fs::read_to_string(second.agents[0].dir.join(".aver/BRIEF.md")).unwrap();
    assert!(
        brief.contains(&format!(
            "BUILDS ON: task #1 \"Add negate\", accepted as branch {negate}"
        )),
        "{brief}"
    );
    assert!(!brief.contains("SIBLING TASKS"), "#1 is done: {brief}");
    let t2 = &out.summary.tasks[1];
    assert_eq!(
        serde_json::to_value(t2).unwrap()["stacked_on"]["kind"],
        "run"
    );

    // Accepting #2 makes a stacked branch: its parent is #1's.
    let (code, accepted, err) = cli(&d, &repo, &["accept", "2"]);
    assert_eq!(code, 0, "{accepted}{err}");
    let subtract = git_str(&repo, &["branch", "--list", "aver/add-subtract-*"]);
    let subtract = subtract.trim();
    assert_eq!(
        git_str(&repo, &["rev-parse", &format!("{subtract}^")]),
        git_str(&repo, &["rev-parse", &negate])
    );
    assert_eq!(
        git_str(&repo, &["show", &format!("{subtract}:src/neg.js")]),
        "module.exports = (x) => -x;"
    );
}

/// Defect: the summary told a task blocked by one of its two prerequisites
/// to `aver rerun 3 --onto 1`, which starts it without #2's work.
#[test]
fn a_task_blocked_with_two_prerequisites_is_told_to_combine_them() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add double\n  - dbl: double(x) returns 2x\n\
         Add subtract\n  after: #1, #2\n  - sub: subtract(a, b) returns a - b\n",
    )
    .unwrap();
    let script = d.case_agent(
        "agent.sh",
        &[
            ("Add negate", &format!("rm src/math.js\n{NEGATE}")),
            ("Add double", DOUBLE),
            ("Add subtract", SUBTRACT),
        ],
    );
    let (out, _) = d.run_batch(batch_spec(&repo, tasks, &script, 1));
    let text = out.summary.render(160);
    assert!(
        text.contains(
            "#3 did not start: `aver rerun 1` redoes #1, then `aver combine 1 2`, then `aver rerun 3 --onto <the combine's branch>`"
        ),
        "{text}"
    );
    // `aver status` reads it back from the batch's files.
    let (code, status, err) = cli(&d, &repo, &["status"]);
    assert_eq!(code, 0, "{err}");
    assert!(status.contains("then `aver combine 1 2`"), "{status}");
}

#[test]
fn a_prerequisite_that_does_not_pass_blocks_its_dependents() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add subtract\n  after: #1\n  - sub: subtract(a, b) returns a - b\n\
         Add double\n  - dbl: double(x) returns 2x\n",
    )
    .unwrap();
    // #1 deletes what the existing test needs: FAIL_OWN.
    let script = d.case_agent(
        "agent.sh",
        &[
            ("Add negate", &format!("rm src/math.js\n{NEGATE}")),
            ("Add subtract", SUBTRACT),
            ("Add double", DOUBLE),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(&repo, tasks, &script, 1));
    let text = out.summary.render(160);
    assert!(
        text.starts_with("3 tasks · 1 ready · 1 broke something · 1 blocked by a prerequisite\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "#2 Add subtract blocked  not started: #1 did not pass: FAIL_OWN broke \"adds\""
        ),
        "{text}"
    );
    assert!(
        text.contains("#2 did not start: `aver rerun 1` redoes #1, then `aver rerun 2 --onto 1`"),
        "{text}"
    );
    assert!(!transcript.contains("#2 started"), "{transcript}");
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    assert!(b.run(&d.home, 2).is_none(), "never started");
    assert_eq!(b.tasks[1].blocked.as_ref().unwrap().after, 1);
    assert!(
        git_str(&repo, &["branch", "--list", "aver/*"]).is_empty(),
        "nothing accepted"
    );
    // `aver status` says the same from the files.
    let (code, status, err) = cli(&d, &repo, &["status"]);
    assert_eq!(code, 0, "{err}");
    assert!(status.contains("#2 Add subtract blocked"), "{status}");
    let (_, json, _) = cli(&d, &repo, &["status", "--json"]);
    assert!(json.contains("\"outcome\": \"blocked_by\""), "{json}");
}

#[test]
fn two_prerequisites_are_combined_and_verified_before_a_third_builds_on_them() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add double\n  - dbl: double(x) returns 2x\n\
         Add both\n  after: #1, #2\n  - both: uses negate and double\n",
    )
    .unwrap();
    let both = "test -f src/neg.js && test -f src/dbl.js || exit 3\nmkdir -p src test\n\
        echo 'module.exports = (x) => -2 * x;' > src/both.js\n\
        echo '{\"[crit:both] both\": \"pass-if:src/both.js\"}' > test/both.test.json\n";
    let script = d.case_agent(
        "agent.sh",
        &[
            ("Add negate", NEGATE),
            ("Add double", DOUBLE),
            ("Add both", both),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(&repo, tasks, &script, 2));
    assert!(out.all_ready(), "{transcript}");
    let branch = "aver/combine-add-negate-and-add-double";
    assert!(
        transcript.contains("#3 started as run") && transcript.contains(branch),
        "{transcript}"
    );
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    let third = b.run(&d.home, 3).unwrap();
    let stack = third.stacked_on.as_ref().unwrap();
    match &stack.onto {
        StackOnto::Combine { branch: b2, runs } => {
            assert_eq!(b2, branch);
            assert_eq!(runs.len(), 2);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(third.base_commit, git_str(&repo, &["rev-parse", branch]));
    let record = aver::run::batch::record::last(&d.home, &out.state.id).unwrap();
    assert_eq!(record.included, [1, 2]);
}

/// #2 is after #1, #3 after #1 and #2, #4 after #2 and #3: each combine's
/// heads are stacked on runs it was not given, which it finds in the home,
/// merging each task once and verifying every one.
#[test]
fn a_diamond_of_stacked_prerequisites_is_combined_from_the_home() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add subtract\n  after: #1\n  - sub: subtract(a, b) returns a - b\n\
         Add both\n  after: #1, #2\n  - both: uses negate and subtract\n\
         Add all\n  after: #2, #3\n  - all: uses everything\n",
    )
    .unwrap();
    let both = "test -f src/neg.js && test -f src/sub.js || exit 3\nmkdir -p src test\n\
        echo 'module.exports = (x) => -x - x;' > src/both.js\n\
        echo '{\"[crit:both] both\": \"pass-if:src/both.js\"}' > test/both.test.json\n";
    let all = "for f in neg sub both; do test -f src/$f.js || exit 3; done\nmkdir -p src test\n\
        echo 'module.exports = 1;' > src/all.js\n\
        echo '{\"[crit:all] all\": \"pass-if:src/all.js\"}' > test/all.test.json\n";
    let script = d.case_agent(
        "agent.sh",
        &[
            ("Add negate", NEGATE),
            ("Add subtract", SUBTRACT),
            ("Add both", both),
            ("Add all", all),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(&repo, tasks, &script, 4));
    assert!(out.all_ready(), "{transcript}");
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    let run = |n: u32| b.run(&d.home, n).unwrap();
    let (one, two, three, four) = (run(1), run(2), run(3), run(4));
    assert!(
        matches!(&two.stacked_on.as_ref().unwrap().onto, StackOnto::Run { run, .. } if *run == one.id),
        "{transcript}"
    );
    // #2's head carries #1: the combine for #3 is #2's head, both verified.
    let StackOnto::Combine { branch, runs } = &three.stacked_on.as_ref().unwrap().onto else {
        panic!("#3 starts on a combine: {:?}", three.stacked_on);
    };
    assert_eq!(runs, &[one.id.clone(), two.id.clone()]);
    assert_eq!(three.base_commit, git_str(&repo, &["rev-parse", branch]));
    for f in ["src/neg.js", "src/sub.js"] {
        git_str(&repo, &["cat-file", "-e", &format!("{branch}:{f}")]);
    }
    // #3 is stacked on #1 and #2, which are not among #4's prerequisites
    // (#2 and #3) both: combine finds #1 through the home.
    let StackOnto::Combine { branch, runs } = &four.stacked_on.as_ref().unwrap().onto else {
        panic!(
            "#4 starts on a combine: {:?}\n{transcript}",
            four.stacked_on
        );
    };
    assert_eq!(runs, &[two.id.clone(), three.id.clone()]);
    for f in ["src/neg.js", "src/sub.js", "src/both.js"] {
        git_str(&repo, &["cat-file", "-e", &format!("{branch}:{f}")]);
    }
    let record = aver::run::batch::record::last(&d.home, &out.state.id).unwrap();
    assert_eq!(record.included, [2, 3]);
    assert_eq!(
        transcript.matches(" started as run ").count(),
        4,
        "each task ran once: {transcript}"
    );
}

#[test]
fn without_auto_accept_dependents_wait_for_the_human() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add subtract\n  after: #1\n  - sub: subtract(a, b) returns a - b\n",
    )
    .unwrap();
    let script = d.case_agent(
        "agent.sh",
        &[("Add negate", NEGATE), ("Add subtract", SUBTRACT)],
    );
    let mut spec = batch_spec(&repo, tasks, &script, 2);
    spec.auto_accept = false;
    let (out, transcript) = d.run_batch(spec);
    assert!(!out.all_ready());
    assert_eq!(
        out.summary.tasks[1].outcome,
        Outcome::Waiting { after: vec![1] },
        "{transcript}"
    );
    let text = out.summary.render(160);
    assert!(text.contains("#2 Add subtract waiting  after #1"), "{text}");
    assert!(
        text.contains("#2 waits for #1: `aver accept 1`, then `aver rerun 2 --onto 1`"),
        "{text}"
    );
    assert!(git_str(&repo, &["branch", "--list", "aver/*"]).is_empty());
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    assert!(b.run(&d.home, 1).unwrap().decided.is_none());
    assert!(RunState::load(&d.home, &b.tasks[1].run).is_err());

    // The hint, end to end: #2 never started, so its prompt and criteria
    // come from the batch's tasks.json.
    let agent_cmd = format!("/bin/sh {}", script.display());
    let rerun = [
        "rerun",
        "2",
        "--agent-cmd",
        &agent_cmd,
        "--no-tui",
        "--no-git",
        "--no-deps",
    ];
    let (code, text, err) = cli(&d, &repo, &rerun);
    assert_eq!(code, 2, "a task after #1 needs --onto: {text}{err}");
    assert!(
        err.contains(
            "#2 never started, and it builds on #1; say what it starts on: aver rerun 2 --onto 1"
        ),
        "{err}"
    );
    let (code, text, err) = cli(&d, &repo, &["accept", "1"]);
    assert_eq!(code, 0, "{text}{err}");
    let (code, text, err) = cli(
        &d,
        &repo,
        &[&rerun[..2], &["--onto", "1"], &rerun[2..]].concat(),
    );
    assert_eq!(code, 0, "{text}{err}");
    assert!(text.contains("Next: aver combine 1 2"), "{text}");
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    assert!(
        b.tasks[1].history.is_empty(),
        "no run to keep: {:?}",
        b.tasks[1]
    );
    let first = b.run(&d.home, 1).unwrap();
    let second = b.run(&d.home, 2).unwrap();
    assert_eq!(second.title, "Add subtract");
    assert_eq!(
        second.base_commit,
        first.decided.as_ref().unwrap().commit.clone().unwrap()
    );
    assert!(
        matches!(
            &second.stacked_on.as_ref().unwrap().onto,
            StackOnto::Run {
                number: Some(1),
                ..
            }
        ),
        "{:?}",
        second.stacked_on
    );
    let brief = std::fs::read_to_string(second.agents[0].dir.join(".aver/BRIEF.md")).unwrap();
    assert!(brief.contains("subtract(a, b) returns a - b"), "{brief}");
    assert!(
        brief.contains("This task starts on #1 (\"Add negate\"), accepted as aver/add-negate-"),
        "RunSpec.context reaches the brief: {brief}"
    );
}

#[test]
fn a_cycle_is_refused_before_anything_is_imported() {
    let d = TestHome::start();
    let repo = calc(&d);
    std::fs::write(
        d.root.join("tasks.txt"),
        "Add negate\n  after: #2\nAdd double\n  after: #1\n",
    )
    .unwrap();
    let tasks = d.root.join("tasks.txt");
    let (code, out, err) = cli(
        &d,
        &repo,
        &[
            "run",
            "--tasks",
            tasks.to_str().unwrap(),
            "--agent-cmd",
            "true",
            "--no-tui",
        ],
    );
    assert_eq!(code, 2, "{out}{err}");
    assert!(
        err.contains("the tasks #1 -> #2 -> #1 are after each other in a cycle"),
        "{err}"
    );
    assert!(!d.home.root().join("batches").exists());
    std::fs::write(d.root.join("tasks.txt"), "Add negate\n  after: #3\n").unwrap();
    let (code, _, err) = cli(
        &d,
        &repo,
        &[
            "run",
            "--tasks",
            tasks.to_str().unwrap(),
            "--agent-cmd",
            "true",
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("task #1 is after #3, which is not a task of this batch"),
        "{err}"
    );
}
