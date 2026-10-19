//! Idempotent hand-off (custody K3) without the fault harness: each test
//! recreates the state a crash leaves (a branch written but the run not
//! saved, a temp ref of a dead export, a pull request opened but not
//! recorded, a combine exported but not recorded, a batch naming a rerun
//! that never started) and checks that the next command finishes the job.
//! Shell checks on `fixtures/sh-calc`: no Node, no Docker.

#[allow(dead_code)]
#[path = "custody/audit.rs"]
mod audit;
#[allow(dead_code)]
#[path = "custody/world.rs"]
mod world;

use std::fs;

use audit::{audit, Allowed, RepoState};
use world::{git, text, World, CRITERION, DOUBLE};

fn branches() -> Allowed {
    Allowed {
        branches: vec!["aver/"],
        ..Allowed::default()
    }
}

fn run_json(w: &World) -> (std::path::PathBuf, serde_json::Value) {
    let dir = fs::read_dir(w.home.join("runs"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path()
        .join("run.json");
    let v = serde_json::from_slice(&fs::read(&dir).unwrap()).unwrap();
    (dir, v)
}

/// An accept run again once its branch is written succeeds: in a
/// directory it adopts the branch at the verified snapshot's commit, and
/// for a run whose decision is saved it says what it did.
#[test]
fn crit_k3_accept_retry() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    let dir = w.passing_workspace();
    w.ok(&dir, &["accept", "--keep"]);
    let tip = |w: &World| {
        git(
            &w.repo,
            &["for-each-ref", "--format=%(objectname)", "refs/heads/aver/"],
        )
    };
    let was = tip(&w);
    let again = w.aver(&dir, &["accept", "--keep"], None);
    assert!(again.status.success(), "{}", text(&again));
    assert_eq!(tip(&w), was, "the same branch at the same commit");
    let broken = audit(&w, &before, &branches());
    assert!(broken.is_empty(), "{broken:#?}");

    // A run's accept that saved its decision and stopped before it said
    // so: run again, it says what it did.
    let w = World::new();
    let before = RepoState::of(&w.repo);
    w.finished_run();
    w.ok(&w.repo, &["accept", "1"]);
    let (_, run) = run_json(&w);
    let again = w.aver(&w.repo, &["accept", "1"], None);
    let said = String::from_utf8_lossy(&again.stdout);
    assert!(again.status.success(), "{}", text(&again));
    let branch = run["decided"]["branch"].as_str().unwrap();
    assert!(
        said.contains(&format!("already accepted: attempt 1 as branch {branch}")),
        "{said}"
    );
    let broken = audit(&w, &before, &branches());
    assert!(broken.is_empty(), "{broken:#?}");
}

/// An edit made after the accept committed the directory is not exported,
/// and the directory that holds it is kept.
#[test]
fn crit_k3_edit_at_commit() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    let dir = w.passing_workspace();
    w.ok(&dir, &["accept", "--keep"]);
    let calc = dir.join("calc.sh");
    let mut edited = fs::read_to_string(&calc).unwrap();
    edited.push_str("half() { echo $(($1 / 2)); }\n");
    fs::write(&calc, &edited).unwrap();
    let out = w.aver(&dir, &["accept"], None);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", text(&out));
    let branch = git(
        &w.repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/aver/",
        ],
    );
    let exported = git(&w.repo, &["show", &format!("{branch}:calc.sh")]);
    assert!(
        exported.contains("neg") && !exported.contains("half"),
        "{exported}"
    );
    assert_eq!(fs::read_to_string(&calc).unwrap(), edited, "{said}");
    assert!(said.contains("Kept its directory"), "{said}");
    let broken = audit(&w, &before, &branches());
    assert!(broken.is_empty(), "{broken:#?}");
}

/// The fast-import refs of an export whose process is gone, and of one an
/// older Aver left long ago, are removed by the next export; a running
/// export's stays.
#[test]
fn crit_k3_temp_refs() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    let dir = w.passing_workspace();
    let head = git(&w.repo, &["rev-parse", "HEAD"]);
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let dead_pid = gone.id();
    gone.wait().unwrap();
    let dead = format!("refs/aver/export/{dead_pid}-1-01M4Y3X2G3GASRXDJ8K4S7NS4K");
    let old = "refs/aver/export/01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let me = std::process::id();
    let live = format!(
        "refs/aver/export/{me}-{}-01M4Y3X2G3GASRXDJ8K4S7NS4K",
        aver::run::state::process_start(me).unwrap_or(0)
    );
    for r in [dead.as_str(), old, live.as_str()] {
        git(&w.repo, &["update-ref", r, &head]);
    }
    w.ok(&dir, &["accept"]);
    let left = git(
        &w.repo,
        &["for-each-ref", "--format=%(refname)", "refs/aver/"],
    );
    assert_eq!(left, live, "only the running export's ref stays");
    git(&w.repo, &["update-ref", "-d", &live]);
    let broken = audit(&w, &before, &branches());
    assert!(broken.is_empty(), "{broken:#?}");
}

/// `aver accept 1 --pr` when `gh` already has an open pull request for the
/// branch (one opened before a crash) adopts it instead of opening another.
#[test]
fn crit_k3_pr_adopt() {
    let w = World::new();
    w.finished_run();
    let log = w.stub_gh();
    fs::write(log.with_file_name("gh.created"), "").unwrap();
    let out = w.aver(&w.repo, &["accept", "1", "--pr"], None);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        said.contains("Pull request: https://github.com/o/r/pull/7"),
        "{said}"
    );
    let calls = fs::read_to_string(&log).unwrap();
    assert!(!calls.contains("pr create"), "{calls}");
    let (_, run) = run_json(&w);
    assert_eq!(
        run["decided"]["pr"].as_str(),
        Some("https://github.com/o/r/pull/7")
    );
}

/// A combine that exported its branch but stopped before recording it,
/// run again, adopts the branch: one combine branch, recorded once.
#[test]
fn crit_k3_combine_resume() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    w.batch(&[("negate numbers", CRITERION), ("double numbers", DOUBLE)]);
    w.ok(&w.repo, &["combine", "1", "2"]);
    let records = w.home.join("combines");
    for e in fs::read_dir(&records).unwrap().flatten() {
        fs::remove_file(e.path()).unwrap();
    }
    let out = w.aver(&w.repo, &["combine", "1", "2"], None);
    assert!(out.status.success(), "{}", text(&out));
    let names = git(&w.repo, &["branch", "--format=%(refname:short)"]);
    let combines = names
        .lines()
        .filter(|b| b.starts_with("aver/combine-"))
        .count();
    assert_eq!(combines, 1, "{names}");
    assert_eq!(fs::read_dir(&records).unwrap().count(), 1);
    let broken = audit(&w, &before, &branches());
    assert!(broken.is_empty(), "{broken:#?}");
}

/// A batch that names a rerun whose run never started (what a crash left
/// before the rerun was recorded only after its first save): `aver
/// status` shows the task's last run and says so, and `aver accept 1`
/// accepts that run.
#[test]
fn crit_k3_rerun_record() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    w.batch(&[("negate numbers", CRITERION)]);
    let batch = fs::read_dir(w.home.join("batches"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path()
        .join("batch.json");
    let mut state: serde_json::Value = serde_json::from_slice(&fs::read(&batch).unwrap()).unwrap();
    let task = &mut state["tasks"][0];
    let old = task["run"].clone();
    task["history"] = serde_json::json!([old]);
    task["run"] = serde_json::json!("ZZZZZZ");
    fs::write(&batch, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    let status = w.aver(&w.repo, &["status"], None);
    let said = String::from_utf8_lossy(&status.stdout);
    assert!(status.status.success(), "{}", text(&status));
    assert!(said.contains("the rerun did not start"), "{said}");
    w.ok(&w.repo, &["accept", "1"]);
    let broken = audit(&w, &before, &branches());
    assert!(broken.is_empty(), "{broken:#?}");
}
