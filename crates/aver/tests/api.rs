//! `run::api::Api` in process (§2.7.1): it acts as the agent whose key it
//! is given. An ad-hoc check goes only with a dry run and names only its
//! command; a workspace, an attempt and a lease are their owner's, and an
//! overseer (`task:decide`, `admin`) reads every attempt and releases any
//! lease. No Docker.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use aver::agents::Capability;
use aver::coord::{LeaseMode, LeaseRequest};
use aver::keys::AgentKey;
use aver::run::api::{Api, ApiError, VerifyFlags};
use aver_core::{SnapshotId, Task, TaskId};
use run_support::*;

const WORKER: &[Capability] = &[
    Capability::TaskClaim,
    Capability::WorkspaceWrite,
    Capability::VerifyRun,
    Capability::LeaseWrite,
];

struct Setup {
    d: TestHome,
    api: Api,
    rt: tokio::runtime::Runtime,
    task: TaskId,
    base: SnapshotId,
}

fn setup() -> Setup {
    let d = TestHome::start();
    let api = d.api();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let repo = d.calc();
    let admin = d.admin.signing_key.clone();
    let imported = rt.block_on(api.import(&admin, &repo, "HEAD")).unwrap();
    let body = aver_core::TaskBody {
        schema: aver_core::TaskSchema::V2,
        title: aver_core::Text::truncated("Add negate"),
        intent: aver_core::Text::truncated("negate(x) returns -x"),
        base: imported.snapshot,
        target: None,
        criteria: aver_core::Criteria::new(Vec::new()).unwrap(),
        checks: aver_core::Set::empty(),
        protected_paths: aver_core::Set::empty(),
        author: d.admin.id(),
        created_at: aver_core::Timestamp::now(),
    };
    let task = rt
        .block_on(api.create_task(&Task::sign(body, &admin)))
        .unwrap();
    Setup {
        d,
        api,
        rt,
        task,
        base: imported.snapshot,
    }
}

impl Setup {
    fn agent(&self, name: &str, caps: &[Capability]) -> AgentKey {
        let key = AgentKey::generate(name.parse().unwrap());
        self.rt
            .block_on(self.api.register_agent(
                &self.d.admin.signing_key,
                name,
                &key.to_file().public_key,
                caps,
            ))
            .unwrap();
        key
    }
}

fn refused<T: std::fmt::Debug>(r: Result<T, ApiError>, code: &str) -> String {
    let e = r.unwrap_err();
    assert_eq!(e.code(), Some(code), "{e}");
    e.to_string()
}

#[test]
fn adhoc_without_dry_run_is_refused() {
    let s = setup();
    let worker = s.agent("worker", WORKER);
    let claim =
        s.rt.block_on(s.api.claim(&worker.signing_key, &s.task, None))
            .unwrap();
    let adhoc =
        |id: &str| aver_core::CheckDef::new(id.parse().unwrap(), vec!["exit".into(), "0".into()]);
    let verify = |flags: VerifyFlags| {
        s.rt.block_on(s.api.verify_with(
            &worker.signing_key,
            &s.task,
            &claim.workspace,
            flags,
            None,
        ))
    };
    let message = refused(
        verify(VerifyFlags {
            adhoc: Some(adhoc(aver::adhoc::ID)),
            ..VerifyFlags::default()
        }),
        "INVALID",
    );
    assert!(message.contains("dry_run"), "{message}");
    let message = refused(
        verify(VerifyFlags {
            dry_run: true,
            adhoc: Some(adhoc("unit")),
            ..VerifyFlags::default()
        }),
        "INVALID",
    );
    assert!(message.contains("an ad-hoc check has id"), "{message}");
    assert_eq!(s.d.sandbox.runs(), 0, "nothing ran");
}

#[test]
fn a_workspace_and_an_attempt_are_their_owners() {
    let s = setup();
    let worker = s.agent("worker", WORKER);
    let other = s.agent("other", WORKER);
    let claim =
        s.rt.block_on(s.api.claim(&worker.signing_key, &s.task, Some(s.base)))
            .unwrap();
    let ws = claim.workspace;
    let theirs = &other.signing_key;
    refused(s.rt.block_on(s.api.snapshot(theirs, &ws)), "FORBIDDEN");
    refused(s.rt.block_on(s.api.diff(theirs, &ws)), "FORBIDDEN");
    refused(s.rt.block_on(s.api.commit(theirs, &ws, "m")), "FORBIDDEN");
    refused(s.rt.block_on(s.api.discard(theirs, &ws)), "FORBIDDEN");
    refused(s.rt.block_on(s.api.sync(theirs, &ws)), "FORBIDDEN");
    refused(
        s.rt.block_on(s.api.restore(theirs, &ws, &["README.md".parse().unwrap()])),
        "FORBIDDEN",
    );
    refused(
        s.rt.block_on(s.api.verify(theirs, &s.task, &ws, false)),
        "FORBIDDEN",
    );
    refused(
        s.rt.block_on(s.api.attempt(theirs, &claim.attempt)),
        "FORBIDDEN",
    );
    assert_eq!(s.d.sandbox.runs(), 0, "a refused verify runs nothing");

    // The owner, and the admin as overseer, read the attempt.
    let mine =
        s.rt.block_on(s.api.attempt(&worker.signing_key, &claim.attempt))
            .unwrap();
    let overseen =
        s.rt.block_on(s.api.attempt(&s.d.admin.signing_key, &claim.attempt))
            .unwrap();
    assert_eq!(mine.id, overseen.id);
    s.rt.block_on(s.api.snapshot(&worker.signing_key, &ws))
        .unwrap();
}

#[test]
fn a_lease_is_released_by_its_holder_or_an_admin() {
    let s = setup();
    let worker = s.agent("worker", WORKER);
    let other = s.agent("other", WORKER);
    let claim =
        s.rt.block_on(s.api.claim(&worker.signing_key, &s.task, Some(s.base)))
            .unwrap();
    let held =
        s.rt.block_on(s.api.acquire_lease(
            &worker.signing_key,
            &claim.workspace,
            &LeaseRequest {
                paths: vec!["src/**".parse().unwrap()],
                mode: LeaseMode::default(),
                reason: "rewriting src".into(),
                ttl_ms: None,
            },
        ))
        .unwrap();
    let id = held.lease.id;
    refused(
        s.rt.block_on(s.api.release_lease(&other.signing_key, &id)),
        "FORBIDDEN",
    );
    s.rt.block_on(s.api.release_lease(&s.d.admin.signing_key, &id))
        .unwrap();
}

/// A task with criterion `div` on a repository whose scripted `unit` check
/// reads `tests.json`, where `divides` fails, and a worker's claim of it.
fn divide() -> (Setup, AgentKey, aver_core::WorkspaceId) {
    let s = setup();
    let repo = s.d.repo(
        "divide",
        &[
            ("aver.toml", &world::unit_toml("")),
            (
                "tests.json",
                r#"{"[crit:div] divides": "fail", "adds": "pass"}"#,
            ),
        ],
    );
    let admin = s.d.admin.signing_key.clone();
    let imported = s.rt.block_on(s.api.import(&admin, &repo, "HEAD")).unwrap();
    let body = aver_core::TaskBody {
        schema: aver_core::TaskSchema::V0,
        title: aver_core::Text::truncated("Implement divide"),
        intent: aver_core::Text::truncated("divide(a, b) returns a / b"),
        base: imported.snapshot,
        target: None,
        criteria: serde_json::from_value(serde_json::json!([
            { "id": "div", "text": "divide returns the quotient" }
        ]))
        .unwrap(),
        checks: aver_core::Set::empty(),
        protected_paths: aver_core::Set::empty(),
        author: s.d.admin.id(),
        created_at: aver_core::Timestamp::now(),
    };
    let task =
        s.rt.block_on(s.api.create_task(&Task::sign(body, &admin)))
            .unwrap();
    let worker = s.agent("worker", WORKER);
    let claim =
        s.rt.block_on(
            s.api
                .claim(&worker.signing_key, &task, Some(imported.snapshot)),
        )
        .unwrap();
    let s = Setup {
        task,
        base: imported.snapshot,
        ..s
    };
    (s, worker, claim.workspace)
}

/// The agent's own command, as `aver check -- <cmd>` makes it.
fn yours(extra: serde_json::Value) -> aver_core::CheckDef {
    let mut check = serde_json::json!({
        "id": aver::adhoc::ID,
        "argv": ["junit", "yours.xml", "tests.json"],
        "report": { "format": "junit", "path": "/aver/out/yours.xml" },
    });
    for (k, v) in extra.as_object().unwrap() {
        check[k] = v.clone();
    }
    serde_json::from_value(check).unwrap()
}

#[test]
fn an_adhoc_check_cannot_pick_its_image_or_setup() {
    let (s, worker, ws) = divide();
    let image = serde_json::json!({ "image": {
        "ref": "evil:latest",
        "digest": format!("sha256:{}", "0".repeat(64)),
    } });
    let setup = serde_json::json!({ "setup": [["sh", "-c", "true"]] });
    for extra in [image, setup] {
        refused(
            s.rt.block_on(s.api.verify_with(
                &worker.signing_key,
                &s.task,
                &ws,
                VerifyFlags {
                    dry_run: true,
                    adhoc: Some(yours(extra)),
                    ..VerifyFlags::default()
                },
                None,
            )),
            "INVALID",
        );
    }
    assert_eq!(s.d.sandbox.runs(), 0, "nothing ran");
}

/// A dry run with an ad-hoc check runs only that check, on both sides,
/// lists its runs in the diagnostics' timings (`aver check` reads the
/// transplant's back by them), and stores no receipt.
#[test]
fn an_adhoc_dry_run_runs_only_the_adhoc_check() {
    let (s, worker, ws) = divide();
    s.rt.block_on(s.api.edit(
        &worker.signing_key,
        &ws,
        &[(
            "tests.json".into(),
            br#"{"[crit:div] divides": "pass", "adds": "pass"}"#.to_vec(),
        )],
    ))
    .unwrap();
    let v =
        s.rt.block_on(s.api.verify_with(
            &worker.signing_key,
            &s.task,
            &ws,
            VerifyFlags {
                dry_run: true,
                adhoc: Some(yours(serde_json::json!({}))),
                ..VerifyFlags::default()
            },
            None,
        ))
        .unwrap();
    let receipt = serde_json::to_value(&v.receipt).unwrap();
    let checks = receipt["checks"].as_array().unwrap();
    let ids: Vec<&str> = checks.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert!(!ids.is_empty());
    assert!(ids.iter().all(|id| id.starts_with("yours")), "{ids:?}");
    let fixed = checks
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("divides"))
        .unwrap();
    assert_eq!(
        (&fixed["base"], &fixed["head"]),
        (&serde_json::json!("fail"), &serde_json::json!("pass"))
    );
    let diagnostics = serde_json::to_value(&v.diagnostics).unwrap();
    let runs = diagnostics["timings"]["runs"].as_array().unwrap();
    assert!(!runs.is_empty());
    assert!(runs.iter().all(|r| r["check"] == "yours"), "{runs:?}");
    assert!(
        s.d.store.get::<aver_core::Receipt>(&v.receipt_id).is_err(),
        "a dry run stores no receipt"
    );
}

/// A claim's workspace in the store, as the worker who owns it.
fn claimed(s: &Setup) -> (AgentKey, aver_core::WorkspaceId, aver_core::AttemptId) {
    let worker = s.agent("worker", WORKER);
    let claim =
        s.rt.block_on(s.api.claim(&worker.signing_key, &s.task, Some(s.base)))
            .unwrap();
    (worker, claim.workspace, claim.attempt)
}

#[test]
fn an_edit_is_atomic_and_refuses_conflicts_secrets_and_escapes() {
    let s = setup();
    let (worker, ws, _) = claimed(&s);
    let edit = |files: &[(&str, &[u8])]| {
        let files: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_vec()))
            .collect();
        s.rt.block_on(s.api.edit(&worker.signing_key, &ws, &files))
    };
    refused(
        edit(&[("ok.txt", b"x"), ("README.md/x", b"x")]),
        "PATH_CONFLICT",
    );
    refused(edit(&[("src", b"x")]), "PATH_CONFLICT");
    let key = format!("{}{}", "AKIA", "ABCDEFGHIJKLMNOP");
    let message = refused(
        edit(&[("src/config.js", format!("k='{key}'\n").as_bytes())]),
        "SECRET_DETECTED",
    );
    assert!(!message.contains(&key), "{message}");
    refused(edit(&[("../etc/passwd", b"x")]), "INVALID");
    assert_eq!(
        s.rt.block_on(s.api.snapshot(&worker.signing_key, &ws))
            .unwrap(),
        s.base,
        "no refused batch wrote any of its files"
    );
}

#[test]
fn commit_and_discard_close_the_workspace() {
    let s = setup();
    let (worker, ws, _) = claimed(&s);
    let me = &worker.signing_key;
    let note = || vec![("NOTES".to_string(), b"n\n".to_vec())];
    refused(s.rt.block_on(s.api.commit(me, &ws, "noop")), "NO_CHANGES");
    s.rt.block_on(s.api.edit(me, &ws, &note())).unwrap();
    let committed = s.rt.block_on(s.api.commit(me, &ws, "m")).unwrap();
    assert_eq!(
        s.rt.block_on(s.api.read_file(&committed.snapshot, "NOTES"))
            .unwrap(),
        b"n\n"
    );
    refused(
        s.rt.block_on(s.api.edit(me, &ws, &note())),
        "WORKSPACE_CLOSED",
    );
    refused(
        s.rt.block_on(s.api.commit(me, &ws, "m")),
        "WORKSPACE_CLOSED",
    );
    refused(s.rt.block_on(s.api.discard(me, &ws)), "WORKSPACE_CLOSED");

    let open = s.rt.block_on(s.api.open_workspace(me, &s.base)).unwrap();
    s.rt.block_on(s.api.edit(me, &open, &note())).unwrap();
    s.rt.block_on(s.api.discard(me, &open)).unwrap();
    s.rt.block_on(s.api.discard(me, &open))
        .expect("a second discard is a no-op");
    refused(
        s.rt.block_on(s.api.edit(me, &open, &note())),
        "WORKSPACE_CLOSED",
    );
    refused(
        s.rt.block_on(s.api.commit(me, &open, "m")),
        "WORKSPACE_CLOSED",
    );
}

#[test]
fn export_takes_only_an_absolute_repository_path() {
    let s = setup();
    for repo in ["relative/out.git", "--shared=0666"] {
        refused(
            s.rt.block_on(
                s.api
                    .export(&s.base, std::path::Path::new(repo), "main", None),
            ),
            "INVALID",
        );
    }
}

/// A verify without an attempt attaches to the workspace's own attempt,
/// and the stored receipt reads back as it was returned.
#[test]
fn a_verify_attaches_to_the_claims_attempt_and_reads_back() {
    let s = setup();
    let (worker, ws, attempt) = claimed(&s);
    let v =
        s.rt.block_on(s.api.verify(&worker.signing_key, &s.task, &ws, false))
            .unwrap();
    assert_eq!(v.receipt.body.attempt_id, Some(attempt));
    let r = s.rt.block_on(s.api.receipt(&v.receipt_id)).unwrap();
    assert_eq!(r.receipt_id, v.receipt_id);
    assert_eq!(r.receipt, v.receipt);
    assert_eq!(r.diagnostics, v.diagnostics);
}

#[test]
fn import_refuses_secrets_unless_told_to_warn_and_never_prints_them() {
    let d = TestHome::start();
    let token = format!("{}{}", "AKIA", "ABCDEFGHIJKLMNOP");
    let repo = d.repo(
        "leaky",
        &[
            ("aver.toml", &aver_toml()),
            ("src/config.js", &format!("exports.key = \"{token}\";\n")),
        ],
    );
    let repo = repo.to_str().unwrap();
    let refused = aver(&d.home, &d.root, &["import", repo], b"");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{stderr}");
    assert!(stderr.contains("src/config.js"), "{stderr}");
    assert!(!stderr.contains(&token), "{stderr}");
    let warned = aver(
        &d.home,
        &d.root,
        &["import", repo, "--secrets", "warn"],
        b"",
    );
    let stderr = String::from_utf8_lossy(&warned.stderr);
    assert!(warned.status.success(), "{stderr}");
    assert!(
        stderr.contains("src/config.js looks like it contains a secret (aws-access-key-id) (high severity); imported anyway"),
        "{stderr}"
    );
    assert!(!stderr.contains(&token), "{stderr}");
}

#[test]
fn workspace_new_warns_on_fixture_secrets_and_refuses_real_ones() {
    let d = TestHome::start();
    let token = format!("{}{}", "AKIA", "ABCDEFGHIJKLMNOP");
    let pem = format!("-----BEGIN RSA {}-----\nabc\n", "PRIVATE KEY");
    let fixtures = d.repo(
        "fixtures",
        &[
            ("aver.toml", &aver_toml()),
            ("tests/fixtures/key.pem", &pem),
            (".env.example", &format!("AWS_ACCESS_KEY_ID={token}\n")),
            (
                "testdata/creds.json",
                &format!("{{\"key\": \"{token}\"}}\n"),
            ),
        ],
    );
    let made = aver(
        &d.home,
        &fixtures,
        &["workspace", "new", "--no-deps", "--json"],
        b"",
    );
    let stderr = String::from_utf8_lossy(&made.stderr);
    assert!(made.status.success(), "{stderr}");
    for path in [
        ".env.example",
        "testdata/creds.json",
        "tests/fixtures/key.pem",
    ] {
        assert!(stderr.contains(path), "names {path}: {stderr}");
    }
    assert!(
        !stderr.contains(&token) && !stderr.contains("BEGIN RSA"),
        "{stderr}"
    );
    // `aver run` imports through the services' policy.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let api = d.api();
    let admin = d.admin.signing_key.clone();
    let imported = rt.block_on(api.import(&admin, &fixtures, "HEAD")).unwrap();
    assert_eq!(
        imported.secrets,
        [
            ".env.example",
            "testdata/creds.json",
            "tests/fixtures/key.pem"
        ]
    );

    let leaky = d.repo(
        "leaky",
        &[
            ("aver.toml", &aver_toml()),
            ("tests/fixtures/key.pem", &pem),
            ("src/config.js", &format!("exports.key = \"{token}\";\n")),
        ],
    );
    let refused = aver(
        &d.home,
        &leaky,
        &["workspace", "new", "--no-deps", "--json"],
        b"",
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{stderr}");
    assert!(stderr.contains("src/config.js"), "{stderr}");
    assert!(!stderr.contains("tests/fixtures/key.pem"), "{stderr}");
    assert!(!stderr.contains(&token), "{stderr}");
    assert!(rt.block_on(api.import(&admin, &leaky, "HEAD")).is_err());
}
