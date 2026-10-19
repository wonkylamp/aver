//! Leases and overlap notices between tasks in one repository (§1.8.4,
//! §2.11), through the coordination service in process. No Docker.

mod common;
mod scripted;
mod world;

use std::fs;
use std::path::PathBuf;

use aver::agents::Capability;
use aver::coord::{Coord, CoordError, LeaseMode, LeaseRequest, Overlap};
use aver::keys::AgentKey;
use aver::workspace::MaterializeOptions;
use aver_core::*;
use rusqlite::Connection;
use world::*;

const A_JS: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";

struct Fx {
    w: World,
    repo: PathBuf,
    base: SnapshotId,
    coord: Coord,
    n: std::cell::Cell<u32>,
}

/// One attempt's agent, workspace and directory.
struct Att {
    key: AgentKey,
    attempt: AttemptId,
    ws: WorkspaceId,
    dir: PathBuf,
}

impl Fx {
    fn new() -> Self {
        Self::with_files(&[("src/a.js", A_JS), ("README.md", "hi\n")])
    }

    fn with_files(files: &[(&str, &str)]) -> Self {
        let w = World::new();
        let repo = w.dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let base = w.snapshot(files);
        let coord = Coord::new(w.store.clone(), w.workspaces.clone());
        Self {
            w,
            repo,
            base,
            coord,
            n: Default::default(),
        }
    }

    fn task(&self, title: &str) -> TaskId {
        let mut body = self.w.task_body(&self.w.admin, self.base, &[]);
        body.title = title.parse().unwrap();
        self.w.create_task(body)
    }

    fn attempt(&self, task: &TaskId) -> Att {
        self.n.set(self.n.get() + 1);
        let key = self.w.agent(
            &format!("agent-{}", self.n.get()),
            &[
                Capability::TaskClaim,
                Capability::WorkspaceWrite,
                Capability::LeaseWrite,
            ],
        );
        let claim = self
            .w
            .tasks
            .claim(&key.id(), task, Some(self.base))
            .unwrap();
        let m = self
            .w
            .workspaces
            .materialize(
                &claim.workspace,
                &MaterializeOptions {
                    repo: Some(self.repo.clone()),
                    deps: false,
                    ..Default::default()
                },
            )
            .unwrap();
        Att {
            key,
            attempt: claim.attempt,
            ws: claim.workspace,
            dir: m.dir,
        }
    }

    fn lease(&self, a: &Att, globs: &[&str], mode: LeaseMode) -> Result<LeaseId, CoordError> {
        self.coord
            .acquire(
                &a.key.id(),
                &a.ws,
                LeaseRequest {
                    paths: globs.iter().map(|g| g.parse().unwrap()).collect(),
                    mode,
                    reason: "restructuring".into(),
                    ttl_ms: None,
                },
            )
            .map(|h| h.lease.id)
    }

    fn db(&self) -> Connection {
        Connection::open(self.w.store.path()).unwrap()
    }
}

#[test]
fn lease_exclusive_conflict_across_tasks() {
    let fx = Fx::new();
    let (a, b) = (fx.task("Migrate sessions"), fx.task("Rate limit"));
    let (a1, b1) = (fx.attempt(&a), fx.attempt(&b));
    fx.lease(&a1, &["src/**"], LeaseMode::Exclusive).unwrap();
    let err = fx.lease(&b1, &["src/a.js"], LeaseMode::Shared).unwrap_err();
    match &err {
        CoordError::Conflict { message, holder } => {
            assert!(message.contains("\"Migrate sessions\""), "{message}");
            assert_eq!(holder.lease.task, Some(a));
        }
        other => panic!("expected a conflict, got {other}"),
    }
    assert_eq!(err.code(), "LEASE_CONFLICT");
    fx.lease(&b1, &["lib/**", "README.md"], LeaseMode::Exclusive)
        .unwrap();
    // Two shared leases on the same paths coexist.
    fx.lease(&a1, &["docs"], LeaseMode::Shared).unwrap();
    fx.lease(&b1, &["docs/x.md"], LeaseMode::Shared).unwrap();
    let seen = fx.coord.leases(&b1.key.id(), &b1.ws).unwrap();
    assert_eq!(seen.len(), 4);
    assert_eq!(seen.iter().filter(|h| h.binds()).count(), 2);
}

#[test]
fn lease_ignores_sibling_attempts() {
    let fx = Fx::new();
    let a = fx.task("Migrate sessions");
    let (a1, a2) = (fx.attempt(&a), fx.attempt(&a));
    fx.lease(&a1, &["src/**"], LeaseMode::Exclusive).unwrap();
    fx.lease(&a2, &["src/a.js"], LeaseMode::Exclusive).unwrap();
    let seen = fx.coord.leases(&a2.key.id(), &a2.ws).unwrap();
    assert!(seen.iter().all(|h| !h.binds()), "{seen:?}");
    assert!(seen.iter().any(|h| h.sibling));
    // Siblings never see each other's changes either.
    fs::write(a1.dir.join("src/a.js"), A_JS.replace("l2", "x2")).unwrap();
    fs::write(a2.dir.join("src/a.js"), A_JS.replace("l2", "y2")).unwrap();
    fx.w.workspaces.sync(&a1.key.id(), &a1.ws).unwrap();
    let n = fx.coord.notices(&a2.key.id(), &a2.ws, true).unwrap();
    assert!(n.is_empty(), "{n:?}");
}

#[test]
fn lease_expires_on_read() {
    let fx = Fx::new();
    let (a, b) = (fx.task("A"), fx.task("B"));
    let (a1, b1) = (fx.attempt(&a), fx.attempt(&b));
    let id = fx.lease(&a1, &["src"], LeaseMode::Exclusive).unwrap();
    fx.db()
        .execute(
            "UPDATE leases SET expires_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
            [id.to_string()],
        )
        .unwrap();
    assert!(fx.coord.leases(&b1.key.id(), &b1.ws).unwrap().is_empty());
    fx.lease(&b1, &["src/a.js"], LeaseMode::Exclusive).unwrap();
}

#[test]
fn lease_renewed_by_sync() {
    let fx = Fx::new();
    let a = fx.task("A");
    let a1 = fx.attempt(&a);
    let id = fx.lease(&a1, &["src"], LeaseMode::Exclusive).unwrap();
    let soon = Timestamp::from_unix_millis(Timestamp::now().unix_millis() + 30_000).unwrap();
    fx.db()
        .execute(
            "UPDATE leases SET expires_at = ?2 WHERE id = ?1",
            [id.to_string(), soon.to_string()],
        )
        .unwrap();
    fx.w.workspaces.sync(&a1.key.id(), &a1.ws).unwrap();
    let held = fx.coord.leases(&a1.key.id(), &a1.ws).unwrap();
    let lease = &held[0].lease;
    assert_eq!(lease.renewals, 1);
    assert!(
        lease.expires_at.unix_millis() > Timestamp::now().unix_millis() + 29 * 60 * 1000,
        "{}",
        lease.expires_at
    );
}

#[test]
fn a_discarded_or_released_holder_frees_its_leases() {
    let fx = Fx::new();
    let (a, b) = (fx.task("A"), fx.task("B"));
    let (a1, a2, b1) = (fx.attempt(&a), fx.attempt(&a), fx.attempt(&b));
    fx.lease(&a1, &["src"], LeaseMode::Exclusive).unwrap();
    let id = fx.lease(&a2, &["README.md"], LeaseMode::Exclusive).unwrap();
    fx.w.workspaces.discard(&a1.key.id(), &a1.ws).unwrap();
    fx.lease(&b1, &["src"], LeaseMode::Exclusive).unwrap();
    assert!(matches!(
        fx.coord.release(&b1.key.id(), &id, false),
        Err(CoordError::Forbidden(_))
    ));
    let released = fx.coord.release(&a2.key.id(), &id, false).unwrap();
    assert!(released.released_at.is_some());
    fx.lease(&b1, &["README.md"], LeaseMode::Exclusive).unwrap();
}

#[test]
fn notice_overlapping_lines() {
    let fx = Fx::new();
    let (a, b) = (fx.task("Migrate sessions"), fx.task("Rate limit"));
    let (a1, b1) = (fx.attempt(&a), fx.attempt(&b));
    fs::write(
        a1.dir.join("src/a.js"),
        A_JS.replace("l3\n", "x3\n").replace("l4\n", "x4\n"),
    )
    .unwrap();
    fs::write(b1.dir.join("src/a.js"), A_JS.replace("l4\n", "y4\n")).unwrap();
    fx.w.workspaces.sync(&a1.key.id(), &a1.ws).unwrap();
    let n = fx.coord.notices(&b1.key.id(), &b1.ws, true).unwrap();
    assert_eq!(n.notices.len(), 1, "{n:?}");
    let notice = &n.notices[0];
    assert_eq!(notice.path, "src/a.js");
    assert_eq!(notice.overlap, Overlap::Lines);
    assert_eq!(notice.title.as_deref(), Some("Migrate sessions"));
    assert_eq!(notice.theirs[0].human(), "lines 3-4");
    assert_eq!(notice.mine[0].human(), "line 4");
    assert_eq!(n.tasks.len(), 1);
    assert_eq!(n.tasks[0].changed, vec!["src/a.js"]);
    assert_eq!(notice.merge, Some(aver::coord::Merge::Conflict));
    let text = aver::coord::agent_text(&n, false);
    assert!(text.contains("OVERLAP"), "{text}");
    assert!(
        text.contains(
            "\"Migrate sessions\" changed lines 3-4; you changed line 4; merging both CONFLICTS\n  theirs, at lines 3-4:\n    -l3\n    -l4\n    +x3\n    +x4\n  yours, at line 4:\n    -l4\n    +y4\n```\n"
        ),
        "{text}"
    );
    assert!(text.contains("`aver check --combined`"), "{text}");
}

/// The orchestrator view of a repository: every task with live
/// directories there, one edge for the pair whose changes overlap (the
/// third task changed nothing they did), and every lease; admin only.
#[test]
fn repo_view_lists_tasks_overlap_edges_and_leases() {
    let fx = Fx::new();
    let (a, b, c) = (
        fx.task("Migrate sessions"),
        fx.task("Rate limit"),
        fx.task("Docs"),
    );
    let (a1, a2, b1, c1) = (
        fx.attempt(&a),
        fx.attempt(&a),
        fx.attempt(&b),
        fx.attempt(&c),
    );
    fs::write(
        a1.dir.join("src/a.js"),
        A_JS.replace("l3\n", "x3\n").replace("l4\n", "x4\n"),
    )
    .unwrap();
    fs::write(b1.dir.join("src/a.js"), A_JS.replace("l4\n", "y4\n")).unwrap();
    fs::write(c1.dir.join("README.md"), "hello\n").unwrap();
    for att in [&a1, &a2, &b1, &c1] {
        fx.w.workspaces.sync(&att.key.id(), &att.ws).unwrap();
    }
    fx.lease(&c1, &["README.md"], LeaseMode::Exclusive).unwrap();

    let view = fx.coord.repo_view(&fx.w.admin.id(), &fx.repo).unwrap();
    assert_eq!(view.tasks.len(), 3, "{view:?}");
    let task_a = view.tasks.iter().find(|t| t.task == Some(a)).unwrap();
    assert_eq!(task_a.workspaces.len(), 2);
    assert_eq!(task_a.changed, vec!["src/a.js"]);
    assert_eq!(task_a.title.as_deref(), Some("Migrate sessions"));
    assert_eq!(view.edges.len(), 1, "{:?}", view.edges);
    let edge = &view.edges[0];
    let mut pair = [edge.a.unwrap(), edge.b.unwrap()];
    pair.sort();
    let mut want = [a, b];
    want.sort();
    assert_eq!(pair, want);
    assert_eq!(edge.worst, Overlap::Lines);
    assert_eq!(edge.paths.len(), 1);
    assert_eq!(edge.paths[0].path, "src/a.js");
    assert_eq!(edge.paths[0].merge, Some(aver::coord::Merge::Conflict));
    assert_eq!(view.leases.len(), 1);
    assert_eq!(view.leases[0].lease.task, Some(c));
    assert_eq!(view.leases[0].title.as_deref(), Some("Docs"));

    let text = aver::coord::repo_text(&view);
    assert!(
        text.contains(
            "OVERLAP src/a.js: tasks \"Migrate sessions\" and \"Rate limit\" change the same lines; merging both CONFLICTS"
        ) || text.contains(
            "OVERLAP src/a.js: tasks \"Rate limit\" and \"Migrate sessions\" change the same lines; merging both CONFLICTS"
        ),
        "{text}"
    );
    assert!(
        text.contains("task \"Docs\" (1 workspace) changes README.md"),
        "{text}"
    );
    assert!(text.contains("CONFLICTS: "), "{text}");
    assert!(text.contains("aver release --repo"), "{text}");

    let back: aver::coord::RepoView =
        serde_json::from_value(serde_json::to_value(&view).unwrap()).unwrap();
    assert_eq!(back, view);

    let err = fx.coord.repo_view(&a1.key.id(), &fx.repo).unwrap_err();
    assert_eq!(err.code(), "FORBIDDEN", "{err}");
    let empty = fx
        .coord
        .repo_view(&fx.w.admin.id(), &fx.w.dir.path().join("elsewhere"))
        .unwrap();
    assert!(empty.tasks.is_empty() && empty.edges.is_empty() && empty.leases.is_empty());
    assert!(aver::coord::repo_text(&empty).starts_with("No task has a live workspace directory"));
}

/// Both tasks only append to the end of one file: the merge conflicts, and
/// the notice says that moving one side's lines resolves it. Repository
/// text in the diff stays on its line.
#[test]
fn notice_same_place_insertions() {
    let fx = Fx::new();
    let (a, b) = (fx.task("A"), fx.task("B"));
    let (a1, b1) = (fx.attempt(&a), fx.attempt(&b));
    fs::write(a1.dir.join("src/a.js"), format!("{A_JS}theirs\u{1b}[2J\n")).unwrap();
    fs::write(b1.dir.join("src/a.js"), format!("{A_JS}mine\n")).unwrap();
    fx.w.workspaces.sync(&a1.key.id(), &a1.ws).unwrap();
    let n = fx.coord.notices(&b1.key.id(), &b1.ws, true).unwrap();
    assert_eq!(n.notices[0].merge, Some(aver::coord::Merge::SamePlace));
    let text = aver::coord::agent_text(&n, false);
    assert!(
        text.contains("you both add lines at the same place: combine keeps both"),
        "{text}"
    );
    assert!(text.contains("    +theirs\\u001b[2J\n"), "{text}");
    assert!(
        text.contains("SAME PLACE: you both only add lines"),
        "{text}"
    );
}

#[test]
fn notice_disjoint_lines() {
    let fx = Fx::new();
    let (a, b) = (fx.task("A"), fx.task("B"));
    let (a1, b1) = (fx.attempt(&a), fx.attempt(&b));
    fs::write(a1.dir.join("src/a.js"), A_JS.replace("l2\n", "x2\n")).unwrap();
    fs::write(b1.dir.join("src/a.js"), A_JS.replace("l9\n", "y9\n")).unwrap();
    fs::write(b1.dir.join("new.js"), "new\n").unwrap();
    fx.w.workspaces.sync(&a1.key.id(), &a1.ws).unwrap();
    let n = fx.coord.notices(&b1.key.id(), &b1.ws, true).unwrap();
    assert_eq!(n.notices.len(), 1, "{n:?}");
    assert_eq!(n.notices[0].overlap, Overlap::Disjoint);
    assert_eq!(n.notices[0].merge, Some(aver::coord::Merge::Clean));
    assert_eq!(n.notices[0].diff, None);
    let text = aver::coord::agent_text(&n, false);
    assert!(
        text.contains("(no overlap; merging both is clean)"),
        "{text}"
    );
    assert!(!text.contains("CONFLICTS"), "{text}");
    // The other side sees the same file from its end.
    let back = fx.coord.notices(&a1.key.id(), &a1.ws, true).unwrap();
    assert_eq!(back.notices[0].overlap, Overlap::Disjoint);
    assert_eq!(back.tasks[0].changed, vec!["new.js", "src/a.js"]);
}

#[test]
fn notices_name_changes_under_another_tasks_lease() {
    let fx = Fx::new();
    let (a, b) = (fx.task("A"), fx.task("B"));
    let (a1, b1) = (fx.attempt(&a), fx.attempt(&b));
    let id = fx.lease(&a1, &["src/**"], LeaseMode::Exclusive).unwrap();
    fs::write(b1.dir.join("src/a.js"), "rewritten\n").unwrap();
    let n = fx.coord.notices(&b1.key.id(), &b1.ws, true).unwrap();
    assert_eq!(n.leased.len(), 1);
    assert_eq!(n.leased[0].lease, id);
    assert_eq!(n.leases.len(), 1);
    let full = aver::coord::agent_text(&n, true);
    assert!(full.contains("LEASED"), "{full}");
    assert!(full.contains("holds an exclusive lease on"), "{full}");
}

/// An edit under another task's live exclusive lease reaches the verdict,
/// whatever agent tool made it (Claude Code's edit hook is only one of
/// them): verify adds a `lease::<path>` entry, cause CHANGED, so the
/// receipt is REVIEW at best, and the agent report says not done until the
/// edit is undone. A shared lease, and a path outside the lease, change
/// nothing; neither does the holder's own lease.
#[tokio::test]
async fn verify_marks_edits_under_another_tasks_exclusive_lease_for_review() {
    use aver::run::report::{agent_text, exit_code, ReportInput};
    use aver::verify::{Head, Request};
    let fx = Fx::with_files(&[
        ("aver.toml", &unit_toml("")),
        ("tests.json", r#"{"t": "pass"}"#),
        ("src/a.js", A_JS),
        ("README.md", "hi\n"),
    ]);
    let (a, b) = (fx.task("Migrate sessions"), fx.task("Rate limit"));
    let (a1, b1) = (fx.attempt(&a), fx.attempt(&b));
    let lease = fx.lease(&a1, &["src/**"], LeaseMode::Exclusive).unwrap();
    fs::write(b1.dir.join("src/a.js"), "rewritten\n").unwrap();
    fs::write(b1.dir.join("README.md"), "hello\n").unwrap();
    let verify = |att: &Att, task: TaskId| {
        let (caller, ws) = (att.key.id(), att.ws);
        let w = &fx.w;
        async move {
            let sync = w.workspaces.sync(&caller, &ws).unwrap();
            let v = w
                .verifier
                .verify(
                    caller,
                    Request {
                        task,
                        head: Head::Workspace(ws),
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
                .unwrap();
            (v, sync)
        }
    };
    let (v, sync) = verify(&b1, b).await;
    let receipt = &v.receipt;
    let leased = receipt.body.checks.get("lease::src/a.js").unwrap();
    assert_eq!(
        (leased.base, leased.head, leased.cause),
        (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::Changed))
    );
    let detail = leased.detail.as_ref().unwrap().as_str();
    assert!(detail.contains("\"Migrate sessions\""), "{detail}");
    assert!(detail.contains(&lease.to_string()), "{detail}");
    assert!(receipt.body.checks.get("lease::README.md").is_none());
    assert_eq!(receipt.body.verdict, Verdict::Review);
    common::assert_matches_schema("receipt", receipt);
    let input = ReportInput {
        receipt_id: &v.id,
        receipt,
        diagnostics: Some(&v.diagnostics),
        sync: sync.as_ref(),
        took: None,
        budget: None,
        notices: None,
        timing: None,
    };
    let text = agent_text(&input);
    assert!(
        text.starts_with("NOT DONE: REVIEW, because you changed 1 path another task holds an exclusive lease on."),
        "{text}"
    );
    assert!(text.contains("aver restore <path>"), "{text}");
    assert_eq!(exit_code(&input), 1);

    // The holder's own edit under its lease is its business.
    fs::write(a1.dir.join("src/a.js"), "mine\n").unwrap();
    let (v, _) = verify(&a1, a).await;
    assert!(v.receipt.body.checks.get("lease::src/a.js").is_none());
    assert_eq!(v.receipt.body.verdict, Verdict::Pass);

    // A shared lease only announces work: no entry.
    fx.coord.release(&a1.key.id(), &lease, false).unwrap();
    fx.lease(&a1, &["src/**"], LeaseMode::Shared).unwrap();
    let (v, _) = verify(&b1, b).await;
    assert!(v.receipt.body.checks.get("lease::src/a.js").is_none());
    assert_eq!(v.receipt.body.verdict, Verdict::Pass);
}

/// Stale work drops out of other tasks' notices, leases and trial merges
/// (§2.11): a workspace whose latest verify said FAIL_OWN of the files it
/// still holds (it counts again once it changes), and an abandoned
/// attempt's (a rerun replaced its run).
#[tokio::test]
async fn fail_own_and_abandoned_attempts_leave_notices_and_the_trial() {
    use aver::verify::{Head, Request};
    let fx = Fx::with_files(&[
        ("aver.toml", &unit_toml("")),
        ("tests.json", r#"{"t": "pass-if:src/keep.js"}"#),
        ("src/a.js", A_JS),
        ("src/keep.js", "kept\n"),
    ]);
    let (a, b, c) = (fx.task("Broke it"), fx.task("Mine"), fx.task("Replaced"));
    let (a1, b1, c1) = (fx.attempt(&a), fx.attempt(&b), fx.attempt(&c));
    fs::write(a1.dir.join("src/a.js"), A_JS.replace("l2\n", "a2\n")).unwrap();
    fs::remove_file(a1.dir.join("src/keep.js")).unwrap();
    fs::write(c1.dir.join("src/a.js"), A_JS.replace("l8\n", "c8\n")).unwrap();
    fs::write(b1.dir.join("src/a.js"), A_JS.replace("l5\n", "b5\n")).unwrap();
    fx.lease(&a1, &["docs/**"], LeaseMode::Shared).unwrap();
    fx.lease(&c1, &["lib/**"], LeaseMode::Shared).unwrap();
    for att in [&a1, &c1] {
        fx.w.workspaces.sync(&att.key.id(), &att.ws).unwrap();
    }
    let titles = |n: &aver::coord::Notices| -> Vec<String> {
        let mut t: Vec<String> = n
            .notices
            .iter()
            .filter_map(|x| x.title.clone())
            .chain(n.leases.iter().filter_map(|h| h.title.clone()))
            .collect();
        t.sort();
        t.dedup();
        t
    };
    let trial = || {
        let mine = fx.w.workspaces.snapshot(&b1.key.id(), &b1.ws).unwrap();
        let mut with = fx
            .coord
            .trial(&b1.key.id(), &b1.ws, &mine)
            .unwrap()
            .map(|t| t.with)
            .unwrap_or_default();
        with.sort();
        with
    };
    let n = fx.coord.notices(&b1.key.id(), &b1.ws, true).unwrap();
    assert_eq!(titles(&n), vec!["Broke it", "Replaced"], "{n:?}");
    assert_eq!(trial(), vec!["\"Broke it\"", "\"Replaced\""]);

    let v =
        fx.w.verifier
            .verify(
                a1.key.id(),
                Request {
                    task: a,
                    head: Head::Workspace(a1.ws),
                    attempt: Some(a1.attempt),
                    fresh: false,
                    all: false,
                    progress: None,
                    ci: None,
                    adhoc: None,
                    reason: None,
                },
            )
            .await
            .unwrap();
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn);
    fx.w.tasks.abandon(&[c1.attempt]).unwrap();
    let n = fx.coord.notices(&b1.key.id(), &b1.ws, true).unwrap();
    assert!(titles(&n).is_empty(), "{n:?}");
    assert!(n.tasks.is_empty(), "{n:?}");
    assert!(trial().is_empty());

    // A FAIL_OWN workspace that changed since its verify is live work again.
    fs::write(a1.dir.join("src/keep.js"), "back\n").unwrap();
    fx.w.workspaces.sync(&a1.key.id(), &a1.ws).unwrap();
    let n = fx.coord.notices(&b1.key.id(), &b1.ws, true).unwrap();
    assert_eq!(titles(&n), vec!["Broke it"], "{n:?}");
    assert_eq!(trial(), vec!["\"Broke it\""]);
}
