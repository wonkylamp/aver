//! What `aver accept` leaves behind (§2.10, Accept): the winner's
//! directory is released once its branch is written, unless `--keep` or
//! unfetched Git-view commits keep it; `aver gc --accepted` removes what
//! is left of finished runs; the commit is exactly the verified snapshot;
//! `aver accept <n> --pr` after the accept opens the pull request. Scripted
//! fake agents with the scripted runner. No Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use aver::keys::AgentKey;
use aver::run::accept::{self, AcceptOptions, Cleanup};
use aver::run::agent::Template;
use aver::run::state::{Budget, RunState};
use aver::run::supervisor::{RunSpec, Supervisor};
use aver::run::view::Lines;
use git_support::git_str;
use run_support::*;

const GOOD: &str = r#"mkdir -p src test
echo 'module.exports = (x) => -x;' > src/neg.js
echo '{"[crit:neg] negates": "pass-if:src/neg.js"}' > test/neg.test.json
"#;

fn admin(d: &TestHome) -> AgentKey {
    AgentKey::read(&d.home.key_path(&"admin".parse().unwrap())).unwrap()
}

fn run(d: &TestHome, repo: &Path, script: &Path, agents: u32, git_view: bool) -> RunState {
    run_spec(d, spec(repo, script, agents, git_view))
}

fn spec(repo: &Path, script: &Path, agents: u32, git_view: bool) -> RunSpec {
    RunSpec {
        repo: repo.to_owned(),
        rev: "HEAD".into(),
        prompt: "Add negate".into(),
        title: None,
        criteria: vec![("neg".into(), "negate(x) returns -x".into())],
        protected: Vec::new(),
        agents: (0..agents)
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
        git_view,
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

fn run_spec(d: &TestHome, spec: RunSpec) -> RunState {
    let stop = AtomicBool::new(false);
    let mut view = Lines::new(Vec::new());
    Supervisor::new(&d.home, d.api(), admin(d), &mut view, &stop)
        .unwrap()
        .run(spec)
        .unwrap()
        .state
}

fn accept(d: &TestHome, run: &mut RunState, opts: AcceptOptions) -> accept::Accepted {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(accept::accept(
            &d.api(),
            &d.home,
            &admin(d),
            run,
            "1",
            &opts,
        ))
        .unwrap()
}

/// The state `aver workspace ls --all` gives the directory `dir`.
fn mat_state(d: &TestHome, dir: &Path) -> String {
    let out = aver(
        &d.home,
        &d.root,
        &["workspace", "ls", "--all", "--json"],
        b"",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["dir"].as_str().map(PathBuf::from).as_deref() == Some(dir))
        .unwrap_or_else(|| panic!("no row for {}: {rows}", dir.display()))["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn accept_removes_the_winners_directory() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let mut state = run(&d, &repo, &script, 1, true);
    let dir = state.agents[0].dir.clone();
    assert!(dir.exists());
    let accepted = accept(&d, &mut state, AcceptOptions::default());
    assert_eq!(accepted.cleanup, Cleanup::Released);
    assert!(!dir.exists(), "the winner's directory is gone");
    assert_eq!(mat_state(&d, &dir), "released");
    let suffix = accepted
        .branch
        .strip_prefix("aver/add-negate-")
        .unwrap_or_else(|| panic!("{}", accepted.branch));
    assert_eq!(
        suffix,
        state.id.as_str()[state.id.as_str().len() - 4..].to_lowercase(),
        "the title's slug and the run id's last four characters"
    );
    assert_eq!(
        git_str(&repo, &["show", &format!("{}:src/neg.js", accepted.branch)]),
        "module.exports = (x) => -x;",
        "the branch holds the work"
    );
}

#[test]
fn keep_keeps_the_winners_directory() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let mut state = run(&d, &repo, &script, 1, false);
    let dir = state.agents[0].dir.clone();
    let accepted = accept(
        &d,
        &mut state,
        AcceptOptions {
            keep: true,
            ..Default::default()
        },
    );
    assert_eq!(accepted.cleanup, Cleanup::Kept);
    assert!(dir.exists());
    assert!(accepted
        .cleanup
        .describe(&dir)
        .contains("aver gc --accepted"));
}

#[test]
fn unfetched_git_view_commits_keep_the_directory_and_say_why() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script(
        "commits.sh",
        &format!("{GOOD}git add -A\ngit commit -q -m 'negate, my way'\n"),
    );
    let mut state = run(&d, &repo, &script, 1, true);
    let dir = state.agents[0].dir.clone();
    let accepted = accept(&d, &mut state, AcceptOptions::default());
    let Cleanup::KeptBecause(why) = &accepted.cleanup else {
        panic!("kept for a reason, not {:?}", accepted.cleanup);
    };
    assert!(why.contains("Git view has a commit"), "{why}");
    assert!(why.contains("aver workspace rm"), "{why}");
    assert!(dir.exists());
    assert_eq!(mat_state(&d, &dir), "live");
}

#[test]
fn gc_accepted_removes_what_finished_runs_left() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let mut state = run(&d, &repo, &script, 2, false);
    let dirs: Vec<PathBuf> = state.agents.iter().map(|a| a.dir.clone()).collect();
    accept(
        &d,
        &mut state,
        AcceptOptions {
            keep: true,
            keep_losers: true,
            ..Default::default()
        },
    );
    assert!(dirs.iter().all(|d| d.exists()));
    // A plain gc keeps them: they are live.
    let out = aver(&d.home, &d.root, &["gc"], b"");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dirs.iter().all(|d| d.exists()));
    let out = aver(&d.home, &d.root, &["gc", "--accepted"], b"");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("removed 2 directories of accepted or discarded work"),
        "{stdout}"
    );
    for dir in &dirs {
        assert!(!dir.exists(), "{} is gone", dir.display());
        assert_eq!(mat_state(&d, dir), "released");
    }
}

/// A commit names the snapshot it must be: a directory that changed since
/// is refused with SNAPSHOT_MOVED and stays open, so `aver accept` never
/// exports an edit made after the verify under that verify's receipt.
#[test]
fn a_commit_refuses_a_directory_that_moved_since_its_snapshot() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let state = run(&d, &repo, &script, 1, false);
    let agent = &state.agents[0];
    let key = AgentKey::read(&d.home.key_path(&agent.key.parse().unwrap())).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let api = d.api();
    let verified = rt
        .block_on(api.snapshot(&key.signing_key, &agent.workspace))
        .unwrap();
    std::fs::write(agent.dir.join("src/neg.js"), "module.exports = () => 0;\n").unwrap();
    let commit = |snapshot: &aver_core::SnapshotId| {
        rt.block_on(api.commit_if(&key.signing_key, &agent.workspace, "m", Some(snapshot)))
    };
    let refused = commit(&verified).unwrap_err();
    assert_eq!(refused.code(), Some("SNAPSHOT_MOVED"), "{refused}");
    let now = rt
        .block_on(api.snapshot(&key.signing_key, &agent.workspace))
        .unwrap();
    assert_ne!(now, verified);
    let done = commit(&now).unwrap();
    assert_eq!(done.snapshot, now);
}

fn edit_neg(dir: &Path) {
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => 0 - x;\n").unwrap();
}

/// An edit after accept's last check fails the commit; the attempt stays
/// open (nothing is decided before the commit), so the next accept
/// verifies the directory as it is and accepts it.
#[test]
fn an_edit_between_check_and_commit_leaves_the_attempt_open_and_retryable() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let mut state = run(&d, &repo, &script, 1, false);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let api = d.api();
    let refused = rt.block_on(accept::accept(
        &api,
        &d.home,
        &admin(&d),
        &mut state,
        "1",
        &AcceptOptions {
            before_commit: Some(edit_neg),
            ..Default::default()
        },
    ));
    let err = refused.expect_err("the commit refuses the moved directory");
    assert!(
        err.to_string().contains("changed after its last verify"),
        "{err}"
    );
    let attempt = rt
        .block_on(api.attempt(&admin(&d).signing_key, &state.agents[0].attempt))
        .unwrap();
    assert_eq!(attempt.state, aver::tasks::AttemptState::Open);
    assert!(state.decided.is_none());
    assert!(git_str(&repo, &["branch", "--list", "aver/*"]).is_empty());

    let accepted = accept(&d, &mut state, AcceptOptions::default());
    assert_eq!(
        git_str(&repo, &["show", &format!("{}:src/neg.js", accepted.branch)]),
        "module.exports = (x) => 0 - x;",
        "the second accept verified and exported the edit"
    );
}

/// `GOOD`, but it removes what the existing test needs and rewrites the
/// test to pass anyway: `REVIEW`.
const REWRITES_A_TEST: &str = r#"mkdir -p src test
rm src/math.js
echo 'module.exports = (x) => -x;' > src/neg.js
echo '{"[crit:neg] negates": "pass-if:src/neg.js"}' > test/neg.test.json
echo '{"adds": "pass"}' > test/math.test.json
"#;

/// An approval binds to the receipt it approved (B5, addendum 1): one
/// journaled for head X, whose commit then failed because the directory
/// moved to Y, does not approve Y's `REVIEW`. Y needs its own approval,
/// and the commit names Y's receipt.
#[test]
fn an_approval_journaled_for_one_head_does_not_carry_to_another() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("review.sh", REWRITES_A_TEST);
    let mut state = run(&d, &repo, &script, 1, false);
    let run_id = state.id.clone();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let api = d.api();
    let mut try_accept = |opts: AcceptOptions| {
        rt.block_on(accept::accept(
            &api,
            &d.home,
            &admin(&d),
            &mut state,
            "1",
            &opts,
        ))
    };
    let err = try_accept(AcceptOptions::default()).expect_err("REVIEW needs an approval");
    assert!(
        err.to_string().contains("attempt 1 is REVIEW")
            && err.to_string().contains("`aver accept 1 --approve-review`"),
        "{err}"
    );

    let err = try_accept(AcceptOptions {
        approve_review: true,
        before_commit: Some(edit_neg),
        ..Default::default()
    })
    .expect_err("the directory moved before the commit");
    assert!(
        err.to_string().contains("changed after its last verify"),
        "{err}"
    );
    let x = aver::run::state::Accepting::load(&d.home, &run_id)
        .and_then(|j| j.approved)
        .expect("the approval of X is journaled");

    let err = try_accept(AcceptOptions::default())
        .expect_err("the journaled approval of X does not approve Y");
    assert!(err.to_string().contains("attempt 1 is REVIEW"), "{err}");
    assert!(!err.to_string().contains(&x.to_string()), "{err}");

    let accepted = try_accept(AcceptOptions {
        approve_review: true,
        ..Default::default()
    })
    .unwrap();
    let y = accepted.approved.expect("Y approved");
    assert_ne!(x, y);
    let message = git_str(&repo, &["log", "-1", "--format=%B", &accepted.branch]);
    assert!(
        message
            .lines()
            .any(|l| l == format!("Aver-Approved-Review: {y}")),
        "{message}"
    );
    assert_eq!(
        git_str(&repo, &["show", &format!("{}:src/neg.js", accepted.branch)]),
        "module.exports = (x) => 0 - x;"
    );
}

/// A stub `gh` that records its arguments and prints a PR URL, first on
/// `PATH`, and a bare `origin` for the push.
fn stub_gh(d: &TestHome, repo: &Path) -> (PathBuf, PathBuf) {
    let bin = d.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = d.root.join("gh.log");
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" >> {}\necho https://github.com/o/r/pull/7\n",
            log.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let origin = d.root.join("origin.git");
    git_support::git(
        &d.root,
        &["init", "--quiet", "--bare", origin.to_str().unwrap()],
        b"",
    );
    git_support::git(
        repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        b"",
    );
    (bin, log)
}

fn aver_with_path(d: &TestHome, bin: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    std::process::Command::new(aver_bin())
        .args(args)
        .current_dir(cwd)
        .env("AVER_HOME", d.home.root())
        .env("PATH", path)
        .env_remove("AVER_URL")
        .output()
        .unwrap()
}

/// `aver accept 1`, then `aver accept 1 --pr` as its Next line says: the
/// second opens the pull request for the branch the first wrote.
#[test]
fn accept_pr_after_accept_opens_the_pr_for_the_recorded_branch() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let state = run(&d, &repo, &script, 1, false);
    let (bin, log) = stub_gh(&d, &repo);
    let first = aver_with_path(&d, &bin, &repo, &["accept", "1", "--run", &state.id]);
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(stdout.contains("aver accept 1 --pr"), "{stdout}");
    let second = aver_with_path(
        &d,
        &bin,
        &repo,
        &["accept", "1", "--run", &state.id, "--pr"],
    );
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        stdout.contains("Pull request: https://github.com/o/r/pull/7"),
        "{stdout}"
    );
    let args = std::fs::read_to_string(&log).unwrap();
    assert!(args.contains("pr create --head aver/add-negate"), "{args}");
    let saved = RunState::load(&d.home, &state.id).unwrap();
    assert_eq!(
        saved.decided.unwrap().pr.as_deref(),
        Some("https://github.com/o/r/pull/7")
    );
    // A third time names the PR it opened.
    let third = aver_with_path(
        &d,
        &bin,
        &repo,
        &["accept", "1", "--run", &state.id, "--pr"],
    );
    assert!(String::from_utf8_lossy(&third.stdout)
        .contains("its pull request: https://github.com/o/r/pull/7"));
}

/// A run stacked on another task's accepted branch opens its pull request
/// against that branch.
#[test]
fn a_stacked_run_opens_its_pr_on_the_prerequisite_branch() {
    let d = TestHome::start();
    let repo = d.calc();
    let script = d.script("good.sh", GOOD);
    let mut first = run(&d, &repo, &script, 1, false);
    let prerequisite = accept(&d, &mut first, AcceptOptions::default()).branch;
    let decided = first.decided.clone().unwrap();
    let double = d.script(
        "double.sh",
        "echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
         echo '{\"[crit:neg] doubles\": \"pass-if:src/dbl.js\"}' > test/dbl.test.json\n",
    );
    let mut stacked = spec(&repo, &double, 1, false);
    stacked.rev = decided.commit.clone().unwrap();
    stacked.prompt = "Add double".into();
    stacked.stacked_on = Some(aver::run::state::Stack {
        onto: aver::run::state::StackOnto::Run {
            run: first.id.clone(),
            number: None,
        },
        commit: decided.commit.clone().unwrap(),
        head: decided.head.unwrap(),
        root: first.base,
    });
    let second = run_spec(&d, stacked);
    assert!(second.stacked_on.is_some());
    let (bin, log) = stub_gh(&d, &repo);
    let out = aver_with_path(
        &d,
        &bin,
        &repo,
        &["accept", "1", "--run", &second.id, "--pr"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let args = std::fs::read_to_string(&log).unwrap();
    assert!(
        args.contains("pr create --head aver/add-double-")
            && args.contains(&format!(" --base {prerequisite} ")),
        "{args}"
    );
}
