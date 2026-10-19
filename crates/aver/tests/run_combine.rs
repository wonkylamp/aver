//! `aver combine` over a batch's tasks and `aver rerun`, with scripted fake
//! agents and the scripted runner: a combine
//! that conflicts accepts and exports nothing and prints both sides' lines,
//! `--partial` leaves out the conflicting or failing task and exports the
//! rest, `aver rerun 2 --onto 1` stacks task 2 on task 1's accepted work,
//! and combining the stacked task verifies the task under it too. No
//! Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};

use aver::run::batch::record::{self, CombineStatus, ExcludedWhy};
use aver::run::batch::{input, BatchState};
use aver::run::state::{RunState, StackOnto};
use git_support::git_str;
use run_support::*;

const README: &str = "calc\nl2\nl3\nl4\nl5\n";

fn calc(d: &TestHome) -> PathBuf {
    d.repo(
        "calc",
        &[
            ("aver.toml", &aver_toml()),
            ("src/math.js", "exports.add = (a, b) => a + b;\n"),
            ("test/math.test.json", r#"{"adds": "pass-if:src/math.js"}"#),
            ("README.md", README),
        ],
    )
}

fn cli(d: &TestHome, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = aver(&d.home, cwd, args, b"");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn branches(repo: &Path) -> String {
    git_str(repo, &["branch", "--list", "--format=%(refname:short)"])
}

const NEGATE: &str = "mkdir -p src test\n\
    echo 'module.exports = (x) => -x;' > src/neg.js\n\
    echo '{\"[crit:neg] negates\": \"pass-if:src/neg.js\"}' > test/neg.test.json\n";

const DOUBLE: &str = "mkdir -p src test\n\
    echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
    echo '{\"[crit:dbl] doubles\": \"pass-if:src/dbl.js\"}' > test/dbl.test.json\n";

const TRIPLE: &str = "mkdir -p src test\n\
    echo 'module.exports = (x) => 3 * x;' > src/tri.js\n\
    echo '{\"[crit:tri] triples\": \"pass-if:src/tri.js\"}' > test/tri.test.json\n";

/// Line 3 of README.md with `text` appended, whatever it holds now: on
/// the base `l3 + text`, on top of task 1 `negate here + text`.
fn append_line_3(text: &str) -> String {
    format!(
        "l3=$(sed -n 3p README.md)\n\
         {{ sed -n 1,2p README.md; echo \"$l3 + {text}\"; sed -n '4,$p' README.md; }} > README.new\n\
         mv README.new README.md\n"
    )
}

fn tasks(text: &str) -> Vec<input::BatchTask> {
    input::parse_tasks(text).unwrap()
}

#[test]
fn a_conflicting_combine_exports_nothing_and_partial_leaves_the_conflict_out() {
    let d = TestHome::start();
    let repo = calc(&d);
    let base = git_str(&repo, &["rev-parse", "HEAD"]);
    let script = d.case_agent(
        "agent.sh",
        &[
            (
                "Add negate",
                &format!("{NEGATE}printf 'calc\\nl2\\nnegate here\\nl4\\nl5\\n' > README.md"),
            ),
            (
                "Add double",
                &format!("{DOUBLE}printf 'calc\\nl2\\ndouble here\\nl4\\nl5\\n' > README.md"),
            ),
            (
                "Add triple",
                &format!("{TRIPLE}printf 'calc\\nl2\\nl3\\nl4\\nl5 triple\\n' > README.md"),
            ),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks(
            "Add negate\n  - neg: negate\n\
             Add double\n  - dbl: double\n\
             Add triple\n  - tri: triple\n",
        ),
        &script,
        3,
    ));
    assert!(out.all_ready(), "{transcript}");
    let id = out.state.id.clone();
    let before = branches(&repo);

    // All or nothing: a conflict accepts nothing and exports nothing.
    let (code, text, err) = cli(&d, &repo, &["combine", "--all-pass"]);
    assert_eq!(code, 1, "{text}{err}");
    assert_eq!(branches(&repo), before, "no branch: {text}");
    assert!(text.contains(aver::run::report::FENCE_NOTE), "{text}");
    assert!(
        text.contains("README.md: both changed the same or adjacent lines"),
        "{text}"
    );
    assert!(
        text.contains("#1 (\"Add negate\")'s lines:\n```\nnegate here\n```"),
        "{text}"
    );
    assert!(
        text.contains("#2 (\"Add double\")'s lines:\n```\ndouble here\n```"),
        "{text}"
    );
    assert!(
        text.contains("Next: aver rerun 2 --onto 1   aver combine --all-pass --partial"),
        "{text}"
    );
    let rec = record::last(&d.home, &id).unwrap();
    assert_eq!(rec.status, CombineStatus::Conflicts);
    assert!(rec.included.is_empty());
    assert_eq!(rec.excluded.len(), 1);
    assert_eq!(rec.excluded[0].number, 2);
    assert_eq!(
        rec.excluded[0].why,
        ExcludedWhy::Conflict {
            with: 1,
            paths: vec!["README.md".into()]
        }
    );
    for t in &BatchState::load(&d.home, &id).unwrap().tasks {
        let r = RunState::load(&d.home, &t.run).unwrap();
        assert!(r.decided.is_none(), "#{} was not accepted", t.number);
    }

    // --partial: #2 conflicts with #1, which comes first, so it is left
    // out; #1 and #3 are merged, verified, accepted and exported.
    let (code, json, err) = cli(&d, &repo, &["combine", "--all-pass", "--partial", "--json"]);
    assert_eq!(code, 1, "something was left out: {json}{err}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let branch = "aver/combine-add-negate-and-add-triple";
    assert_eq!(v["status"], "partial", "{json}");
    assert_eq!(v["branch"], branch, "{json}");
    let included: Vec<u64> = v["included"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["number"].as_u64().unwrap())
        .collect();
    assert_eq!(included, [1, 3], "{json}");
    assert_eq!(v["excluded"][0]["number"], 2, "{json}");
    assert_eq!(v["excluded"][0]["why"]["kind"], "conflict", "{json}");
    assert_eq!(v["excluded"][0]["why"]["with"]["number"], 1, "{json}");
    assert_eq!(v["excluded"][0]["why"]["paths"][0], "README.md", "{json}");
    assert!(
        v["verdicts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["verdict"] == "PASS"),
        "{json}"
    );
    let next: Vec<&str> = v["next"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(
        next,
        [
            format!("git switch {branch}"),
            format!("aver rerun 2 --onto {branch}")
        ],
        "{json}"
    );
    let names = branches(&repo);
    let names: Vec<&str> = names.lines().collect();
    assert!(
        names.len() == 4
            && names[0].starts_with("aver/add-negate-")
            && names[1].starts_with("aver/add-triple-")
            && names[2] == branch
            && names[3] == "main",
        "one combine, and only the included tasks accepted: {names:?}"
    );
    assert_eq!(git_str(&repo, &["rev-parse", &format!("{branch}^")]), base);
    assert_eq!(
        git_str(&repo, &["show", &format!("{branch}:README.md")]),
        "calc\nl2\nnegate here\nl4\nl5 triple"
    );
    let message = git_str(&repo, &["log", "-1", "--format=%B", branch]);
    assert!(
        message.contains("Left out:\n- Add double: conflicts with #1"),
        "{message}"
    );
    let rec = record::last(&d.home, &id).unwrap();
    assert_eq!(rec.status, CombineStatus::Partial);
    assert_eq!(rec.included, [1, 3]);
    assert_eq!(rec.branch.as_deref(), Some(branch));
    assert_eq!(rec.excluded[0].number, 2);

    // The left-out task again, on the combine, as the hint says; then
    // combining it alone verifies the three tasks.
    let agent_cmd = format!("/bin/sh {}", script.display());
    let (code, text, err) = cli(
        &d,
        &repo,
        &[
            "rerun",
            "2",
            "--onto",
            branch,
            "--agent-cmd",
            &agent_cmd,
            "--no-tui",
            "--no-git",
            "--no-deps",
        ],
    );
    assert_eq!(code, 0, "{text}{err}");
    assert!(text.contains("Next: aver combine 2"), "{text}");
    let two = RunState::load(
        &d.home,
        &BatchState::load(&d.home, &id).unwrap().tasks[1].run,
    )
    .unwrap();
    let Some(StackOnto::Combine { branch: on, runs }) =
        two.stacked_on.as_ref().map(|s| s.onto.clone())
    else {
        panic!("stacked on the combine: {:?}", two.stacked_on);
    };
    assert_eq!(on, branch);
    assert_eq!(runs.len(), 2);
    assert_eq!(
        two.base_commit,
        git_str(&repo, &["rev-parse", branch]),
        "task 2 starts on the combine"
    );
    let brief = std::fs::read_to_string(two.agents[0].dir.join(".aver/BRIEF.md")).unwrap();
    assert!(
        brief.contains(&format!("This task starts on {branch}"))
            && brief.contains("Its earlier run conflicted with #1 at README.md"),
        "the rerun's context reaches the brief: {brief}"
    );
    let (code, json, err) = cli(
        &d,
        &repo,
        &["combine", "2", "--json", "--branch", "aver/all"],
    );
    assert_eq!(code, 0, "{json}{err}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let mut verified: Vec<u64> = v["verdicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["number"].as_u64().unwrap())
        .collect();
    verified.sort_unstable();
    assert_eq!(verified, [1, 2, 3], "{json}");
    assert_eq!(
        git_str(&repo, &["rev-parse", "aver/all^"]),
        git_str(&repo, &["rev-parse", branch])
    );
    assert_eq!(record::last(&d.home, &id).unwrap().included, [1, 2, 3]);
}

#[test]
fn a_task_that_fails_on_the_merge_is_left_out() {
    let d = TestHome::start();
    let repo = calc(&d);
    // Task 3's test passes while src/tri.js is a copy of src/math.js; task 1
    // changes src/math.js, so they merge cleanly and task 3 fails on it.
    let script = d.case_agent(
        "agent.sh",
        &[
            (
                "Add negate",
                &format!("{NEGATE}echo 'exports.neg = (x) => -x;' >> src/math.js"),
            ),
            ("Add double", DOUBLE),
            (
                "Add triple",
                "mkdir -p test\ncp src/math.js src/tri.js\n\
                 echo '{\"[crit:tri] triples\": \"agree:src/tri.js=src/math.js\"}' > test/tri.test.json",
            ),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks(
            "Add negate\n  - neg: negate\n\
             Add double\n  - dbl: double\n\
             Add triple\n  - tri: triple\n",
        ),
        &script,
        3,
    ));
    assert!(out.all_ready(), "{transcript}");
    let id = out.state.id.clone();
    let before = branches(&repo);

    let (code, text, err) = cli(&d, &repo, &["combine", "--all-pass"]);
    assert_eq!(code, 1, "{text}{err}");
    assert!(
        text.contains("Merged cleanly, but not every task passes on the result; nothing was accepted or exported"),
        "{text}"
    );
    assert_eq!(
        text.matches("#3 \"Add triple\": FAIL").count(),
        1,
        "once, on its verdict line: {text}"
    );
    assert_eq!(branches(&repo), before, "{text}");
    assert_eq!(
        record::last(&d.home, &id).unwrap().status,
        CombineStatus::NotPass
    );

    let (code, text, err) = cli(&d, &repo, &["combine", "--all-pass", "--partial"]);
    assert_eq!(code, 1, "{text}{err}");
    let branch = "aver/combine-add-negate-and-add-double";
    assert!(
        text.contains(&format!("Combined 2 runs as branch {branch}")),
        "{text}"
    );
    assert!(
        text.contains("#3 (\"Add triple\") left out: #3 (\"Add triple\") is"),
        "{text}"
    );
    assert!(
        text.contains(&format!("aver rerun 3 --onto {branch}")),
        "{text}"
    );
    let rec = record::last(&d.home, &id).unwrap();
    assert_eq!(rec.status, CombineStatus::Partial);
    assert_eq!(rec.included, [1, 2]);
    assert_eq!(rec.excluded.len(), 1);
    assert_eq!(rec.excluded[0].number, 3);
    assert!(
        matches!(rec.excluded[0].why, ExcludedWhy::NotPass { .. }),
        "{:?}",
        rec.excluded
    );
    assert!(!branches(&repo).contains("aver/add-triple"));
}

#[test]
fn rerun_onto_stacks_a_task_and_combine_verifies_the_task_under_it() {
    let d = TestHome::start();
    let repo = calc(&d);
    let base = git_str(&repo, &["rev-parse", "HEAD"]);
    let script = d.case_agent(
        "agent.sh",
        &[
            (
                "Add negate",
                &format!("{NEGATE}printf 'calc\\nl2\\nnegate here\\nl4\\nl5\\n' > README.md"),
            ),
            (
                "Add double",
                &format!("{DOUBLE}{}", append_line_3("double")),
            ),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks("Add negate\n  - neg: negate\nAdd double\n  - dbl: double\n"),
        &script,
        2,
    ));
    assert!(out.all_ready(), "{transcript}");
    let id = out.state.id.clone();
    let (code, text, _) = cli(&d, &repo, &["combine", "--all-pass"]);
    assert_eq!(code, 1, "the two change line 3: {text}");
    let old = BatchState::load(&d.home, &id).unwrap().tasks[1].run.clone();

    // Task 2 again, on task 1's work, which is accepted first.
    let agent_cmd = format!("/bin/sh {}", script.display());
    let (code, text, err) = cli(
        &d,
        &repo,
        &[
            "rerun",
            "2",
            "--onto",
            "1",
            "--agent-cmd",
            &agent_cmd,
            "--no-tui",
            "--no-git",
            "--no-deps",
        ],
    );
    assert_eq!(code, 0, "{text}{err}");
    assert!(
        text.contains("Accepted #1 (\"Add negate\") as aver/add-negate-")
            && text.contains(" to build on."),
        "{text}"
    );
    assert!(text.contains("Next: aver combine 1 2"), "{text}");
    let batch = BatchState::load(&d.home, &id).unwrap();
    let entry = &batch.tasks[1];
    assert_eq!(entry.history, std::slice::from_ref(&old));
    assert_ne!(entry.run, old);
    // The replaced run's attempts are abandoned: no longer anyone's notice.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let admin = d.admin_key();
    for a in &RunState::load(&d.home, &old).unwrap().agents {
        let attempt = rt
            .block_on(d.api().attempt(&admin.signing_key, &a.attempt))
            .unwrap();
        assert_eq!(attempt.state, aver::tasks::AttemptState::Abandoned);
    }
    let one = RunState::load(&d.home, &batch.tasks[0].run).unwrap();
    let two = RunState::load(&d.home, &entry.run).unwrap();
    let accepted = one.decided.as_ref().unwrap().commit.clone().unwrap();
    assert_eq!(
        two.base_commit, accepted,
        "task 2 starts on task 1's commit"
    );
    let stack = two.stacked_on.as_ref().unwrap();
    assert_eq!(
        stack.onto,
        StackOnto::Run {
            run: one.id.clone(),
            number: Some(1)
        }
    );
    assert_eq!(stack.root, batch.base);
    assert_eq!(two.batch.as_ref().unwrap().number, 2);
    assert_eq!(two.title, "Add double");

    // Task 2 alone carries task 1: both are verified on the result.
    let (code, json, err) = cli(&d, &repo, &["combine", "2", "--json"]);
    assert_eq!(code, 0, "{json}{err}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let branch = "aver/combine-add-negate-and-add-double";
    assert_eq!(v["status"], "exported", "{json}");
    assert_eq!(v["branch"], branch, "{json}");
    let verdicts: Vec<(u64, &str)> = v["verdicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["number"].as_u64().unwrap(),
                t["verdict"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(verdicts, [(1, "PASS"), (2, "PASS")], "{json}");
    assert_eq!(v["verdicts"][0]["via"], two.id, "{json}");
    // One head carries both tasks: the branch is its history, task 1's
    // commit then task 2's, on the base.
    assert_eq!(
        git_str(&repo, &["rev-parse", &format!("{branch}^")]),
        accepted
    );
    assert_eq!(git_str(&repo, &["rev-parse", &format!("{branch}~2")]), base);
    assert_eq!(
        git_str(&repo, &["show", &format!("{branch}:README.md")]),
        "calc\nl2\nnegate here + double\nl4\nl5"
    );
    assert!(git_str(&repo, &["show", &format!("{branch}:src/neg.js")]).contains("-x"));
    let log = git_str(&repo, &["log", "--format=%B", &format!("{base}..{branch}")]);
    assert_eq!(log.matches("Aver-Task: task_").count(), 2, "{log}");

    // Naming task 1 beside it merges task 1's change once, through task 2.
    let (code, text, err) = cli(&d, &repo, &["combine", "1", "2", "--branch", "aver/both"]);
    assert_eq!(code, 0, "{text}{err}");
    assert!(
        text.contains(&format!(
            "run {} is already in run {}'s work",
            one.id, two.id
        )),
        "{text}"
    );
    assert!(
        text.contains("Combined 2 runs as branch aver/both"),
        "{text}"
    );
    assert!(text.contains("#1 \"Add negate\": PASS"), "{text}");
    assert!(text.contains("#2 \"Add double\": PASS"), "{text}");
    assert_eq!(
        git_str(&repo, &["show", "aver/both:README.md"]),
        "calc\nl2\nnegate here + double\nl4\nl5"
    );
}

fn edit_neg(dir: &Path) {
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => 0 - x;\n").unwrap();
}

/// An input not accepted yet is merged and verified at one snapshot; an
/// edit to its directory before the accepts must not get its task accepted
/// at another snapshot than the branch holds: every input is checked
/// against its merged head before the first accept, and nothing is
/// accepted or exported.
#[test]
fn an_input_edited_after_the_merge_is_not_accepted_at_another_snapshot() {
    use aver::run::combine::{self, Combine, Input};
    let d = TestHome::start();
    let repo = calc(&d);
    let script = d.case_agent(
        "agent.sh",
        &[("Add negate", NEGATE), ("Add double", DOUBLE)],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks("Add negate\n  - neg: negate\nAdd double\n  - dbl: double\n"),
        &script,
        2,
    ));
    assert!(out.all_ready(), "{transcript}");
    let before = branches(&repo);
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (api, admin) = (d.api(), d.admin_key());
    let resolved: Vec<_> = [1, 2]
        .iter()
        .map(|n| {
            let run = b.run(&d.home, *n).unwrap();
            rt.block_on(combine::resolve(
                &api,
                &d.home,
                &admin,
                &Input::Best(run, 1),
            ))
            .unwrap()
        })
        .collect();
    let err = rt
        .block_on(
            Combine {
                api: &api,
                home: Some(&d.home),
                admin: &admin,
                branch: None,
                partial: false,
                batch: None,
                approve_review: false,
                before_accept: Some(edit_neg),
            }
            .run(resolved),
        )
        .expect_err("a moved input stops the combine");
    assert!(
        err.to_string()
            .contains("changed after it was merged and verified"),
        "{err}"
    );
    assert_eq!(branches(&repo), before, "nothing exported or accepted");
    for n in [1, 2] {
        assert!(b.run(&d.home, n).unwrap().decided.is_none(), "#{n}");
    }
}

/// `aver rerun --onto <combine branch>` builds on what the combine
/// verified: a tip amended since is refused, and once the branch is back
/// at the combine's commit the new run starts on the recorded snapshot.
#[test]
fn rerun_onto_an_amended_combine_branch_is_refused() {
    let d = TestHome::start();
    let repo = calc(&d);
    let script = d.case_agent(
        "agent.sh",
        &[
            ("Add negate", NEGATE),
            ("Add double", DOUBLE),
            ("Add triple", TRIPLE),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks("Add negate\n  - neg: negate\nAdd double\n  - dbl: double\nAdd triple\n  - tri: triple\n"),
        &script,
        3,
    ));
    assert!(out.all_ready(), "{transcript}");
    let branch = "aver/c";
    let (code, json, err) = cli(
        &d,
        &repo,
        &["combine", "1", "2", "--branch", branch, "--json"],
    );
    assert_eq!(code, 0, "{json}{err}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let exported = v["commit"].as_str().unwrap().to_owned();
    let rec = record::exported(&d.home, &exported).expect("the combine is recorded by commit");
    assert_eq!(rec.branch, branch);
    assert_eq!(rec.runs.len(), 2);

    // Amended: the same trailers, other code.
    let wt = d.home.root().join("amend");
    git_str(&repo, &["worktree", "add", wt.to_str().unwrap(), branch]);
    std::fs::write(wt.join("src/neg.js"), "module.exports = (x) => x;\n").unwrap();
    git_str(&wt, &["commit", "-a", "--amend", "--no-edit"]);
    let amended = git_str(&repo, &["rev-parse", branch]);
    assert_ne!(amended, exported);
    assert!(git_str(&repo, &["log", "-1", "--format=%B", branch]).contains("Aver-Task: "));
    let agent_cmd = format!("/bin/sh {}", script.display());
    let rerun = |d: &TestHome| {
        cli(
            d,
            &repo,
            &[
                "rerun",
                "3",
                "--onto",
                branch,
                "--agent-cmd",
                &agent_cmd,
                "--no-tui",
                "--no-git",
                "--no-deps",
            ],
        )
    };
    let (code, text, err) = rerun(&d);
    assert_ne!(code, 0, "{text}{err}");
    assert!(
        err.contains("was amended or moved since, so its tip is not verified work"),
        "{err}"
    );
    let id = out.state.id.clone();
    let three = BatchState::load(&d.home, &id).unwrap().tasks[2].clone();
    assert!(three.history.is_empty(), "no new run");

    // Back at the combine's commit: the run starts on the snapshot the
    // combine verified.
    git_str(
        &repo,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );
    git_str(&repo, &["branch", "-f", branch, &exported]);
    let (code, text, err) = rerun(&d);
    assert_eq!(code, 0, "{text}{err}");
    let run = RunState::load(
        &d.home,
        &BatchState::load(&d.home, &id).unwrap().tasks[2].run,
    )
    .unwrap();
    let stack = run.stacked_on.as_ref().unwrap();
    assert_eq!(stack.head, rec.snapshot);
    assert_eq!(stack.commit, exported);
    assert_eq!(run.base_commit, exported);
}

/// A repository with two existing tests, so two tasks can each change
/// what one expects (REVIEW).
fn review_repo(d: &TestHome) -> PathBuf {
    d.repo(
        "calc",
        &[
            ("aver.toml", &aver_toml()),
            ("src/math.js", "exports.add = (a, b) => a + b;\n"),
            ("src/mul.js", "exports.mul = (a, b) => a * b;\n"),
            ("test/math.test.json", r#"{"adds": "pass-if:src/math.js"}"#),
            ("test/mul.test.json", r#"{"muls": "pass-if:src/mul.js"}"#),
            ("README.md", README),
        ],
    )
}

const RENAME_ADD: &str = "rm src/math.js\nmkdir -p src test\n\
    echo 'exports.sum = (a, b) => a + b;' > src/sum.js\n\
    echo '{\"adds\": \"pass\"}' > test/math.test.json\n\
    echo '{\"[crit:sum] sums\": \"pass-if:src/sum.js\"}' > test/sum.test.json\n";

const RENAME_MUL: &str = "rm src/mul.js\nmkdir -p src test\n\
    echo 'exports.prod = (a, b) => a * b;' > src/prod.js\n\
    echo '{\"muls\": \"pass\"}' > test/mul.test.json\n\
    echo '{\"[crit:prod] multiplies\": \"pass-if:src/prod.js\"}' > test/prod.test.json\n";

/// Only a person approves REVIEW: `aver combine` and `aver rerun --onto`
/// accept a REVIEW task nobody accepted only with `--approve-review`, and
/// say so.
#[test]
fn combine_and_rerun_onto_accept_review_only_with_approve_review() {
    let d = TestHome::start();
    let repo = review_repo(&d);
    let script = d.case_agent(
        "agent.sh",
        &[
            ("Add negate", NEGATE),
            ("Rename add", RENAME_ADD),
            ("Rename mul", RENAME_MUL),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(
        &repo,
        tasks(
            "Add negate\n  - neg: negate\nRename add\n  - sum: sum\nRename mul\n  - prod: prod\n",
        ),
        &script,
        3,
    ));
    let rendered = out.summary.render(160);
    assert!(
        rendered.contains("#2 Rename add REVIEW"),
        "{rendered}{transcript}"
    );
    assert!(rendered.contains("#3 Rename mul REVIEW"), "{rendered}");
    let id = out.state.id.clone();
    let run = |n: u32| {
        BatchState::load(&d.home, &id)
            .unwrap()
            .run(&d.home, n)
            .unwrap()
    };
    let before = branches(&repo);

    // Combine: the REVIEW task is left out; nothing accepted or exported.
    let (code, text, err) = cli(&d, &repo, &["combine", "1", "2"]);
    assert_eq!(code, 1, "{text}{err}");
    assert!(
        text.contains("#2 (\"Rename add\") is REVIEW") && text.contains("only a person approves"),
        "{text}"
    );
    assert!(text.contains("aver combine 1 2 --approve-review"), "{text}");
    assert_eq!(branches(&repo), before);
    assert!(run(1).decided.is_none() && run(2).decided.is_none());

    // Rerun onto it: refused before anything is accepted.
    let agent_cmd = format!("/bin/sh {}", script.display());
    let rerun = |onto: &str, extra: &[&str]| {
        let mut args = vec![
            "rerun",
            "1",
            "--onto",
            onto,
            "--agent-cmd",
            &agent_cmd,
            "--no-tui",
            "--no-git",
            "--no-deps",
        ];
        args.extend_from_slice(extra);
        cli(&d, &repo, &args)
    };
    let (code, text, err) = rerun("3", &[]);
    assert_ne!(code, 0, "{text}{err}");
    assert!(
        err.contains("#3 (\"Rename mul\") needs review before others build on it")
            && err.contains("--approve-review"),
        "{err}"
    );
    assert!(run(3).decided.is_none());
    assert_eq!(branches(&repo), before);

    // With the flag, combine accepts it and says so.
    let (code, text, err) = cli(&d, &repo, &["combine", "1", "2", "--approve-review"]);
    assert_eq!(code, 0, "{text}{err}");
    assert!(
        text.contains("REVIEW approved by --approve-review: #2 (\"Rename add\")"),
        "{text}"
    );
    assert_eq!(
        run(2).decided.unwrap().verdict,
        Some(aver_core::Verdict::Review)
    );
    assert_eq!(
        run(1).decided.unwrap().verdict,
        Some(aver_core::Verdict::Pass)
    );

    // And rerun --onto accepts it and says so.
    let (code, text, err) = rerun("3", &["--approve-review"]);
    assert_eq!(code, 0, "{text}{err}");
    assert!(
        text.contains("Accepted #3 (\"Rename mul\") as aver/rename-mul-")
            && text.contains(" to build on.")
            && text.contains("accepted because you passed --approve-review"),
        "{text}"
    );
    assert_eq!(
        run(3).decided.unwrap().verdict,
        Some(aver_core::Verdict::Review)
    );
}
