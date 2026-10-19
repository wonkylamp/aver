//! Batches (`aver run --tasks`, `--issue`, `--issues`) with scripted fake
//! agents and the scripted runner: a
//! batch of four tasks with mixed outcomes and its summary, the overlap
//! between two of them, `aver accept --all-pass`, the queue under
//! `--parallel 2` with a queued task hearing about an earlier one,
//! `aver combine --all-pass`, `aver status`, and issues read with a stub
//! `gh`, answered with `--feedback`. No Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};

use aver::run::batch::{self, input, BatchSpec, BatchState};
use aver::run::state::RunState;
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

/// An agent that picks what to do from the task in its brief.
fn agent(d: &TestHome, cases: &[(&str, &str)]) -> PathBuf {
    d.case_agent("agent.sh", cases)
}

fn spec(repo: &Path, tasks: Vec<input::BatchTask>, script: &Path, parallel: u32) -> BatchSpec {
    batch_spec(repo, tasks, script, parallel)
}

fn run(d: &TestHome, spec: BatchSpec) -> (batch::BatchOutcome, String) {
    d.run_batch(spec)
}

fn cli(d: &TestHome, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = aver(&d.home, cwd, args, b"");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

const NEGATE: &str = "mkdir -p src test\n\
    echo 'module.exports = (x) => -x;' > src/neg.js\n\
    echo '{\"[crit:neg] negates\": \"pass-if:src/neg.js\"}' > test/neg.test.json\n";

#[test]
fn a_batch_of_four_tasks_ends_with_one_summary() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add double\n  - dbl: double(x) returns 2x\n\
         Rename add\n  - sum: sum(a, b) adds\n\
         Add triple\n  - tri: triple(x) returns 3x\n",
    )
    .unwrap();
    let script = agent(
        &d,
        &[
            (
                "Add negate",
                &format!("{NEGATE}printf 'calc with negate\\nl2\\nl3\\nl4\\nl5\\n' > README.md\naver verify || true"),
            ),
            // Breaks the existing test by deleting what it needs.
            (
                "Add double",
                "rm src/math.js\nmkdir -p src test\n\
                 echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
                 echo '{\"[crit:dbl] doubles\": \"pass-if:src/dbl.js\"}' > test/dbl.test.json",
            ),
            // Rewrites the existing test's expectation.
            (
                "Rename add",
                "rm src/math.js\nmkdir -p src test\n\
                 echo 'exports.sum = (a, b) => a + b;' > src/sum.js\n\
                 echo '{\"adds\": \"pass\"}' > test/math.test.json\n\
                 echo '{\"[crit:sum] sums\": \"pass-if:src/sum.js\"}' > test/sum.test.json",
            ),
            // A tagged test that passes without the change proves nothing.
            (
                "Add triple",
                "mkdir -p src test\n\
                 echo 'module.exports = (x) => 3 * x;' > src/tri.js\n\
                 echo '{\"[crit:tri] triples\": \"pass\"}' > test/tri.test.json\n\
                 printf 'calc with triple\\nl2\\nl3\\nl4\\nl5\\n' > README.md",
            ),
        ],
    );
    let (out, transcript) = run(&d, spec(&repo, tasks, &script, 4));
    assert_eq!(
        out.summary.render(160),
        "4 tasks · 1 ready · 1 broke something · 1 needs review · 1 unproven\n\
         #1 Add negate PASS     +3 -1    aver accept 1\n\
         #2 Add double FAIL_OWN broke \"adds\"\n\
         #3 Rename add REVIEW   changed 1 test expectation: aver diff 3\n\
         #4 Add triple UNPROVEN not proven: tri (passes_without_change)\n\
         overlap: #1 and #4 both change README.md (overlapping lines)\n\
         overlap: #2 and #3 both change src/math.js (the same file)\n\
         Next: aver accept --all-pass [--pr]   aver diff <n>   aver accept <n>   aver discard\n",
        "{transcript}"
    );
    assert!(!out.all_ready());
    assert!(
        transcript.contains("#1.1 custom  started"),
        "events name the task and the agent: {transcript}"
    );
    assert!(
        transcript.contains("#2 finished: FAIL_OWN broke \"adds\""),
        "{transcript}"
    );
    // Every task is an ordinary run that knows its batch.
    let saved = BatchState::load(&d.home, &out.state.id).unwrap();
    assert!(saved.finished_at.is_some());
    assert_eq!(saved.tasks.len(), 4);
    for t in &saved.tasks {
        let r = RunState::load(&d.home, &t.run).unwrap();
        let b = r.batch.as_ref().unwrap();
        assert_eq!((b.id.as_str(), b.number), (out.state.id.as_str(), t.number));
        assert_eq!(r.base_commit, saved.base_commit, "one base for every task");
        assert_eq!(r.title, t.title);
    }

    // `aver status` prints the summary again from the files.
    let (code, status, err) = cli(&d, &repo, &["status"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        status.contains("4 tasks · 1 ready · 1 broke something · 1 needs review · 1 unproven"),
        "{status}"
    );
    assert!(
        status.contains("overlap: #1 and #4 both change README.md"),
        "{status}"
    );
    // A task number names that task's run; a missing one says what exists.
    let (code, third, err) = cli(&d, &repo, &["status", "3"]);
    assert_eq!(code, 0, "{third}{err}");
    assert!(third.contains("· task #3 of batch"), "{third}");
    let (code, _, err) = cli(&d, &repo, &["status", "#9"]);
    assert_ne!(code, 0);
    assert!(
        err.contains("no task #9 in batch") && err.contains("tasks are 1–4"),
        "{err}"
    );

    // Only the passing task is accepted, as its own branch.
    let (code, accepted, err) = cli(&d, &repo, &["accept", "--all-pass"]);
    assert_eq!(code, 0, "{accepted}{err}");
    assert!(
        accepted.contains("#1 accepted as aver/add-negate"),
        "{accepted}"
    );
    assert!(
        accepted.contains("#2 not accepted: FAIL_OWN broke \"adds\""),
        "{accepted}"
    );
    let branches = git_str(&repo, &["branch", "--list", "aver/*"]);
    let negate = branches.trim();
    assert!(
        negate.starts_with("aver/add-negate-") && negate.len() == "aver/add-negate-".len() + 4,
        "one branch, the title's slug and a short id: {branches}"
    );
    assert_eq!(
        git_str(&repo, &["show", &format!("{negate}:src/neg.js")]),
        "module.exports = (x) => -x;"
    );
    // A REVIEW waits for a person's explicit approval (B5).
    let (code, one, err) = cli(&d, &repo, &["accept", "3"]);
    assert_eq!(code, 2, "{one}{err}");
    assert!(err.contains("aver accept 3 --approve-review"), "{err}");
    let (code, one, err) = cli(&d, &repo, &["accept", "3", "--approve-review"]);
    assert_eq!(code, 0, "{one}{err}");
    assert!(one.contains("Accepted task #3 (attempt 1 of run"), "{one}");
    let (_, again, _) = cli(&d, &repo, &["status"]);
    assert!(
        again.contains("#1 Add negate PASS     accepted as aver/add-negate"),
        "{again}"
    );
    assert!(
        again.contains("REVIEW   accepted as aver/rename-add"),
        "{again}"
    );

    // `aver status --all` lists the batch and each of its runs.
    let (code, all, err) = cli(&d, &repo, &["status", "--all", "--json"]);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&all).unwrap();
    assert_eq!(v["batches"].as_array().unwrap().len(), 1);
    assert_eq!(v["runs"].as_array().unwrap().len(), 4);
    let outcomes: Vec<&str> = v["batches"][0]["summary"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["ready", "broke", "review", "unproven"]);
    let (_, all_text, _) = cli(&d, &repo, &["status", "--all"]);
    assert!(
        all_text.contains(&format!("batch {}", out.state.id)),
        "{all_text}"
    );
    assert!(all_text.contains("FAIL_OWN"), "{all_text}");
}

#[test]
fn parallel_two_queues_the_third_task_which_hears_about_the_first() {
    let d = TestHome::start();
    let repo = calc(&d);
    let root = d.root.clone();
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add double\n  - dbl: double(x) returns 2x\n\
         Add triple\n  - tri: triple(x) returns 3x\n",
    )
    .unwrap();
    let script = agent(
        &d,
        &[
            (
                "Add negate",
                &format!("{NEGATE}printf 'calc\\nl2 negate\\nl3\\nl4\\nl5\\n' > README.md"),
            ),
            (
                "Add double",
                "sleep 2\nmkdir -p src test\n\
                 echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
                 echo '{\"[crit:dbl] doubles\": \"pass-if:src/dbl.js\"}' > test/dbl.test.json",
            ),
            (
                "Add triple",
                &format!(
                    "aver notices > {}/notices-3.txt\nmkdir -p src test\n\
                     echo 'module.exports = (x) => 3 * x;' > src/tri.js\n\
                     echo '{{\"[crit:tri] triples\": \"pass-if:src/tri.js\"}}' > test/tri.test.json\n\
                     printf 'calc\\nl2\\nl3\\nl4\\nl5 triple\\n' > README.md",
                    root.display()
                ),
            ),
        ],
    );
    let (out, transcript) = run(&d, spec(&repo, tasks, &script, 2));
    assert!(out.all_ready(), "{transcript}");
    // Never more than two tasks at once, and the third waited.
    let mut running = 0;
    let mut most = 0;
    let mut order = Vec::new();
    for line in transcript.lines() {
        if let Some((_, rest)) = line.split_once("] ") {
            if rest.contains(" started as run ") {
                running += 1;
                most = most.max(running);
                order.push(rest[..2].to_owned());
            } else if rest.contains(" finished: ") {
                running -= 1;
                order.push(format!("{} done", &rest[..2]));
            }
        }
    }
    assert_eq!(most, 2, "{transcript}");
    assert_eq!(
        order[..4],
        ["#1", "#2", "#1 done", "#3"],
        "the third task waits for a free slot: {transcript}"
    );
    assert_eq!(order.len(), 6, "{transcript}");
    let notices = std::fs::read_to_string(root.join("notices-3.txt")).unwrap();
    assert!(notices.contains("task \"Add negate\" changes"), "{notices}");
    assert!(notices.contains("README.md"), "{notices}");
    assert_eq!(
        out.summary.collisions.len(),
        1,
        "{:?}",
        out.summary.collisions
    );
    assert_eq!(out.summary.collisions[0].tasks, [1, 3]);
    let text = out.summary.render(160);
    assert!(
        text.contains("overlap: #1 and #3 both change README.md (different lines)"),
        "{text}"
    );
    assert!(
        text.contains("Next: aver accept --all-pass [--pr]   aver combine --all-pass"),
        "{text}"
    );

    // Every passing task accepted, then merged and verified against each.
    let base = git_str(&repo, &["rev-parse", "HEAD"]);
    let (code, combined, err) = cli(&d, &repo, &["combine", "--all-pass"]);
    assert_eq!(code, 0, "{combined}{err}");
    assert!(
        combined.contains("#1 accepted as aver/add-negate"),
        "{combined}"
    );
    assert!(combined.contains("Combined 3 runs as branch"), "{combined}");
    // A slug over 40 characters is cut at a word and carries the run id.
    let branch = combined
        .split("runs as branch ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap();
    assert!(
        branch.starts_with("aver/combine-add-negate-and-add-double-and-"),
        "{branch}"
    );
    assert_eq!(
        branch.len(),
        "aver/combine-add-negate-and-add-double-and-".len() + 6,
        "{branch}"
    );
    assert_eq!(git_str(&repo, &["rev-parse", &format!("{branch}^")]), base);
    assert_eq!(
        git_str(&repo, &["show", &format!("{branch}:README.md")]),
        "calc\nl2 negate\nl3\nl4\nl5 triple"
    );
}

#[test]
fn issues_come_from_gh_and_their_agents_answer_feedback() {
    let d = TestHome::start();
    let repo = calc(&d);
    let root = d.root.clone();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let issue = |n: u64, title: &str, crit: &str| {
        serde_json::json!({
            "number": n,
            "title": title,
            "body": format!("Please fix.\n\n```aver\n- {crit}\n```\n"),
        })
    };
    std::fs::write(
        root.join("issue-41.json"),
        issue(41, "Add negate", "neg: negate(x) returns -x").to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("list.json"),
        serde_json::json!([
            issue(42, "Add double", "dbl: double(x) returns 2x"),
            issue(41, "Add negate", "neg: negate(x) returns -x"),
        ])
        .to_string(),
    )
    .unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" >> {root}/gh.log\n\
             case \"$1 $2\" in\n\
             'issue view') cat {root}/issue-$3.json ;;\n\
             'issue list') cat {root}/list.json ;;\n\
             'pr create') cat \"$8\" >> {root}/prs.md; echo \"https://github.example/pull/$4\" ;;\n\
             *) exit 1 ;;\n\
             esac\n",
            root = root.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let script = agent(
        &d,
        &[
            ("issue #41", NEGATE),
            (
                "issue #42",
                "mkdir -p src test\n\
                 echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
                 echo '{\"[crit:dbl] doubles\": \"pass-if:src/dbl.js\"}' > test/dbl.test.json",
            ),
        ],
    );
    let answer = d.script(
        "answer.sh",
        "printf 'helped: the summary\\nRATING: 9/10\\n'\n",
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = std::process::Command::new(aver_bin())
        .args([
            "run",
            "--issue",
            "41",
            "--issues",
            "bug",
            "--agent-cmd",
            &format!("/bin/sh {}", script.display()),
            "--resume-cmd",
            &format!("/bin/sh {} {{feedback}}", answer.display()),
            "--feedback",
            "--no-deps",
            "--no-tui",
            "--parallel",
            "2",
        ])
        .current_dir(&repo)
        .env("AVER_HOME", d.home.root())
        .env("PATH", &path)
        .env_remove("AVER_URL")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let log = std::fs::read_to_string(root.join("gh.log")).unwrap();
    assert!(
        log.contains("issue view 41 --json number,title,body"),
        "{log}"
    );
    assert!(log.contains("issue list --label bug --state open"), "{log}");
    assert!(
        stdout.contains("2 tasks · 2 ready"),
        "issue 41 is listed once: {stdout}"
    );
    assert!(stdout.contains("#41 Add negate PASS"), "{stdout}");
    assert!(stdout.contains("#42 Add double PASS"), "{stdout}");
    assert!(stdout.contains("feedback #41.1 rating 9/10"), "{stdout}");
    assert!(stdout.contains("feedback #42.1 rating 9/10"), "{stdout}");
    let b = BatchState::current(&d.home, None).unwrap();
    let numbers: Vec<(u32, Option<u64>)> = b.tasks.iter().map(|t| (t.number, t.issue)).collect();
    assert_eq!(numbers, [(41, Some(41)), (42, Some(42))]);
    let r = b.run(&d.home, 41).unwrap();
    assert!(r.agents[0].feedback.as_ref().unwrap().rating == Some(9));

    // `aver accept --all-pass --pr`: a branch named after each issue,
    // pushed, and a pull request that closes the issue.
    let remote = root.join("remote.git");
    git_support::git(&root, &["init", "--quiet", "--bare", "remote.git"], b"");
    git_support::git(
        &repo,
        &["remote", "add", "origin", &remote.display().to_string()],
        b"",
    );
    let out = std::process::Command::new(aver_bin())
        .args(["accept", "--all-pass", "--pr"])
        .current_dir(&repo)
        .env("AVER_HOME", d.home.root())
        .env("PATH", &path)
        .env_remove("AVER_URL")
        .output()
        .unwrap();
    let accepted = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{accepted}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        accepted.contains("#41 accepted as aver/issue-41-add-negate-"),
        "{accepted}"
    );
    assert!(
        accepted.contains(", PR https://github.example/pull/aver/issue-42-add-double-"),
        "{accepted}"
    );
    let pushed = git_str(&remote, &["branch", "--list", "aver/*"]);
    let pushed: Vec<&str> = pushed.lines().map(str::trim).collect();
    assert!(
        pushed.len() == 2
            && pushed[0].starts_with("aver/issue-41-add-negate-")
            && pushed[1].starts_with("aver/issue-42-add-double-"),
        "{pushed:?}"
    );
    let prs = std::fs::read_to_string(root.join("prs.md")).unwrap();
    assert!(prs.starts_with("Fixes #41.\n\nAdd negate\n"), "{prs}");
    assert!(prs.contains("Fixes #42.\n\nAdd double\n"), "{prs}");
    let r = b.run(&d.home, 42).unwrap();
    assert_eq!(
        r.decided.unwrap().pr,
        Some(format!("https://github.example/pull/{}", pushed[1]))
    );
}

#[test]
fn a_retry_requeues_a_failing_task_and_tells_it_what_went_wrong() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks("Add negate\n  - neg: negate(x) returns -x\n").unwrap();
    // The first attempt breaks the existing test; the retry, told so in its
    // brief, does not.
    let script = agent(
        &d,
        &[(
            "Add negate",
            &format!(
                "if ! grep -q 'PREVIOUS ATTEMPT' .aver/BRIEF.md; then rm src/math.js; fi\n{NEGATE}"
            ),
        )],
    );
    let mut s = spec(&repo, tasks, &script, 1);
    s.limits.retries = 1;
    s.limits.retry_on = vec![batch::RetryOn::FailOwn];
    let (out, transcript) = run(&d, s);
    assert!(out.all_ready(), "{transcript}");
    assert!(
        transcript.contains("#1 finished: FAIL_OWN broke \"adds\""),
        "{transcript}"
    );
    assert!(
        transcript.contains("#1 retrying (1 of 1) as run "),
        "{transcript}"
    );
    assert!(transcript.contains(": fail-own"), "{transcript}");
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    let entry = &b.tasks[0];
    assert_eq!((entry.retries, entry.history.len()), (1, 1));
    let first = RunState::load(&d.home, &entry.history[0]).unwrap();
    let second = b.run(&d.home, 1).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(first.base_commit, second.base_commit, "same base");
    let brief = std::fs::read_to_string(second.agents[0].dir.join(".aver/BRIEF.md")).unwrap();
    assert!(
        brief.contains(&format!(
            "PREVIOUS ATTEMPT: run {} worked on this task",
            first.id
        )),
        "{brief}"
    );
    assert!(
        brief.contains("```\noutcome: FAIL_OWN broke \"adds\""),
        "{brief}"
    );
    assert!(brief.contains("NEW failure: "), "{brief}");

    // Without fail-own in --retry-on, nothing is retried.
    let d2 = TestHome::start();
    let repo2 = calc(&d2);
    let script2 = d2.case_agent("agent.sh", &[("Add negate", "rm src/math.js")]);
    let mut s = spec(
        &repo2,
        input::parse_tasks("Add negate\n").unwrap(),
        &script2,
        1,
    );
    s.limits.retries = 2;
    let (out, transcript) = run(&d2, s);
    assert!(!transcript.contains("retrying"), "{transcript}");
    assert_eq!(out.state.tasks[0].retries, 0);
}

#[test]
fn a_spent_verify_budget_keeps_the_remaining_tasks_from_starting() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add double\n  - dbl: double(x) returns 2x\n\
         Add triple\n  - tri: triple(x) returns 3x\n",
    )
    .unwrap();
    let script = agent(&d, &[("Add negate", NEGATE)]);
    let mut s = spec(&repo, tasks, &script, 1);
    s.limits.verifies = Some(1);
    let (out, transcript) = run(&d, s);
    let text = out.summary.render(160);
    assert!(
        text.starts_with("3 tasks · 1 ready · 2 over budget\nspent $0.00 · 1/1 verifies · "),
        "{text}"
    );
    assert!(
        text.contains("#2 Add double budget   the batch used 1 of 1 verifies"),
        "{text}"
    );
    assert!(text.contains("aver rerun <n>"), "{text}");
    assert!(!transcript.contains("#2 started"), "{transcript}");
    assert!(
        transcript.contains("batch: the batch used 1 of 1 verifies: no more tasks start"),
        "{transcript}"
    );
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    assert_eq!(b.spent.verifies, 1);
    assert_eq!(
        b.over_budget.as_deref(),
        Some("the batch used 1 of 1 verifies")
    );
}

#[test]
fn a_spent_money_budget_stops_the_running_agents() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add double\n  - dbl: double(x) returns 2x\n",
    )
    .unwrap();
    // A custom agent reports its cost in .aver/usage.json, as its brief
    // says, then would work on for a minute.
    let script = agent(
        &d,
        &[(
            "Add negate",
            &format!(
                "{NEGATE}grep -q usage.json .aver/BRIEF.md\n\
                 echo '{{\"cost_usd\": 3.5, \"input_tokens\": 1000}}' > .aver/usage.json\nsleep 60"
            ),
        )],
    );
    let mut s = spec(&repo, tasks, &script, 1);
    s.limits.usd = Some(2.0);
    let started = std::time::Instant::now();
    let (out, transcript) = run(&d, s);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(40),
        "stopped early: {transcript}"
    );
    let text = out.summary.render(160);
    assert!(
        text.starts_with("2 tasks · 2 over budget\nspent $3.50/$2.00 · "),
        "{text}"
    );
    assert!(
        text.contains("over budget: the batch spent $3.50 of $2.00"),
        "{text}"
    );
    assert!(
        transcript.contains("stopping the running agents"),
        "{transcript}"
    );
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    let r = b.run(&d.home, 1).unwrap();
    assert_eq!(r.agents[0].usage.cost_usd, Some(3.5));
    assert!(r.agents[0].dir.exists(), "its directory stays for a rerun");
    assert!(b.run(&d.home, 2).is_none());
}
