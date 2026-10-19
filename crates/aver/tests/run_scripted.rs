//! `aver run` with scripted fake agents (shell scripts that edit files and
//! call `aver verify`) with the scripted runner: ranking, the supervisor's verify and gate, resume, timeouts,
//! the Stop hook, `aver accept`, and the Git view. No Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use aver::keys::AgentKey;
use aver::run::accept::{self, AcceptOptions};
use aver::run::agent::Template;
use aver::run::compare;
use aver::run::state::{AgentState, Budget, RunState};
use aver::run::supervisor::{RunOutcome, RunSpec, Supervisor};
use aver::run::view::Lines;
use aver::workspace::MaterializeOptions;
use aver_core::Verdict;
use git_support::git_str;
use run_support::*;

const GOOD: &str = r#"mkdir -p src test
echo 'module.exports = (x) => -x;' > src/neg.js
echo '{"[crit:neg] negates": "pass-if:src/neg.js"}' > test/neg.test.json
echo "implemented negate"
"#;

/// A tagged test that passes without the change: proves nothing.
const VACUOUS: &str = r#"mkdir -p src test
echo 'module.exports = (x) => -x;' > src/neg.js
echo '{"[crit:neg] negates": "pass"}' > test/neg.test.json
echo "wrote a test that always passes"
"#;

/// Breaks the existing test by deleting what it needs.
const BREAKS: &str = r#"rm src/math.js
mkdir -p src test
echo 'module.exports = (x) => -x;' > src/neg.js
echo '{"[crit:neg] negates": "pass-if:src/neg.js"}' > test/neg.test.json
echo "rewrote math"
"#;

fn admin(d: &TestHome) -> AgentKey {
    AgentKey::read(&d.home.key_path(&"admin".parse().unwrap())).unwrap()
}

fn spec(repo: &Path, spawn: &str, resume: Option<&str>, n: u32) -> RunSpec {
    RunSpec {
        repo: repo.to_owned(),
        rev: "HEAD".into(),
        prompt: "Add negate".into(),
        title: None,
        criteria: vec![("neg".into(), "negate(x) returns -x".into())],
        protected: Vec::new(),
        agents: (0..n)
            .map(|_| Template::Custom {
                spawn: spawn.into(),
                resume: resume.map(str::to_owned),
            })
            .collect(),
        budget: Budget {
            timeout_ms: 60_000,
            idle_ms: 60_000,
            max_verifies: 4,
            max_usd: None,
        },
        deps: false,
        git_view: true,
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

fn run(d: &TestHome, spec: RunSpec) -> (RunOutcome, String) {
    let stop = AtomicBool::new(false);
    let mut view = Lines::new(Vec::new());
    let outcome = Supervisor::new(&d.home, d.api(), admin(d), &mut view, &stop)
        .unwrap()
        .run(spec)
        .unwrap();
    let text = String::from_utf8(view.into_inner()).unwrap();
    (outcome, text)
}

/// The event lines of a line-mode transcript, without their times.
fn events(transcript: &str) -> Vec<String> {
    transcript
        .lines()
        .filter_map(|l| l.strip_prefix('[').and_then(|l| l.split_once("] ")))
        .map(|(_, rest)| rest.to_owned())
        .collect()
}

fn sh(script: &Path, extra: &str) -> String {
    format!("/bin/sh {}{extra}", script.display())
}

#[test]
fn pass_unproven_and_fail_own_rank_in_that_order() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script(
        "agent.sh",
        &format!(
            "case \"$AVER_AGENT_INDEX\" in\n1) {VACUOUS} ;;\n2) {GOOD} ;;\n3) {BREAKS} ;;\nesac\naver verify || true\n"
        ),
    );
    let (out, transcript) = run(&d, spec(&repo, &sh(&script, ""), None, 3));
    let order: Vec<(u32, Option<Verdict>)> = compare::ranked(&out.compared)
        .iter()
        .map(|(_, c)| (c.agent.index, c.verdict))
        .collect();
    assert_eq!(
        order,
        vec![
            (2, Some(Verdict::Pass)),
            (1, Some(Verdict::Unproven)),
            (3, Some(Verdict::FailOwn)),
        ],
        "{transcript}"
    );
    let states: Vec<AgentState> = out.state.agents.iter().map(|a| a.state).collect();
    assert_eq!(
        states,
        vec![AgentState::Stopped, AgentState::Done, AgentState::Stopped]
    );
    let table = compare::table(&out.compared);
    assert!(table.contains("neg: passes_without_change"), "{table}");
    assert!(compare::hints(&out.compared).contains("aver accept 2"));
    assert!(out.any_pass());
    // Every agent ran in its own directory, under the run's id.
    for a in &out.state.agents {
        assert!(
            a.dir
                .ends_with(format!("work/calc/{}-{}", out.state.id, a.index)),
            "{}",
            a.dir.display()
        );
        assert!(a.last_action.is_some());
    }
    let saved = RunState::load(&d.home, &out.state.id).unwrap();
    assert!(saved.finished_at.is_some());
}

#[test]
fn the_supervisor_verifies_an_agent_that_never_did() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("lazy.sh", GOOD);
    let (out, transcript) = run(&d, spec(&repo, &sh(&script, ""), None, 1));
    let c = &out.compared[0];
    assert_eq!(c.verdict, Some(Verdict::Pass), "{transcript}");
    assert_eq!(c.verifies, 1);
    assert_eq!(out.state.agents[0].state, AgentState::Done);
    assert_eq!(c.diff.map(|d| d.0), Some(2));
}

/// Also the line-mode golden transcript.
#[test]
fn the_gate_resumes_through_the_resume_template() {
    let d = TestHome::start();
    let repo = d.calc();
    let first = d.script("first.sh", VACUOUS);
    let feedback = d.root.join("feedback.txt");
    let fix = d.script(
        "fix.sh",
        &format!("printf '%s' \"$1\" > {}\n{GOOD}", feedback.display()),
    );
    let (out, transcript) = run(
        &d,
        spec(&repo, &sh(&first, ""), Some(&sh(&fix, " {feedback}")), 1),
    );
    assert_eq!(
        events(&transcript),
        vec![
            "#1 custom  started",
            "#1 custom  exited 0",
            "#1 custom  verified UNPROVEN 0/1",
            "#1 custom  resumed with the report (1)",
            "#1 custom  exited 0",
            "#1 custom  verified PASS 1/1",
            "#1 custom  done: PASS 1/1",
        ],
        "{transcript}"
    );
    let a = &out.state.agents[0];
    assert_eq!((a.state, a.resumes), (AgentState::Done, 1));
    let text = std::fs::read_to_string(&feedback).unwrap();
    assert!(text.starts_with("NOT DONE: UNPROVEN."), "{text}");
    assert!(
        text.contains("  neg  NOT PROVEN (passes_without_change)"),
        "{text}"
    );
    assert!(text.contains("Verifies: 1 of 4 used, 3 left."), "{text}");
}

#[test]
fn a_timeout_kills_the_whole_process_group_crit_linux_green() {
    let d = TestHome::start();
    let repo = d.calc();
    let pid_file = d.root.join("child.pid");
    let script = d.script(
        "slow.sh",
        &format!("sleep 60 &\necho $! > {}\nwait\n", pid_file.display()),
    );
    let mut s = spec(&repo, &sh(&script, ""), None, 1);
    s.budget.timeout_ms = 1500;
    let (out, transcript) = run(&d, s);
    assert_eq!(
        out.state.agents[0].state,
        AgentState::Timeout,
        "{transcript}"
    );
    assert!(transcript.contains("timed out after"), "{transcript}");
    let child: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        !aver::run::state::process_alive(child),
        "the agent's child process survived the timeout"
    );
}

/// `aver verify --last` reprints the latest receipt's report: the same
/// verdict first and last, no new receipt (the verifies used stay), the
/// receipt's exit code, and a stale mark once the directory changes.
#[test]
fn verify_last_reprints_the_latest_report_without_verifying() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("vacuous.sh", VACUOUS);
    let (out, _) = run(&d, spec(&repo, &sh(&script, ""), None, 1));
    let dir = out.state.agents[0].dir.clone();
    let last = || {
        let o = aver(&d.home, &dir, &["verify", "--last"], b"");
        assert_eq!(
            o.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    };
    let first = last();
    assert!(first.starts_with("NOT DONE: UNPROVEN."), "{first}");
    let end = first.lines().last().unwrap().to_owned();
    assert!(
        end.starts_with("NOT DONE: UNPROVEN (1 of 4 verifies used)"),
        "{end}"
    );
    assert!(!first.contains("STALE"), "{first}");
    let again = last();
    assert_eq!(again.lines().last().unwrap(), end, "no verify was spent");
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => 0 - x;\n").unwrap();
    let stale = last();
    assert!(
        stale
            .lines()
            .nth(1)
            .is_some_and(|l| l == aver::run::report::STALE_LINE),
        "{stale}"
    );
    assert!(stale.trim_end().ends_with("(stale)"), "{stale}");
}

#[test]
fn the_stop_hook_blocks_twice_then_lets_the_agent_stop() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("vacuous.sh", VACUOUS);
    let (out, _) = run(&d, spec(&repo, &sh(&script, ""), None, 1));
    let dir = out.state.agents[0].dir.clone();
    // Each stop ends a different turn; identical inputs would be one event
    // to the hook's claim.
    let turn = std::cell::Cell::new(0);
    let input = |active: bool| {
        turn.set(turn.get() + 1);
        format!(
            r#"{{"session_id":"s","stop_hook_active":{active},"last_assistant_message":"turn {}","cwd":"{}"}}"#,
            turn.get(),
            dir.display()
        )
    };
    let hook = |active: bool| {
        let rev = aver::integrate::INTEGRATION_REV.to_string();
        let o = aver(
            &d.home,
            &dir,
            &["hook", "stop", "--rev", &rev],
            input(active).as_bytes(),
        );
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    let first = hook(false);
    let v: serde_json::Value = serde_json::from_str(first.trim()).unwrap();
    assert_eq!(v["decision"], "block");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.contains("  neg  NOT PROVEN (passes_without_change)"),
        "{reason}"
    );
    assert!(hook(true).contains("\"block\""));
    assert_eq!(hook(true), "", "the third consecutive stop is allowed");
    // Not a run workspace (no .aver/run.toml): the worktree gate, which
    // verifies nothing, blocks once because the last verify of these files
    // was not a pass, then allows.
    std::fs::remove_file(dir.join(".aver/run.toml")).unwrap();
    let gate = hook(false);
    assert!(gate.contains("said UNPROVEN"), "{gate}");
    assert_eq!(hook(false), "");
}

#[test]
fn accept_exports_a_branch_whose_parent_is_the_real_base() {
    let d = TestHome::start();
    let repo = d.calc();
    let base = git_str(&repo, &["rev-parse", "HEAD"]);
    let good = d.script("good.sh", GOOD);
    let vacuous = d.script("vacuous.sh", VACUOUS);
    let script = d.script(
        "agent.sh",
        &format!(
            "if [ \"$AVER_AGENT_INDEX\" = 1 ]; then exec /bin/sh {}; else exec /bin/sh {}; fi\n",
            good.display(),
            vacuous.display()
        ),
    );
    let (mut out, _) = run(&d, spec(&repo, &sh(&script, ""), None, 2));
    let loser = out.state.agents[1].dir.clone();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let refused = rt.block_on(accept::accept(
        &d.api(),
        &d.home,
        &admin(&d),
        &mut out.state,
        "2",
        &AcceptOptions::default(),
    ));
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("UNPROVEN, not PASS"),
        "a non-PASS attempt needs --force"
    );
    let checked_out = rt.block_on(accept::accept(
        &d.api(),
        &d.home,
        &admin(&d),
        &mut out.state,
        "1",
        &AcceptOptions {
            branch: Some("main".into()),
            ..Default::default()
        },
    ));
    assert!(
        checked_out.unwrap_err().to_string().contains("checked out"),
        "never the checked-out branch"
    );
    let accepted = rt
        .block_on(accept::accept(
            &d.api(),
            &d.home,
            &admin(&d),
            &mut out.state,
            "1",
            &AcceptOptions::default(),
        ))
        .unwrap();
    let id = out.state.id.as_str();
    let branch = format!("aver/add-negate-{}", id[id.len() - 4..].to_lowercase());
    assert_eq!(
        accepted.branch, branch,
        "the title's slug and the run id's end"
    );
    assert_eq!(accepted.verdict, Verdict::Pass);
    assert_eq!(
        git_str(&repo, &["rev-parse", &format!("{branch}^")]),
        base,
        "the branch is a child of the real base commit"
    );
    assert_eq!(
        git_str(&repo, &["show", &format!("{branch}:src/neg.js")]),
        "module.exports = (x) => -x;"
    );
    let message = git_str(&repo, &["log", "-1", "--format=%B", &branch]);
    assert!(message.starts_with("Add negate\n"), "{message}");
    assert!(message.contains("Aver-Receipt: rcpt_"), "{message}");
    assert!(message.contains("\nAver-Attempt: att_"), "{message}");
    assert!(
        message.contains(&format!(
            "\nAver-Agent: custom #1 of aver run {} (agent_",
            out.state.id
        )),
        "{message}"
    );
    assert_eq!(
        git_str(
            &repo,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", &branch]
        ),
        "Repo Owner <owner@example.com>|Repo Owner <owner@example.com>",
        "the branch is authored and committed as the repository's Git identity"
    );
    // The user's checkout is untouched.
    assert_eq!(
        git_str(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );
    assert_eq!(git_str(&repo, &["rev-parse", "HEAD"]), base);
    assert_eq!(git_str(&repo, &["status", "--porcelain"]), "");
    // The loser's directory is gone, the decision recorded.
    assert!(!loser.exists());
    let saved = RunState::load(&d.home, &out.state.id).unwrap();
    assert_eq!(
        saved.decided.unwrap().branch.as_deref(),
        Some(branch.as_str())
    );
    // A second accept of the same branch is refused rather than rewritten.
    let again = rt.block_on(accept::accept(
        &d.api(),
        &d.home,
        &admin(&d),
        &mut out.state,
        "1",
        &AcceptOptions::default(),
    ));
    assert!(again.unwrap_err().to_string().contains("already accepted"));
}

/// A workspace with a Git view on `repo`'s HEAD.
fn viewed(d: &TestHome, repo: &Path) -> (PathBuf, AgentKey, aver_core::WorkspaceId) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let api = d.api();
    let admin = admin(d);
    let imported = rt
        .block_on(api.import(&admin.signing_key, repo, "HEAD"))
        .unwrap();
    let body = aver_core::TaskBody {
        schema: aver_core::TaskSchema::V2,
        title: "View".parse().unwrap(),
        intent: "View".parse().unwrap(),
        base: imported.snapshot,
        target: None,
        criteria: aver_core::Criteria::new(Vec::new()).unwrap(),
        checks: aver_core::Set::empty(),
        protected_paths: aver_core::Set::empty(),
        author: admin.id(),
        created_at: aver_core::Timestamp::now(),
    };
    let signed = aver_core::Task::sign(body, &admin.signing_key);
    let task_id = rt.block_on(api.create_task(&signed)).unwrap();
    let claim = rt
        .block_on(api.claim(&admin.signing_key, &task_id, None))
        .unwrap();
    let m = rt
        .block_on(api.materialize(
            &claim.workspace,
            &MaterializeOptions {
                repo: Some(repo.to_owned()),
                deps: false,
                git_view: true,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(m.git_view);
    (m.dir, admin, claim.workspace)
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = git_support::hermetic(&mut std::process::Command::new("git"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn gitview_status_is_clean_after_create() {
    let d = TestHome::start();
    let repo = d.calc();
    std::fs::write(repo.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(
        repo.join("run.sh"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    std::os::unix::fs::symlink("README.md", repo.join("link")).unwrap();
    git_support::git(&repo, &["add", "-A"], b"");
    git_support::git(&repo, &["commit", "--quiet", "-m", "more"], b"");
    let (dir, _, _) = viewed(&d, &repo);
    assert_eq!(git_in(&dir, &["status", "--porcelain"]), "");
    assert_eq!(
        git_in(&dir, &["rev-parse", "HEAD"]).trim(),
        git_str(&repo, &["rev-parse", "HEAD"])
    );
    // The view borrows the repository's objects instead of copying them.
    let gitdir = git_in(&dir, &["rev-parse", "--git-common-dir"]);
    assert!(gitdir.trim().ends_with(&format!(
        "git/{}",
        Path::new(gitdir.trim())
            .file_name()
            .unwrap()
            .to_string_lossy()
    )));
    assert_eq!(
        git_in(&dir, &["count-objects"]).trim(),
        "0 objects, 0 kilobytes"
    );
}

#[test]
fn gitview_commits_as_the_repository_identity() {
    let d = TestHome::start();
    let repo = d.calc();
    let (dir, _, _) = viewed(&d, &repo);
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    git_in(&dir, &["add", "src/neg.js"]);
    git_in(&dir, &["commit", "--quiet", "-m", "negate"]);
    assert_eq!(
        git_in(&dir, &["log", "-1", "--format=%an <%ae> %cn <%ce>"]).trim(),
        "Repo Owner <owner@example.com> Repo Owner <owner@example.com>"
    );
}

#[test]
fn gitview_diff_matches_the_workspace_diff() {
    let d = TestHome::start();
    let repo = d.calc();
    let (dir, admin, ws) = viewed(&d, &repo);
    std::fs::write(dir.join("src/math.js"), "exports.add = (a, b) => b + a;\n").unwrap();
    std::fs::remove_file(dir.join("README.md")).unwrap();
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let files = rt.block_on(d.api().diff(&admin.signing_key, &ws)).unwrap();
    let mut ours: Vec<String> = files
        .iter()
        .map(|f| {
            let tag = match f.change {
                aver::workspace::DiffChange::Added => "??",
                aver::workspace::DiffChange::Modified => "M",
                aver::workspace::DiffChange::Deleted => "D",
            };
            format!("{tag} {}", f.path)
        })
        .collect();
    ours.sort();
    let mut theirs: Vec<String> = git_in(&dir, &["status", "--porcelain"])
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    theirs.sort();
    assert_eq!(ours, theirs);
    let git_diff = git_in(&dir, &["diff", "--", "src/math.js"]);
    let ours = files.iter().find(|f| f.path == "src/math.js").unwrap();
    let changed = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|l| {
                (l.starts_with('+') || l.starts_with('-'))
                    && !l.starts_with("+++")
                    && !l.starts_with("---")
            })
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(changed(&ours.diff), changed(&git_diff));
}

#[test]
fn run_detach_returns_at_once_and_finishes_in_the_background() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let cmd = sh(&script, "");
    let out = aver(
        &d.home,
        &repo,
        &[
            "run",
            "Add negate",
            "-n",
            "1",
            "-c",
            "neg: negate(x) returns -x",
            "--agent-cmd",
            &cmd,
            "--no-deps",
            "--detach",
        ],
        b"",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8(out.stdout).unwrap().trim().to_owned();
    assert_eq!(id.len(), 6, "{id}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let run = loop {
        if let Ok(run) = RunState::load(&d.home, &id) {
            if run.finished_at.is_some() {
                break run;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the background run did not finish; log: {}",
            std::fs::read_to_string(aver::run::state::run_dir(&d.home, &id).join("supervisor.log"))
                .unwrap_or_default()
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert_eq!(run.agents[0].state, AgentState::Done);
    assert!(run.supervisor.start.is_some());
    assert_ne!(run.supervisor.pid, std::process::id());
    let log =
        std::fs::read_to_string(aver::run::state::run_dir(&d.home, &id).join("supervisor.log"))
            .unwrap();
    assert!(log.contains("#1 custom  done: PASS 1/1"), "{log}");
}

#[test]
fn discard_never_signals_a_reused_pid() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let (mut out, _) = run(&d, spec(&repo, &sh(&script, ""), None, 1));
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30");
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut stranger = cmd.spawn().unwrap();
    let pid = stranger.id();
    let start = aver::run::state::process_start(pid).unwrap();
    // The run file says agent 1 still works as this pid, but it started at
    // another time: the pid was reused.
    let agent = &mut out.state.agents[0];
    agent.state = AgentState::Working;
    agent.pid = Some(pid);
    agent.pgid = Some(pid as i32);
    agent.proc_start = Some(start + 1);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut state = out.state.clone();
    rt.block_on(accept::discard(&d.api(), &d.home, &mut state))
        .unwrap();
    assert!(
        stranger.try_wait().unwrap().is_none(),
        "a reused pid is left alone"
    );
    let mut state = out.state.clone();
    state.agents[0].proc_start = Some(start);
    rt.block_on(accept::discard(&d.api(), &d.home, &mut state))
        .unwrap();
    let status = stranger.wait().unwrap();
    assert!(!status.success(), "the agent's own group is stopped");
}

/// `aver run --feedback` and `aver feedback` (§2.10): each agent's session
/// is resumed with the fixed questions, read-only, and its answer and
/// rating are kept with the run.
#[test]
fn feedback_asks_every_agent_the_fixed_questions() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let (out, _) = run(&d, spec(&repo, &sh(&script, ""), None, 2));
    let answer = d.script(
        "answer.sh",
        "case \"$PWD\" in\n\
         *-1) printf 'helped: the brief (feedback env %s)\\nRATING: 8/10\\n' \"$AVER_FEEDBACK\" ;;\n\
         *) echo '// sneaked in' >> src/neg.js; printf 'It would be 5/10.\\n%s\\n' \"$1\" ;;\n\
         esac\n",
    );
    let mut state = out.state.clone();
    let asked = aver::run::feedback::ask(
        &d.home,
        &mut state,
        Some(&format!("/bin/sh {} {{feedback}}", answer.display())),
        std::time::Duration::from_secs(60),
        &[],
    )
    .unwrap();
    assert_eq!(asked.len(), 2);
    let saved = RunState::load(&d.home, &state.id).unwrap();
    let one = saved.agents[0].feedback.as_ref().expect("agent 1 answered");
    assert_eq!(one.rating, Some(8));
    assert!(!one.changed_files);
    let text = std::fs::read_to_string(&one.file).unwrap();
    assert!(text.contains("Rating: 8/10"), "{text}");
    assert!(text.contains(aver::run::feedback::QUESTIONS), "{text}");
    assert!(text.contains("(feedback env 1)"), "{text}");
    let two = saved.agents[1].feedback.as_ref().expect("agent 2 answered");
    assert_eq!(two.rating, Some(5));
    assert!(two.changed_files, "an edit while answering is noticed");
    let answered = std::fs::read_to_string(&two.file).unwrap();
    assert!(
        answered.contains("files in the workspace changed while the agent answered"),
        "{answered}"
    );
    assert_eq!(
        aver::run::feedback::ratings(&saved).as_deref(),
        Some("#1 8/10, #2 5/10")
    );

    // While answering, Claude's Stop hook lets it stop without verifying.
    let hook = |kind: &str| {
        let mut child = std::process::Command::new(aver_bin())
            .args(["hook", kind])
            .current_dir(&saved.agents[0].dir)
            .env("AVER_HOME", d.home.root())
            .env(aver::run::feedback::FEEDBACK_ENV, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(br#"{"tool_name":"Write","tool_input":{"file_path":"src/x.js"}}"#)
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    assert_eq!(hook("stop"), "");
}

fn status_in(d: &TestHome, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = aver(&d.home, cwd, args, b"");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn commands_without_a_run_named_mean_this_repository() {
    let d = TestHome::start();
    let calc = d.calc();
    let other = d.repo(
        "other",
        &[
            ("aver.toml", &aver_toml()),
            ("src/math.js", "exports.add = (a, b) => a + b;\n"),
            ("test/math.test.json", r#"{"adds": "pass-if:src/math.js"}"#),
        ],
    );
    let empty = d.repo("empty", &[("README.md", "empty\n")]);
    let plain = d.root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let script = d.script("good.sh", GOOD);
    let tasks = aver::run::batch::input::parse_tasks("Add negate\n  - neg: negates\n").unwrap();
    let (batch, _) = d.run_batch(batch_spec(&calc, tasks, &script, 1));
    // Newer than the batch, in another repository.
    let (newer, _) = run(&d, spec(&other, &sh(&script, ""), None, 1));

    let (code, text, err) = status_in(&d, &calc.join("src"), &["status"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        text.starts_with(&format!("batch {}", batch.state.id)),
        "the batch of this repository, not the newer run elsewhere: {text}"
    );
    let (code, text, err) = status_in(&d, &other, &["status"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        text.starts_with(&format!("run {}", newer.state.id)),
        "{text}"
    );
    let (code, text, err) = status_in(&d, &plain, &["status"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        text.starts_with(&format!("run {}", newer.state.id)),
        "outside a repository, the newest anywhere: {text}"
    );
    for args in [
        &["status"][..],
        &["diff", "1"],
        &["accept", "1"],
        &["discard"],
    ] {
        let (code, _, err) = status_in(&d, &empty, args);
        assert_eq!(code, 2, "aver {args:?}: {err}");
        assert!(
            err.contains("no runs in") && err.contains("aver status --all"),
            "aver {args:?}: {err}"
        );
    }

    let (code, all, err) = status_in(&d, &empty, &["status", "--all", "--json"]);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&all).unwrap();
    assert_eq!(v["batches"][0]["repo"], calc.to_str().unwrap());
    let repos: Vec<&str> = v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["repo"].as_str().unwrap())
        .collect();
    assert_eq!(repos, [other.to_str().unwrap(), calc.to_str().unwrap()]);
    let (_, text, _) = status_in(&d, &empty, &["status", "--all"]);
    let line = |id: &str| {
        text.lines()
            .find(|l| l.contains(id))
            .unwrap_or_else(|| panic!("{id} in {text}"))
            .to_owned()
    };
    assert!(line(&batch.state.id).contains(" calc "), "{text}");
    assert!(line(&newer.state.id).contains(" other "), "{text}");
}

/// The run's verify budget is hard: verify N+1 of N exits 2 before running
/// anything and adds no receipt; `aver check` has no budget.
#[test]
fn verify_past_the_budget_exits_2_and_check_still_runs() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("vacuous.sh", VACUOUS);
    let mut s = spec(&repo, &sh(&script, ""), None, 1);
    s.budget.max_verifies = 1;
    let (out, transcript) = run(&d, s);
    assert_eq!(out.compared[0].verifies, 1, "{transcript}");
    let dir = out.state.agents[0].dir.clone();
    let o = aver(&d.home, &dir, &["verify"], b"");
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.contains("out of verifies: 1 of 1 used"), "{err}");
    assert!(o.stdout.is_empty(), "nothing ran");
    let last = aver(&d.home, &dir, &["verify", "--last"], b"");
    let text = String::from_utf8(last.stdout).unwrap();
    assert!(
        text.contains("Verifies: 1 of 1 used"),
        "no receipt added: {text}"
    );
    let check = aver(&d.home, &dir, &["check"], b"");
    let err = String::from_utf8_lossy(&check.stderr);
    assert!(!err.contains("out of verifies"), "{err}");
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("Verify would stop at UNPROVEN"),
        "{}{err}",
        String::from_utf8_lossy(&check.stdout)
    );
}

/// An agent that exits twice in a row without changing a file is not
/// resumed again: it ends `idle`, which `aver status` shows.
#[test]
fn an_agent_that_exits_twice_without_a_change_is_not_resumed() {
    let d = TestHome::start();
    let repo = d.calc();
    let idle = d.script("idle.sh", "echo 'nothing to do'\n");
    let (out, transcript) = run(
        &d,
        spec(&repo, &sh(&idle, ""), Some(&sh(&idle, " {feedback}")), 1),
    );
    let a = &out.state.agents[0];
    assert_eq!((a.state, a.resumes), (AgentState::Idle, 1), "{transcript}");
    let last = events(&transcript).last().cloned().unwrap();
    assert!(
        last.starts_with("#1 custom  stopped: exited 2 times in a row without changing a file"),
        "{transcript}"
    );
    let o = aver(&d.home, &repo, &["status", &out.state.id], b"");
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains(" idle"), "{text}");
}

/// Defect: an agent that changed nothing, on a task without criteria, was
/// `done: PASS` in the live view and offered for `aver accept`, while its
/// verify report said NOT DONE ("you changed nothing yet").
#[test]
fn an_agent_that_changed_nothing_is_not_done() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("idle.sh", "echo 'nothing to do'\naver verify || true\n");
    let mut spec = spec(&repo, &sh(&script, ""), None, 1);
    spec.criteria.clear();
    let (out, transcript) = run(&d, spec);
    let lines = events(&transcript);
    assert!(
        !lines.iter().any(|l| l.contains("done: PASS")),
        "{transcript}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.ends_with("not done: changed nothing")),
        "{transcript}"
    );
    assert_ne!(out.state.agents[0].state, AgentState::Done);
    assert!(!out.any_pass());
    let hints = compare::hints(&out.compared);
    assert!(!hints.contains("aver accept"), "{hints}");
}

/// Defect: the hint under the ranking named `aver accept <best>` even when
/// no attempt was acceptable, and accept then refused it.
#[test]
fn the_hints_offer_accept_only_for_an_acceptable_best() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("breaks.sh", &format!("{BREAKS}aver verify || true\n"));
    let (out, transcript) = run(&d, spec(&repo, &sh(&script, ""), None, 1));
    assert_eq!(
        out.compared[0].verdict,
        Some(Verdict::FailOwn),
        "{transcript}"
    );
    assert_eq!(
        compare::hints(&out.compared),
        "  aver diff 1   aver discard\n"
    );
}

/// Defect: a second Ctrl-C made `aver run` exit 130 at once but left an
/// agent that ignores SIGTERM running in its own process group, still
/// editing its directory with nobody watching.
#[test]
fn a_second_ctrl_c_kills_the_agents_before_exiting() {
    let d = TestHome::start();
    let repo = d.calc();
    let pids = d.root.join("agent.pid");
    let script = d.script(
        "stubborn.sh",
        &format!("trap '' TERM\necho $$ > {}\nsleep 300\n", pids.display()),
    );
    let mut run = std::process::Command::new(aver_bin());
    for var in aver::run::accept::AGENT_ENV {
        run.env_remove(var);
    }
    let mut child = run
        .args([
            "run",
            "Add negate",
            "-n",
            "1",
            "--agent-cmd",
            &sh(&script, ""),
            "--no-deps",
            "--no-tui",
        ])
        .current_dir(&repo)
        .env("AVER_HOME", d.home.root())
        .env("AVER_SCRIPTED", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let agent: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pids)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            break pid;
        }
        assert!(child.try_wait().unwrap().is_none(), "aver run ended first");
        assert!(started.elapsed() < std::time::Duration::from_secs(60));
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    for _ in 0..2 {
        // SAFETY: signals the child this test spawned.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(std::time::Instant::now() < deadline, "aver run still runs");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let mut said = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut out, &mut said);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut err, &mut said);
    }
    assert_eq!(status.code(), Some(130), "{said}");
    std::thread::sleep(std::time::Duration::from_millis(200));
    // SAFETY: signal 0 only checks; the agent's shell led its group.
    let alive = unsafe { libc::kill(-agent, 0) } == 0;
    if alive {
        // SAFETY: as above; cleans up before failing.
        unsafe { libc::kill(-agent, libc::SIGKILL) };
    }
    assert!(!alive, "the agent's group {agent} still runs");
}
