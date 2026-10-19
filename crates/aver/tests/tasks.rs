//! Tasks and attempts in process (§2.6): task creation checks, claim base
//! selection, attempts verified in the base's environment, and the records
//! and ledger entry `decide` writes.

mod common;
mod scripted;
mod world;

use aver::agents::Capability;
use aver::keys::AgentKey;
use aver::tasks::{AttemptState, Claim, TaskError};
use aver::verify::{Head, Request};
use aver_core::id::kind;
use aver_core::*;
use world::*;

const FAILING: &str = r#"{"[crit:div] divides": "fail", "adds": "pass"}"#;
const PASSING: &str = r#"{"[crit:div] divides": "pass", "adds": "pass"}"#;

struct Setup {
    w: World,
    worker: AgentKey,
    trunk: RefName,
    base: SnapshotId,
    task: TaskId,
}

fn setup() -> Setup {
    setup_with(&[])
}

fn setup_with(extra: &[(&str, &str)]) -> Setup {
    let w = World::new();
    let worker = w.agent(
        "worker",
        &[
            Capability::TaskClaim,
            Capability::VerifyRun,
            Capability::WorkspaceWrite,
        ],
    );
    let toml = unit_toml("");
    let mut files = vec![("aver.toml", toml.as_str()), ("tests.json", FAILING)];
    files.extend_from_slice(extra);
    let base = w.snapshot(&files);
    let trunk: RefName = "trunk".parse().unwrap();
    w.store.compare_and_set(&trunk, None, &base).unwrap();
    let mut body = w.task_body(&w.admin, base, &["div"]);
    body.target = Some(trunk.clone());
    let task = w.create_task(body);
    Setup {
        w,
        worker,
        trunk,
        base,
        task,
    }
}

async fn verify_attempt(s: &Setup, claim: &Claim) -> Receipt {
    s.w.verifier
        .verify(
            s.worker.id(),
            Request {
                task: s.task,
                head: Head::Workspace(claim.workspace),
                attempt: Some(claim.attempt),
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
        .receipt
}

fn ledger_chain(w: &World, scope: &str) -> Vec<LedgerEntry> {
    let head = w
        .store
        .get_ref::<kind::Ledger>(&format!("ledger/{scope}").parse().unwrap())
        .unwrap();
    let mut next = head.map(|r| r.target);
    let mut chain = Vec::new();
    while let Some(id) = next {
        let entry: LedgerEntry = w.store.get(&id).unwrap();
        entry.verify(&w.server.signing_key.verifying_key()).unwrap();
        assert_eq!(entry.body.scope, scope);
        next = entry.body.prev;
        chain.push(entry);
    }
    chain.reverse();
    chain
}

#[test]
fn task_creation_checks_the_signature_and_the_authors_capability() {
    let w = World::new();
    let base = w.snapshot(&[("aver.toml", &unit_toml("")), ("tests.json", "{}")]);
    let body = w.task_body(&w.admin, base, &["a"]);

    let mut tampered = Task::sign(body.clone(), &w.admin.signing_key);
    tampered.body.title = "Something else".parse().unwrap();
    let err = w.tasks.create(&tampered).unwrap_err();
    assert!(
        matches!(err, TaskError::Invalid(ref m) if m.contains("signature")),
        "{err}"
    );

    let impostor = w.agent("impostor", &[Capability::TaskCreate]);
    let forged = Task::sign(body.clone(), &impostor.signing_key);
    let err = w.tasks.create(&forged).unwrap_err();
    assert!(
        matches!(err, TaskError::Invalid(_)),
        "a signature by another key: {err}"
    );

    let planner = w.agent("planner", &[Capability::TaskClaim]);
    let unprivileged = Task::sign(w.task_body(&planner, base, &["a"]), &planner.signing_key);
    let err = w.tasks.create(&unprivileged).unwrap_err();
    assert!(
        matches!(err, TaskError::Forbidden(ref m) if m.contains("task:create")),
        "{err}"
    );

    let mut wrong_proves = w.task_body(&w.admin, base, &["a"]);
    wrong_proves.checks = Set::new(vec![TaskCheckDef {
        id: "accept".parse().unwrap(),
        argv: vec!["exit".into(), "0".into()],
        cwd: None,
        timeout_ms: None,
        report: None,
        proves: Set::new(vec!["zzz".parse().unwrap()]).unwrap(),
    }])
    .unwrap();
    let err = w
        .tasks
        .create(&Task::sign(wrong_proves, &w.admin.signing_key))
        .unwrap_err();
    assert!(
        matches!(err, TaskError::Invalid(ref m) if m.contains("zzz")),
        "{err}"
    );

    let task = Task::sign(body, &w.admin.signing_key);
    let id = w.tasks.create(&task).unwrap();
    assert_eq!(id, task.id());
    assert_eq!(
        w.tasks.create(&task).unwrap(),
        id,
        "creating twice is idempotent"
    );
    assert_eq!(w.tasks.get(&id).unwrap(), task);
}

#[test]
fn a_claim_starts_from_the_target_ref_else_the_task_base_unless_told() {
    let s = setup();
    let moved =
        s.w.snapshot(&[("aver.toml", &unit_toml("")), ("tests.json", PASSING)]);
    s.w.store
        .compare_and_set(&s.trunk, Some(&s.base), &moved)
        .unwrap();
    let claim = s.w.tasks.claim(&s.worker.id(), &s.task, None).unwrap();
    assert_eq!(claim.base, moved);
    let ws =
        s.w.workspaces
            .get(&s.worker.id(), &claim.workspace)
            .unwrap();
    assert_eq!(
        (ws.base, ws.owner, ws.attempt),
        (moved, s.worker.id(), Some(claim.attempt))
    );
    let attempt = s.w.tasks.attempt(&s.worker.id(), &claim.attempt).unwrap();
    assert_eq!(attempt.state, AttemptState::Open);
    assert_eq!(attempt.workspace, claim.workspace);

    let explicit =
        s.w.tasks
            .claim(&s.worker.id(), &s.task, Some(s.base))
            .unwrap();
    assert_eq!(explicit.base, s.base);

    let untargeted = s.w.create_task(s.w.task_body(&s.w.admin, s.base, &["x"]));
    assert_eq!(
        s.w.tasks
            .claim(&s.worker.id(), &untargeted, None)
            .unwrap()
            .base,
        s.base
    );

    let other = s.w.agent("other", &[Capability::TaskClaim]);
    let err = s.w.tasks.attempt(&other.id(), &claim.attempt).unwrap_err();
    assert!(matches!(err, TaskError::Forbidden(_)), "{err}");
}

/// REVIEW is for a human: the attempt stays open for `decide`.
#[tokio::test]
async fn a_review_verdict_leaves_the_attempt_open_for_decide() {
    let w = World::new();
    let worker = w.agent(
        "worker",
        &[
            Capability::TaskClaim,
            Capability::VerifyRun,
            Capability::WorkspaceWrite,
        ],
    );
    let toml = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n"
    );
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "test/parse.test.json",
            "{\n  \"keeps\": \"pass\",\n  \"parses\": \"fail\"\n}\n",
        ),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let claim = w.tasks.claim(&worker.id(), &task, None).unwrap();
    w.write(
        &worker,
        &claim.workspace,
        &[(
            "test/parse.test.json",
            "{\n  \"keeps\": \"pass\",\n  \"parses\": \"pass\"\n}\n",
        )],
    );
    let receipt = w
        .verifier
        .verify(
            worker.id(),
            Request {
                task,
                head: Head::Workspace(claim.workspace),
                attempt: Some(claim.attempt),
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
        .receipt;
    assert_eq!(receipt.body.verdict, Verdict::Review);
    let attempt = w.tasks.attempt(&worker.id(), &claim.attempt).unwrap();
    assert_eq!(attempt.state, AttemptState::Open);
    let decided = w
        .tasks
        .decide(&w.admin.id(), &task, &decision(claim.attempt))
        .unwrap();
    assert_eq!(decided.winner, claim.attempt);
}

/// The base's environment picks the checks that verify a head, so a worker
/// that points the check at a file of its choosing gets nothing for it: the
/// base's check runs on the real tests, and the edit to `aver.toml` is a
/// `NEW` judging-file entry, for a maintainer too.
#[tokio::test]
async fn a_head_that_rewrites_the_environment_is_judged_by_the_bases() {
    let s = setup_with(&[("forged.json", FAILING)]);
    let forged_toml = unit_toml("").replace("tests.json", "forged.json");
    let maintainer = s.w.agent(
        "maintainer",
        &[
            Capability::TaskClaim,
            Capability::VerifyRun,
            Capability::WorkspaceWrite,
            Capability::RefWrite,
        ],
    );
    for agent in [&s.worker, &maintainer] {
        let claim = s.w.tasks.claim(&agent.id(), &s.task, None).unwrap();
        s.w.write(
            agent,
            &claim.workspace,
            &[("aver.toml", &forged_toml), ("forged.json", PASSING)],
        );
        let receipt =
            s.w.verifier
                .verify(
                    agent.id(),
                    Request {
                        task: s.task,
                        head: Head::Workspace(claim.workspace),
                        attempt: Some(claim.attempt),
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
                .receipt;
        assert_eq!(receipt.body.verdict, Verdict::Unproven);
        assert!(receipt.body.env_changed);
        let judging = receipt
            .body
            .checks
            .get("judging::aver.toml")
            .expect("the aver.toml edit is reported");
        assert_eq!(judging.cause, Some(Cause::Changed));
        assert_eq!(
            receipt
                .body
                .checks
                .get("unit::[crit:div] divides")
                .unwrap()
                .head,
            HeadStatus::Fail,
            "the base's check read the real tests"
        );
    }
}

/// Verifies a worker's attempt that fixes the tests and also writes `files`.
async fn pass_with(s: &Setup, files: &[(&str, &str)]) -> Receipt {
    let claim = s.w.tasks.claim(&s.worker.id(), &s.task, None).unwrap();
    let mut all = vec![("tests.json", PASSING)];
    all.extend_from_slice(files);
    s.w.write(&s.worker, &claim.workspace, &all);
    verify_attempt(s, &claim).await
}

/// Any edit to `aver.toml` is a judging-file change, even one that leaves
/// the parsed environment unchanged.
#[tokio::test]
async fn a_head_that_edits_aver_toml_proves_nothing() {
    let s = setup();
    let toml = unit_toml("") + "# a comment changes no environment\n";
    let receipt = pass_with(&s, &[("aver.toml", &toml)]).await;
    assert!(!receipt.body.env_changed);
    assert_eq!(receipt.body.verdict, Verdict::Unproven);
    assert!(receipt.body.checks.get("judging::aver.toml").is_some());

    let receipt = pass_with(&s, &[]).await;
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

/// A lockfile comes from the head, so a dependency change installs: changing
/// it sets `env_changed` but is no judging change, and head runs use the
/// base environment with the head's lockfile.
#[tokio::test]
async fn a_head_that_edits_the_lockfile_runs_with_it() {
    let s = setup_with(&[("package.json", "{}\n"), ("package-lock.json", "{}\n")]);
    let lock = "{\"lockfileVersion\":3}\n";
    let receipt = pass_with(&s, &[("package-lock.json", lock)]).await;
    assert!(receipt.body.env_changed);
    assert_eq!(receipt.body.verdict, Verdict::Pass);
    assert!(receipt
        .body
        .checks
        .iter()
        .all(|e| !e.id.as_str().starts_with("judging::")));
    let entry = receipt.body.checks.get("unit::adds").unwrap();
    let run: CheckRun = s.w.store.get(&entry.runs.head[0]).unwrap();
    let env: Environment = s.w.store.get(&run.env).unwrap();
    assert_eq!(
        env.lockfile.unwrap().blob,
        BlobId::of_bytes(lock.as_bytes()),
        "the head ran with its own lockfile"
    );
    assert_eq!(
        receipt.body.environment_hash,
        s.w.store.get::<Snapshot>(&s.base).unwrap().env.unwrap(),
        "the receipt names the base environment"
    );
}

/// The same tree must get the same environment whether it arrives through a
/// workspace or an import, so adding the default root lockfile to a base
/// without one re-derives the environment.
#[test]
fn adding_the_default_lockfile_in_a_workspace_re_derives_the_environment() {
    let s = setup();
    let base_env: Environment =
        s.w.store
            .get(&s.w.store.get::<Snapshot>(&s.base).unwrap().env.unwrap())
            .unwrap();
    assert_eq!(base_env.lockfile, None);
    let claim = s.w.tasks.claim(&s.worker.id(), &s.task, None).unwrap();
    let lock = "{\"lockfileVersion\":3}\n";
    s.w.write(
        &s.worker,
        &claim.workspace,
        &[("package.json", "{}\n"), ("package-lock.json", lock)],
    );
    let head =
        s.w.workspaces
            .snapshot(&s.worker.id(), &claim.workspace)
            .unwrap();
    let head = s.w.store.get::<Snapshot>(&head).unwrap();
    let env: Environment = s.w.store.get(&head.env.unwrap()).unwrap();
    assert_eq!(
        env.lockfile.map(|l| l.blob),
        Some(BlobId::of_bytes(lock.as_bytes()))
    );
    let imported = aver::env::environment(
        &*s.w.store,
        &head.root,
        &aver::env::StaticResolver::default(),
        aver::env::Detection::Off,
    )
    .unwrap();
    assert_eq!(head.env, Some(imported.id()), "same tree, same environment");
}

#[test]
fn a_task_target_needs_the_authors_ref_write() {
    let w = World::new();
    let base = w.snapshot(&[("aver.toml", &unit_toml("")), ("tests.json", "{}")]);
    let planner = w.agent("planner", &[Capability::TaskCreate]);
    let mut body = w.task_body(&planner, base, &["a"]);
    body.target = Some("trunk".parse().unwrap());
    let err = w
        .tasks
        .create(&Task::sign(body.clone(), &planner.signing_key))
        .unwrap_err();
    assert!(
        matches!(err, TaskError::Forbidden(ref m) if m.contains("ref:write")),
        "{err}"
    );
    body.target = None;
    w.tasks
        .create(&Task::sign(body, &planner.signing_key))
        .unwrap();
}

fn decision(winner: AttemptId) -> aver::tasks::Decision {
    aver::tasks::Decision {
        winner,
        ranking: Vec::new(),
        rationale: Some("the only PASS".into()),
        lessons: vec!["agent 3 gave up: needs a dependency".into()],
    }
}

fn attempt_records(w: &World) -> i64 {
    rusqlite::Connection::open(w.store.path())
        .unwrap()
        .query_row("SELECT count(*) FROM objects WHERE kind = 'atr'", [], |r| {
            r.get(0)
        })
        .unwrap()
}

#[tokio::test]
async fn decide_records_all_attempts_and_one_ledger_entry() {
    let s = setup();
    let claim = || s.w.tasks.claim(&s.worker.id(), &s.task, None).unwrap();
    let (a, b, c) = (claim(), claim(), claim());
    s.w.write(&s.worker, &a.workspace, &[("tests.json", PASSING)]);
    assert_eq!(verify_attempt(&s, &a).await.body.verdict, Verdict::Pass);
    let rb = verify_attempt(&s, &b).await;
    assert_ne!(rb.body.verdict, Verdict::Pass);

    let decided =
        s.w.tasks
            .decide(&s.w.admin.id(), &s.task, &decision(a.attempt))
            .unwrap();
    assert!(!decided.already);
    let state = |id: &AttemptId| s.w.tasks.attempt(&s.worker.id(), id).unwrap().state;
    assert_eq!(state(&a.attempt), AttemptState::Accepted);
    assert_eq!(state(&b.attempt), AttemptState::Rejected);
    assert_eq!(state(&c.attempt), AttemptState::Abandoned);
    assert_eq!(
        attempt_records(&s.w),
        2,
        "one record per attempt with a receipt"
    );
    let ids: Vec<String> = rusqlite::Connection::open(s.w.store.path())
        .unwrap()
        .prepare("SELECT id FROM objects WHERE kind = 'atr'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for id in ids {
        let record: AttemptRecord = s.w.store.get(&id.parse().unwrap()).unwrap();
        common::assert_matches_schema("attempt", &record);
        let expected = if record.attempt == a.attempt {
            AttemptOutcome::Accepted
        } else {
            AttemptOutcome::Rejected
        };
        assert_eq!(
            (record.task, record.base, record.outcome),
            (s.task, s.base, expected)
        );
    }

    let chain = ledger_chain(&s.w, &format!("task:{}", s.task));
    assert_eq!(chain.len(), 1, "one entry for the decision");
    let entry = &chain[0].body;
    assert_eq!(entry.kind, LedgerKind::Accept);
    assert_eq!(entry.winner, Some(a.attempt));
    assert_eq!(entry.attempts.len(), 2);
    assert_eq!(entry.rationale, "the only PASS");
    assert!(entry
        .lessons
        .iter()
        .any(|l| l.starts_with(&b.attempt.to_string())));
    assert!(entry.lessons.iter().any(|l| l.contains("gave up")));
    assert_eq!(
        s.w.store
            .get_ref::<kind::Snapshot>(&s.trunk)
            .unwrap()
            .unwrap()
            .target,
        s.base,
        "deciding moves no ref"
    );
}

#[tokio::test]
async fn decide_is_idempotent_and_refuses_another_winner() {
    let s = setup();
    let a = s.w.tasks.claim(&s.worker.id(), &s.task, None).unwrap();
    let b = s.w.tasks.claim(&s.worker.id(), &s.task, None).unwrap();
    s.w.write(&s.worker, &a.workspace, &[("tests.json", PASSING)]);
    verify_attempt(&s, &a).await;
    verify_attempt(&s, &b).await;
    let decide = |w: AttemptId| s.w.tasks.decide(&s.w.admin.id(), &s.task, &decision(w));
    let first = decide(a.attempt).unwrap();
    let again = decide(a.attempt).unwrap();
    assert!(again.already && again.ledger.is_none() && first.ledger.is_some());
    assert_eq!(ledger_chain(&s.w, &format!("task:{}", s.task)).len(), 1);
    assert!(matches!(
        decide(b.attempt),
        Err(TaskError::AlreadyDecided(_))
    ));

    // A winner whose receipt no longer describes its workspace is refused.
    let s2 = setup();
    let x = s2.w.tasks.claim(&s2.worker.id(), &s2.task, None).unwrap();
    verify_attempt(&s2, &x).await;
    s2.w.write(&s2.worker, &x.workspace, &[("tests.json", PASSING)]);
    assert!(matches!(
        s2.w.tasks
            .decide(&s2.w.admin.id(), &s2.task, &decision(x.attempt)),
        Err(TaskError::StaleReceipt(_))
    ));
}

#[tokio::test]
async fn a_receipt_against_another_base_is_not_attached_or_accepted() {
    let s = setup();
    let claim = s.w.tasks.claim(&s.worker.id(), &s.task, None).unwrap();
    s.w.write(&s.worker, &claim.workspace, &[("tests.json", PASSING)]);
    let head =
        s.w.workspaces
            .snapshot(&s.worker.id(), &claim.workspace)
            .unwrap();
    let elsewhere = s.w.snapshot(&[
        ("aver.toml", &unit_toml("")),
        ("tests.json", FAILING),
        ("other.js", "1"),
    ]);
    let verified =
        s.w.verifier
            .verify(
                s.worker.id(),
                Request {
                    task: s.task,
                    head: Head::Snapshot {
                        head,
                        base: Some(elsewhere),
                    },
                    attempt: Some(claim.attempt),
                    fresh: false,
                    all: false,
                    progress: None,
                    ci: None,
                    adhoc: None,
                    reason: None,
                },
            )
            .await;
    assert!(
        matches!(verified, Err(aver::verify::VerifyError::Invalid(_))),
        "a receipt against a base the attempt is not on must not join it: {:?}",
        verified.map(|v| v.receipt.body.verdict)
    );
    let attempt = s.w.tasks.attempt(&s.worker.id(), &claim.attempt).unwrap();
    assert!(attempt.receipts.is_empty());
    assert!(matches!(
        s.w.tasks
            .decide(&s.w.admin.id(), &s.task, &decision(claim.attempt)),
        Err(TaskError::StaleReceipt(_))
    ));
}
