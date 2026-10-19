//! The verification engine over the scripted sandbox: cause attribution,
//! granularity, criterion tags, task checks, reruns, caching, signatures and
//! schema validity. No Docker needed; `verify_docker.rs` covers the fixtures.

mod common;
mod scripted;
mod world;

use aver::verify::{Head, Request, VerifyError};
use aver_core::*;
use common::assert_matches_schema;
use world::*;

fn s(v: &[(&str, &str, &str, &str)]) -> Vec<(String, String, String, String)> {
    v.iter()
        .map(|(a, b, c, d)| (a.to_string(), b.to_string(), c.to_string(), d.to_string()))
        .collect()
}

/// A task without criteria judges only regressions: PREEXISTING and FLAKY
/// failures pass, a NEW one is FAIL_OWN, an ENV one is BLOCKED_NOT_OWN, and
/// the receipt is `aver.receipt/v1`.
#[tokio::test]
async fn a_regression_only_task_passes_unless_a_check_is_new_or_env() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"keeps": "pass", "legacy": "fail", "[crit:a] tagged": "pass"}"#,
        ),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let stored: Task = w.store.get(&task).unwrap();
    assert_eq!(stored.body.schema, TaskSchema::V2);
    assert_matches_schema("task", &stored);

    let verdict_for = |tests: &'static str, extra: &'static str| {
        let toml = unit_toml(extra);
        let head = w.snapshot(&[("aver.toml", &toml), ("tests.json", tests)]);
        let w = &w;
        async move { w.verify(&w.admin, task, head, base).await.receipt }
    };

    let receipt = verdict_for(
        r#"{"keeps": "pass", "legacy": "fail", "flaky": "flaky", "[crit:a] tagged": "pass"}"#,
        "",
    )
    .await;
    assert_eq!(
        entry(&receipt, "unit::flaky").cause,
        Some(Cause::New),
        "a test the head adds that flakes is judged failing"
    );
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);

    let receipt = verdict_for(
        r#"{"keeps": "pass", "legacy": "fail", "[crit:a] tagged": "pass"}"#,
        "",
    )
    .await;
    assert_eq!(receipt.body.schema, ReceiptSchema::V3);
    assert!(receipt.body.criteria.is_empty() && receipt.body.unproven.is_empty());
    assert!(
        proves(entry(&receipt, "unit::[crit:a] tagged")).is_empty(),
        "tags prove nothing without criteria"
    );
    assert_eq!(receipt.body.verdict, Verdict::Pass);
    assert_matches_schema("receipt", &receipt);

    let receipt = verdict_for(
        r#"{"keeps": "fail", "legacy": "fail", "[crit:a] tagged": "pass"}"#,
        "",
    )
    .await;
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);

    let slow = unit_toml("\n[[check]]\nid = \"slow\"\nrun = [\"timeout\"]\n");
    let tests = r#"{"keeps": "pass", "legacy": "fail", "[crit:a] tagged": "pass"}"#;
    let slow_base = w.snapshot(&[("aver.toml", &slow), ("tests.json", tests)]);
    let head = w.snapshot(&[("aver.toml", &slow), ("tests.json", tests)]);
    let receipt = w.verify(&w.admin, task, head, slow_base).await.receipt;
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
}

#[tokio::test]
async fn every_failure_is_attributed_per_test_and_tags_prove_only_this_tasks_criteria() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"[crit:a] adds": "fail", "math > keeps": "pass", "legacy": "fail", "gone": "pass", "[crit:other] elsewhere": "pass", "ignored": "skip"}"#,
        ),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"[crit:a] adds": "pass", "math > keeps": "fail", "legacy": "fail", "[crit:b] new": "pass", "[crit:other] elsewhere": "pass", "ignored": "skip"}"#,
        ),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &["a", "b", "c"]));
    let verified = w.verify(&w.admin, task, head, base).await;
    let receipt = &verified.receipt;

    assert_eq!(
        summary(receipt),
        s(&[
            ("unit::[crit:a] adds", "fail", "pass", "null"),
            ("unit::[crit:b] new", "n/a", "pass", "null"),
            ("unit::[crit:other] elsewhere", "pass", "pass", "null"),
            ("unit::gone", "pass", "fail", "NEW"),
            ("unit::legacy", "fail", "fail", "PREEXISTING"),
            ("unit::math > keeps", "pass", "fail", "NEW"),
        ])
    );
    assert_eq!(proves(entry(receipt, "unit::[crit:a] adds")), ["a"]);
    assert_eq!(proves(entry(receipt, "unit::[crit:b] new")), ["b"]);
    assert!(proves(entry(receipt, "unit::[crit:other] elsewhere")).is_empty());
    let detail = |id| {
        entry(receipt, id)
            .detail
            .as_ref()
            .map(|d| d.as_str().to_owned())
    };
    assert_eq!(
        detail("unit::gone").as_deref(),
        Some("missing from head report")
    );
    assert_eq!(detail("unit::legacy").as_deref(), Some("legacy failed"));
    assert_eq!(detail("unit::[crit:a] adds"), None);
    let unit = entry(receipt, "unit::legacy");
    assert_eq!(unit.source, CheckSource::Env);
    assert_eq!(
        (unit.runs.base.len(), unit.runs.head.len()),
        (3, 3),
        "failing sides rerun to three"
    );
    // `b`'s one proof is absent from the transplant (`tests.json` is no test
    // file here, so the transplant keeps the base's): a weak proof, which
    // alone never proves (§1.9.4).
    assert_eq!(
        receipt.body.unproven,
        vec![
            "b".parse::<CriterionId>().unwrap(),
            "c".parse::<CriterionId>().unwrap()
        ]
    );
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
    assert_eq!(
        receipt.body.environment_hash,
        w.store.get::<Snapshot>(&head).unwrap().env.unwrap()
    );
    assert!(!receipt.body.env_changed);
    assert_eq!(receipt.body.attempt_id, None);
    assert_eq!(receipt.body.agent_id, w.admin.id());
    assert_eq!(
        receipt.body.reproduce,
        format!("aver verify --task {task} --head {head} --base {base} --fresh")
    );

    receipt
        .verify(&w.server.signing_key.verifying_key())
        .unwrap();
    assert_eq!(receipt.signature.key, w.server.id());
    assert_eq!(verified.id, receipt.id());
    assert_eq!(w.store.get::<Receipt>(&verified.id).unwrap(), *receipt);
    assert_matches_schema("receipt", receipt);
    for run in &unit.runs.head {
        assert_matches_schema("run", &w.store.get::<CheckRun>(run).unwrap());
    }
}

#[tokio::test]
async fn a_second_verify_starts_no_sandbox_and_fresh_reruns_everything() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", r#"{"t": "fail"}"#)]);
    let head = w.snapshot(&[("aver.toml", &toml), ("tests.json", r#"{"t": "pass"}"#)]);
    let task = w.create_task(w.task_body(&w.admin, base, &["a"]));
    let first = w.verify(&w.admin, task, head, base).await;
    let starts = w.executor.sandbox_starts();
    assert_eq!(starts, 4, "base fails three times, head passes once");

    let second = w.verify(&w.admin, task, head, base).await;
    assert_eq!(
        w.executor.sandbox_starts(),
        starts,
        "every run is a cache hit"
    );
    assert_eq!(first.receipt.body.checks, second.receipt.body.checks);

    let fresh = w
        .verifier
        .verify(
            w.admin.id(),
            Request {
                task,
                head: Head::Snapshot {
                    head,
                    base: Some(base),
                },
                attempt: None,
                fresh: true,
                all: false,
                progress: None,
                ci: None,
                adhoc: None,
                reason: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(w.executor.sandbox_starts(), starts + 4);
    assert_eq!(summary(&fresh.receipt), summary(&first.receipt));
}

/// A head whose first run fails only where every base run failed is not
/// rerun: reruns could only make those PREEXISTING entries FLAKY, which
/// changes no verdict. A failure of the head's own still reruns to three.
#[tokio::test]
async fn a_head_failing_only_where_the_base_always_failed_runs_once() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"legacy": "fail", "keeps": "pass"}"#),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &["a"]));
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"legacy": "fail", "keeps": "pass", "[crit:a] adds": "pass"}"#,
        ),
    ]);
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let legacy = entry(&receipt, "unit::legacy");
    assert_eq!(
        (legacy.cause, legacy.runs.base.len(), legacy.runs.head.len()),
        (Some(Cause::Preexisting), 3, 1)
    );
    // `a`'s proof is absent from the transplant, a weak proof (§1.9.4).
    assert_eq!(receipt.body.verdict, Verdict::Unproven);

    let head = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"legacy": "fail", "keeps": "fail", "[crit:a] adds": "pass"}"#,
        ),
    ]);
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let keeps = entry(&receipt, "unit::keeps");
    assert_eq!(
        (keeps.cause, keeps.runs.head.len()),
        (Some(Cause::New), 3),
        "a failure the base does not explain is rerun"
    );
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
}

#[tokio::test]
async fn a_test_that_disagrees_with_itself_is_flaky_and_blocks_its_criterion() {
    let w = World::new();
    let toml = unit_toml("");
    let script = r#"{"[crit:a] eventually consistent": "flaky", "stable": "pass"}"#;
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", script)]);
    let head = w.snapshot(&[("aver.toml", &toml), ("tests.json", script), ("x", "1")]);
    let task = w.create_task(w.task_body(&w.admin, base, &["a"]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(
        summary(&receipt),
        s(&[
            (
                "unit::[crit:a] eventually consistent",
                "flaky",
                "flaky",
                "FLAKY"
            ),
            ("unit::stable", "pass", "pass", "null"),
        ])
    );
    let flaky = entry(&receipt, "unit::[crit:a] eventually consistent");
    assert_eq!(
        (flaky.runs.base.len(), flaky.runs.head.len()),
        (2, 2),
        "a disagreeing side stops rerunning"
    );
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
}

#[tokio::test]
async fn a_check_that_times_out_is_retried_once_then_env_and_blocks() {
    let w = World::new();
    let toml = unit_toml("\n[[check]]\nid = \"slow\"\nrun = [\"timeout\"]\n");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"[crit:a] t": "pass"}"#),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"[crit:a] t": "pass"}"#),
        ("x", "1"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &["a"]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let slow = entry(&receipt, "slow");
    assert_eq!(slow.cause, Some(Cause::Env));
    assert_eq!((slow.runs.base.len(), slow.runs.head.len()), (2, 2));
    assert!(
        slow.detail.as_ref().unwrap().as_str().contains("timeout"),
        "{:?}",
        slow.detail
    );
    assert_eq!(
        receipt.body.unproven,
        ["a".parse::<CriterionId>().unwrap()],
        "the tagged test passes on the base too, so it proves nothing"
    );
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
}

#[tokio::test]
async fn a_side_without_a_report_makes_one_command_level_entry() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", r#"{"t": "pass"}"#)]);
    let head = w.snapshot(&[("aver.toml", &toml)]);
    let task = w.create_task(w.task_body(&w.admin, base, &["a"]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(summary(&receipt), s(&[("unit", "pass", "fail", "NEW")]));
    let detail = entry(&receipt, "unit")
        .detail
        .as_ref()
        .unwrap()
        .as_str()
        .to_owned();
    assert!(detail.contains("cannot find tests.json"), "{detail}");
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
}

/// A task body protecting `globs`, written as `aver.task/v1`.
fn protecting(w: &World, base: SnapshotId, criteria: &[&str], globs: &[&str]) -> TaskBody {
    let mut body = w.task_body(&w.admin, base, criteria);
    body.protected_paths = Set::new(globs.iter().map(|g| g.parse().unwrap()).collect()).unwrap();
    body.schema = TaskSchema::for_task(&body.criteria, &body.protected_paths);
    assert_eq!(body.schema, TaskSchema::V1);
    body
}

/// Rewriting the tests that prove a criterion would make any head pass; a
/// task that protects them turns every add, change or delete there into a
/// `NEW` failure, whatever the checks say.
#[tokio::test]
async fn changing_a_protected_path_fails_as_new_whatever_the_checks_say() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"[crit:a] works": "fail", "keeps": "pass"}"#,
        ),
        ("spec/old.txt", "old"),
        ("spec/keep.txt", "keep"),
        ("src/a.js", "a"),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"[crit:a] works": "pass", "keeps": "pass"}"#,
        ),
        ("spec/keep.txt", "edited"),
        ("spec/new.txt", "new"),
        ("src/a.js", "b"),
    ]);
    let task = w.create_task(protecting(&w, base, &["a"], &["spec", "tests.json"]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;

    assert_eq!(
        summary(&receipt),
        s(&[
            ("protected::spec/keep.txt", "pass", "fail", "NEW"),
            ("protected::spec/new.txt", "pass", "fail", "NEW"),
            ("protected::spec/old.txt", "pass", "fail", "NEW"),
            ("protected::tests.json", "pass", "fail", "NEW"),
            ("unit::[crit:a] works", "fail", "pass", "null"),
            ("unit::keeps", "pass", "pass", "null"),
        ])
    );
    let detail = |id| {
        entry(&receipt, id)
            .detail
            .as_ref()
            .unwrap()
            .as_str()
            .to_owned()
    };
    assert_eq!(
        detail("protected::spec/keep.txt"),
        "modified protected path spec/keep.txt (matches spec)"
    );
    assert_eq!(
        detail("protected::spec/new.txt"),
        "added protected path spec/new.txt (matches spec)"
    );
    assert_eq!(
        detail("protected::spec/old.txt"),
        "deleted protected path spec/old.txt (matches spec)"
    );
    let keep = entry(&receipt, "protected::spec/keep.txt");
    assert_eq!(keep.source, CheckSource::Task);
    assert!(keep.runs.base.is_empty() && keep.runs.head.is_empty());
    assert!(receipt.body.unproven.is_empty(), "the criterion is proven");
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
    assert_matches_schema("receipt", &receipt);

    let clean = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"[crit:a] works": "fail", "keeps": "pass"}"#,
        ),
        ("spec/old.txt", "old"),
        ("spec/keep.txt", "keep"),
        ("src/a.js", "fixed"),
    ]);
    let receipt = w.verify(&w.admin, task, clean, base).await.receipt;
    assert!(
        receipt
            .body
            .checks
            .iter()
            .all(|c| !c.id.as_str().starts_with("protected::")),
        "changes outside protected paths add no entries"
    );
}

#[tokio::test]
async fn a_file_replaced_by_a_directory_under_a_protected_path_is_caught() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"t": "pass"}"#),
        ("fixtures", "x"),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"t": "pass"}"#),
        ("fixtures/a.json", "{}"),
    ]);
    let task = w.create_task(protecting(&w, base, &["a"], &["fixtures/**"]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let ids: Vec<&str> = receipt.body.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "protected::fixtures",
            "protected::fixtures/a.json",
            "unit::t"
        ]
    );
}

#[tokio::test]
async fn protected_paths_reserve_the_protected_check_id() {
    let w = World::new();
    let base = w.snapshot(&[("aver.toml", &unit_toml("")), ("tests.json", "{}")]);
    let mut body = protecting(&w, base, &["a"], &["tests.json"]);
    body.checks = Set::new(vec![TaskCheckDef {
        id: "protected".parse().unwrap(),
        argv: vec!["exit".into(), "0".into()],
        cwd: None,
        timeout_ms: None,
        report: None,
        proves: Set::new(vec!["a".parse().unwrap()]).unwrap(),
    }])
    .unwrap();
    let err = w
        .tasks
        .create(&Task::sign(body, &w.admin.signing_key))
        .unwrap_err();
    assert!(err.to_string().contains("reserved"), "{err}");

    let reserving = w.snapshot(&[
        (
            "aver.toml",
            &unit_toml("\n[[check]]\nid = \"protected\"\nrun = [\"exit\", \"0\"]\n"),
        ),
        ("tests.json", "{}"),
    ]);
    let err = w
        .tasks
        .create(&Task::sign(
            protecting(&w, reserving, &["a"], &["tests.json"]),
            &w.admin.signing_key,
        ))
        .unwrap_err();
    assert!(
        err.to_string().contains("reserved"),
        "a base environment defining the reserved check would make every verify INVALID: {err}"
    );

    let task = w.create_task(protecting(&w, base, &["a"], &["tests.json"]));
    let err = w
        .verifier
        .verify(
            w.admin.id(),
            Request {
                task,
                head: Head::Snapshot {
                    head: base,
                    base: Some(reserving),
                },
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
        .unwrap_err();
    assert!(
        matches!(err, VerifyError::Invalid(ref m) if m.contains("reserve")),
        "{err}"
    );
}

#[tokio::test]
async fn a_task_check_proves_its_criteria_and_is_marked_as_the_tasks() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"t": "pass"}"#),
        ("TODO", "x"),
    ]);
    let head = w.snapshot(&[("aver.toml", &toml), ("tests.json", r#"{"t": "pass"}"#)]);
    let mut body = w.task_body(&w.admin, base, &["done"]);
    body.checks = Set::new(vec![TaskCheckDef {
        id: "accept".parse().unwrap(),
        argv: vec!["exit-if".into(), "TODO".into()],
        cwd: None,
        timeout_ms: None,
        report: None,
        proves: Set::new(vec!["done".parse().unwrap()]).unwrap(),
    }])
    .unwrap();
    let task = w.create_task(body);
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let accept = entry(&receipt, "accept");
    assert_eq!(accept.source, CheckSource::Task);
    assert_eq!(proves(accept), ["done"]);
    assert_eq!(
        summary(&receipt),
        s(&[
            ("accept", "fail", "pass", "null"),
            ("unit::t", "pass", "pass", "null")
        ])
    );
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

#[tokio::test]
async fn task_checks_may_not_reuse_an_environment_check_id() {
    let w = World::new();
    let base = w.snapshot(&[("aver.toml", &unit_toml("")), ("tests.json", "{}")]);
    let mut body = w.task_body(&w.admin, base, &["a"]);
    let check = |id: &str| TaskCheckDef {
        id: id.parse().unwrap(),
        argv: vec!["exit".into(), "0".into()],
        cwd: None,
        timeout_ms: None,
        report: None,
        proves: Set::new(vec!["a".parse().unwrap()]).unwrap(),
    };
    body.checks = Set::new(vec![check("unit")]).unwrap();
    let err = w
        .tasks
        .create(&Task::sign(body.clone(), &w.admin.signing_key))
        .unwrap_err();
    assert!(err.to_string().contains("collides"), "{err}");

    body.checks = Set::new(vec![check("judging")]).unwrap();
    let err = w
        .tasks
        .create(&Task::sign(body.clone(), &w.admin.signing_key))
        .unwrap_err();
    assert!(err.to_string().contains("reserved"), "{err}");

    body.checks = Set::new(vec![check("lint")]).unwrap();
    let task = w.create_task(body);
    let other_base = w.snapshot(&[
        (
            "aver.toml",
            &unit_toml("\n[[check]]\nid = \"lint\"\nrun = [\"exit\", \"0\"]\n"),
        ),
        ("tests.json", "{}"),
    ]);
    let err = w
        .verifier
        .verify(
            w.admin.id(),
            Request {
                task,
                head: Head::Snapshot {
                    head: base,
                    base: Some(other_base),
                },
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
        .unwrap_err();
    assert!(
        matches!(err, VerifyError::Invalid(ref m) if m.contains("lint")),
        "{err}"
    );
}

/// The head's `aver.toml` sets `MODE = "lenient"`, which would make the
/// task check pass, and adds a check of its own. Every check runs in the
/// base's environment on both sides, so the criterion stays unproven, the
/// head's check does not run, and the edit is a CHANGED judging-file entry.
#[tokio::test]
async fn every_check_runs_in_the_bases_environment_on_both_sides() {
    let w = World::new();
    let toml =
        |mode: &str, extra: &str| unit_toml(&format!("{extra}\n[vars]\nMODE = \"{mode}\"\n"));
    let base = w.snapshot(&[
        ("aver.toml", &toml("strict", "")),
        ("tests.json", r#"{"t": "pass"}"#),
    ]);
    let head = w.snapshot(&[
        (
            "aver.toml",
            &toml(
                "lenient",
                "\n[[check]]\nid = \"extra\"\nrun = [\"exit\", \"0\"]\n",
            ),
        ),
        ("tests.json", r#"{"t": "pass"}"#),
    ]);
    let mut body = w.task_body(&w.admin, base, &["done"]);
    body.checks = Set::new(vec![TaskCheckDef {
        id: "accept".parse().unwrap(),
        argv: vec!["exit-unless-var".into(), "MODE".into(), "lenient".into()],
        cwd: None,
        timeout_ms: None,
        report: None,
        proves: Set::new(vec!["done".parse().unwrap()]).unwrap(),
    }])
    .unwrap();
    let task = w.create_task(body);
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert!(receipt.body.env_changed);

    let accept = entry(&receipt, "accept");
    assert_eq!(
        (accept.base, accept.head, accept.cause),
        (BaseStatus::Fail, HeadStatus::Fail, Some(Cause::Preexisting))
    );
    let base_env = w.store.get::<Snapshot>(&base).unwrap().env.unwrap();
    let unit = entry(&receipt, "unit::t");
    for run in accept
        .runs
        .base
        .iter()
        .chain(&accept.runs.head)
        .chain(&unit.runs.base)
        .chain(&unit.runs.head)
    {
        assert_eq!(w.store.get::<CheckRun>(run).unwrap().env, base_env);
    }
    let head_run: CheckRun = w.store.get(&unit.runs.head[0]).unwrap();
    assert_ne!(
        head_run.snapshot, head,
        "the head ran with the base's aver.toml"
    );
    assert!(receipt.body.checks.get("extra").is_none());
    assert_eq!(receipt.body.environment_hash, base_env);
    assert_eq!(
        entry(&receipt, "judging::aver.toml").cause,
        Some(Cause::Changed)
    );
    assert_eq!(
        receipt.body.unproven,
        vec!["done".parse::<CriterionId>().unwrap()]
    );
    // The task check fails on both sides with no per-test results, so it
    // hides what the change did: inconclusive, not a review.
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
}

/// A change to source files no check can exercise (here Rust and Python,
/// outside the only check's `covers`) is not a PASS: the receipt lists them
/// and says UNPROVEN, and so do the diagnostics and the agent report.
/// Documentation and files a check's `covers` names are covered.
#[tokio::test]
async fn changes_no_check_covers_hold_the_verdict_at_unproven() {
    use aver::run::report::{agent_text, exit_code, ReportInput};
    let w = World::new();
    let tests = r#"{"keeps": "pass"}"#;
    let toml = unit_toml("covers = [\"src/**\", \"tests.json\"]\n");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("src/a.js", "1"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let verify = |files: Vec<(&'static str, String)>| {
        let w = &w;
        async move {
            let mut all = vec![
                ("tests.json", tests.to_owned()),
                ("src/a.js", "1".to_owned()),
            ];
            all.extend(files);
            let refs: Vec<(&str, &str)> = all.iter().map(|(p, c)| (*p, c.as_str())).collect();
            let head = w.snapshot(&refs);
            w.verify(&w.admin, task, head, base).await
        }
    };

    let covered = verify(vec![
        ("aver.toml", toml.clone()),
        ("src/b.ts", "2".into()),
        ("README.md", "docs".into()),
    ])
    .await;
    assert_eq!(covered.receipt.body.verdict, Verdict::Pass);
    assert!(covered.receipt.body.uncovered.is_none());

    let uncovered = verify(vec![
        ("aver.toml", toml.clone()),
        ("native/src/lib.rs", "fn main() {}".into()),
        ("tools/gen.py", "print(1)".into()),
    ])
    .await;
    let body = &uncovered.receipt.body;
    assert_eq!(body.verdict, Verdict::Unproven);
    assert!(body.unproven.is_empty());
    let listed: Vec<&str> = body
        .uncovered
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|p| p.as_str())
        .collect();
    assert_eq!(listed, ["native/src/lib.rs", "tools/gen.py"]);
    assert_matches_schema("receipt", &uncovered.receipt);
    assert_eq!(
        uncovered.diagnostics.uncovered.as_ref().unwrap().items,
        ["native/src/lib.rs", "tools/gen.py"]
    );
    let input = ReportInput {
        receipt_id: &uncovered.id,
        receipt: &uncovered.receipt,
        diagnostics: Some(&uncovered.diagnostics),
        sync: None,
        took: None,
        budget: None,
        notices: None,
        timing: None,
    };
    let text = agent_text(&input);
    assert!(
        text.starts_with("NOT DONE: UNPROVEN.") && text.contains("no check covers 2 source files"),
        "{text}"
    );
    assert!(
        text.contains("changed files not covered by any check")
            && text.contains("native/src/lib.rs"),
        "{text}"
    );
    assert_eq!(
        aver::run::report::last_line(&input, false, &[]),
        "NOT DONE: UNPROVEN: no check covers the changed files listed above; test them yourself and report",
        "the closing line names the uncovered files, not a criterion"
    );
    assert_eq!(
        text.lines().last(),
        Some(aver::run::report::last_line(&input, false, &[]).as_str()),
        "every report says its verdict first and last: {text}"
    );
    assert_eq!(exit_code(&input), 1);

    // The base's aver.toml says what its check covers; the head's cannot.
    let covers_all = unit_toml("covers = [\"**\"]\n");
    let wide_base = w.snapshot(&[
        ("aver.toml", &covers_all),
        ("tests.json", tests),
        ("src/a.js", "1"),
    ]);
    let wide = w.create_task(w.task_body(&w.admin, wide_base, &[]));
    let head = w.snapshot(&[
        ("aver.toml", &covers_all),
        ("tests.json", tests),
        ("src/a.js", "1"),
        ("native/src/lib.rs", "fn main() {}"),
    ]);
    let receipt = w.verify(&w.admin, wide, head, wide_base).await.receipt;
    assert_eq!(receipt.body.verdict, Verdict::Pass);
    let narrowed = verify(vec![
        ("aver.toml", covers_all.clone()),
        ("native/src/lib.rs", "fn main() {}".into()),
    ])
    .await;
    assert_eq!(
        narrowed.receipt.body.verdict,
        Verdict::Unproven,
        "a head that widens covers is judged by the base's"
    );
}

/// Affected checks (§1.9.3): a check the change does not affect runs on
/// neither side and the receipt lists it in `skipped`; a change no check is
/// affected by verifies without a sandbox; `all` runs everything; a change
/// to `aver.toml` affects every check; nothing changed runs everything, so
/// the report is about the original code.
#[tokio::test]
async fn checks_the_change_does_not_affect_run_on_neither_side() {
    use aver::run::report::{agent_text, ReportInput};
    let sandbox = std::sync::Arc::new(scripted::Scripted::default());
    let w = World::with_sandbox(sandbox.clone());
    let toml = unit_toml(
        "covers = [\"src/**\", \"tests.json\"]\n\n[[check]]\nid = \"lint\"\nrun = [\"exit\", \"0\"]\ncovers = [\"lint/**\"]\n",
    );
    let tests = r#"{"keeps": "pass"}"#;
    let base_files = vec![
        ("aver.toml", toml.clone()),
        ("tests.json", tests.to_owned()),
        ("src/a.js", "1".to_owned()),
        ("lint/rules.json", "{}".to_owned()),
        ("README.md", "# calc\n".to_owned()),
    ];
    let snap = |changes: &[(&'static str, &str)]| {
        let mut files = base_files.clone();
        for (path, text) in changes {
            match files.iter_mut().find(|(p, _)| p == path) {
                Some(f) => f.1 = text.to_string(),
                None => files.push((path, text.to_string())),
            }
        }
        let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
        w.snapshot(&refs)
    };
    let base = snap(&[]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let skipped = |r: &Receipt| -> Vec<String> {
        r.body
            .skipped
            .iter()
            .flat_map(|s| s.iter().map(|s| s.check.to_string()))
            .collect()
    };

    let before = sandbox.runs();
    let docs = w
        .verify(
            &w.admin,
            task,
            snap(&[("README.md", "# calc, documented\n")]),
            base,
        )
        .await;
    let body = &docs.receipt.body;
    assert_eq!(body.verdict, Verdict::Pass);
    assert!(body.checks.is_empty(), "{:?}", body.checks);
    assert_eq!(skipped(&docs.receipt), ["lint", "unit"]);
    assert_eq!(
        body.skipped.as_ref().unwrap().iter().next().unwrap().reason,
        SkipReason::NotAffected
    );
    assert_eq!(sandbox.runs(), before, "no sandbox ran, on either side");
    assert_matches_schema("receipt", &docs.receipt);
    let text = agent_text(&ReportInput {
        receipt_id: &docs.id,
        receipt: &docs.receipt,
        diagnostics: Some(&docs.diagnostics),
        sync: None,
        took: None,
        budget: None,
        notices: None,
        timing: None,
    });
    assert!(
        text.starts_with("DONE: PASS. No check is affected by this change, so none ran."),
        "{text}"
    );
    assert!(text.contains("`aver verify --all`"), "{text}");
    assert!(
        !text.contains("Nothing shows the change works yet"),
        "{text}"
    );
    assert_eq!(text.lines().last(), Some("DONE: PASS"), "{text}");

    let code = w
        .verify(&w.admin, task, snap(&[("src/a.js", "2")]), base)
        .await;
    assert_eq!(skipped(&code.receipt), ["lint"]);
    assert!(entry(&code.receipt, "unit::keeps").base == BaseStatus::Pass);
    // A change to code asks for a test that shows it; one to documentation
    // or data alone does not, even when a check ran.
    let synced = |paths: &[&str]| aver::disk::SyncReport {
        version: 1,
        added: 0,
        modified: paths.len() as u32,
        deleted: 0,
        updated: 1,
        changed: paths.iter().map(|p| (*p).to_owned()).collect(),
        skipped: Vec::new(),
        stats: Default::default(),
    };
    let render = |v: &aver::verify::Verified, sync: &aver::disk::SyncReport| {
        agent_text(&ReportInput {
            receipt_id: &v.id,
            receipt: &v.receipt,
            diagnostics: Some(&v.diagnostics),
            sync: Some(sync),
            took: None,
            budget: None,
            notices: None,
            timing: None,
        })
    };
    let text = render(&code, &synced(&["src/a.js"]));
    assert!(
        text.contains("Nothing shows the change works yet"),
        "{text}"
    );
    assert!(
        text.contains("Not affected by your change, so not run: lint"),
        "{text}"
    );
    let data = w
        .verify(
            &w.admin,
            task,
            snap(&[("tests.json", r#"{"keeps": "pass", "more": "pass"}"#)]),
            base,
        )
        .await;
    let text = render(&data, &synced(&["tests.json"]));
    assert!(
        !text.contains("Nothing shows the change works yet"),
        "{text}"
    );

    let config = w
        .verify(
            &w.admin,
            task,
            snap(&[("aver.toml", &format!("{toml}# edited\n"))]),
            base,
        )
        .await;
    assert!(
        config.receipt.body.skipped.is_none(),
        "aver.toml affects every check"
    );

    let unchanged = w.verify(&w.admin, task, base, base).await;
    assert!(
        unchanged.receipt.body.skipped.is_none(),
        "nothing changed runs everything"
    );

    let all = w
        .verifier
        .verify(
            w.admin.id(),
            Request {
                task,
                head: Head::Snapshot {
                    head: snap(&[("README.md", "# again\n")]),
                    base: Some(base),
                },
                attempt: None,
                fresh: false,
                all: true,
                progress: None,
                ci: None,
                adhoc: None,
                reason: None,
            },
        )
        .await
        .unwrap();
    assert!(all.receipt.body.skipped.is_none());
    assert!(all.receipt.body.reproduce.ends_with(" --all"));
    assert!(entry(&all.receipt, "lint").head == HeadStatus::Pass);
}

/// A type check with the `tsc` report format judges each type error on its
/// own: errors the base already has are PREEXISTING and do not block, one
/// the head adds is NEW, one it fixes passes (not "missing from head"), and
/// an error whose line moved keeps its entry. A type check alone runs no
/// code, so a clean head is `UNPROVEN` (`no_runtime_tests`), not `PASS`.
#[tokio::test]
async fn type_errors_are_judged_one_by_one_so_a_red_base_does_not_block() {
    let w = World::new();
    let toml = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"types\"\nrun = [\"copy-report\", \"tsc.txt\", \"tsc-output.txt\"]\nreport = {{ format = \"tsc\", path = \"/aver/out/tsc.txt\" }}\n"
    );
    let old = "src/a.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.\n";
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tsc-output.txt", old),
        ("src/a.ts", "1"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let verify = |output: &'static str| {
        let w = &w;
        let toml = toml.clone();
        async move {
            let head = w.snapshot(&[
                ("aver.toml", &toml),
                ("tsc-output.txt", output),
                ("src/a.ts", "2"),
            ]);
            w.verify(&w.admin, task, head, base).await.receipt
        }
    };
    let old_id = "types::src/a.ts > TS2322 Type 'string' is not assignable to type 'number'.";

    // The old error moved two lines down: still the same entry.
    let moved =
        verify("src/a.ts(5,7): error TS2322: Type 'string' is not assignable to type 'number'.\n")
            .await;
    assert_eq!(
        moved.body.verdict,
        Verdict::Unproven,
        "{:?}",
        moved.body.checks
    );
    assert_eq!(entry(&moved, old_id).cause, Some(Cause::Preexisting));
    assert_matches_schema("receipt", &moved);

    let added = verify(
        "src/a.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.\nsrc/b.ts(1,1): error TS2304: Cannot find name 'x'.\n",
    )
    .await;
    assert_eq!(added.body.verdict, Verdict::FailOwn);
    assert_eq!(
        entry(&added, "types::src/b.ts > TS2304 Cannot find name 'x'.").cause,
        Some(Cause::New)
    );
    assert_eq!(entry(&added, old_id).cause, Some(Cause::Preexisting));

    let fixed = verify("").await;
    assert_eq!(
        fixed.body.verdict,
        Verdict::Unproven,
        "{:?}",
        fixed.body.checks
    );
    let e = entry(&fixed, old_id);
    assert_eq!(
        (e.base, e.head, e.cause),
        (BaseStatus::Fail, HeadStatus::Pass, None)
    );
    assert!(
        !fixed
            .body
            .checks
            .iter()
            .any(|c| c.id.as_str().ends_with("(exit)")),
        "a clean type check is not an empty report"
    );
}

/// Defect: a base that fails as a whole the same way on every run (a
/// toolchain that refuses, a build that does not compile) runs three times
/// before anything is judged. A build error settles it after one run, an
/// identical failure (exit code and output) after two; per-test failures
/// keep three runs (`every_failure_is_attributed_per_test…`).
#[tokio::test]
async fn a_base_failing_the_same_way_as_a_whole_is_not_rerun_to_three() {
    let w = World::new();
    let toml = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"build\"\nrun = [\"say\", \"build.log\", \"101\"]\n\n[[check]]\nid = \"cmd\"\nrun = [\"exit\", \"3\"]\n"
    );
    let rustc = "   Compiling calc v0.1.0\nerror[E0425]: cannot find function `mul` in crate `calc`\n --> tests/calc.rs:9:5\n";
    let base = w.snapshot(&[("aver.toml", &toml), ("build.log", rustc)]);
    let head = w.snapshot(&[("aver.toml", &toml), ("build.log", rustc), ("x", "1")]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let build = entry(&receipt, "build");
    assert_eq!(
        (build.cause, build.runs.base.len()),
        (Some(Cause::Preexisting), 1),
        "a base that does not compile runs once"
    );
    let cmd = entry(&receipt, "cmd");
    assert_eq!(
        (cmd.cause, cmd.runs.base.len()),
        (Some(Cause::Preexisting), 2),
        "a base failing twice with the same exit code and output stops"
    );
}

/// B8 (`marshmallow`): a test in a file the head left alone whose
/// parameter is built from the clock has a new id on every run. A head
/// that only edits a docstring must not read as having deleted it: the
/// base, run once more, shows its own id changing, so the ids pair.
#[tokio::test]
async fn a_test_whose_parameter_changes_every_run_is_paired_not_new() {
    let w = World::new();
    let toml = unit_toml("covers = [\"src/**\"]\n");
    let tests =
        r#"{"tests/test_t.py > test_at[{nonce}]": "pass", "tests/test_t.py > keeps": "pass"}"#;
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("tests/test_t.py", "def test_at(): pass\n"),
        ("src/calc.py", "def add(a, b):\n    return a + b\n"),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("tests/test_t.py", "def test_at(): pass\n"),
        (
            "src/calc.py",
            "def add(a, b):\n    \"\"\"Adds.\"\"\"\n    return a + b\n",
        ),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let new: Vec<&str> = receipt
        .body
        .checks
        .iter()
        .filter(|c| c.cause == Some(Cause::New))
        .map(|c| c.id.as_str())
        .collect();
    assert!(new.is_empty(), "{new:?}");
    let at: Vec<_> = receipt
        .body
        .checks
        .iter()
        .filter(|c| c.id.as_str().contains("test_at["))
        .collect();
    assert_eq!(at.len(), 1, "{:?}", summary(&receipt));
    assert_eq!(
        (at[0].base, at[0].head, at[0].cause),
        (BaseStatus::Pass, HeadStatus::Pass, None)
    );
    assert_eq!(receipt.body.verdict, Verdict::Pass);

    // A parameter the base reports the same way every run is the test's
    // own: the head's other parameter is no rename of it.
    let stable = |p: &str| {
        format!(
            r#"{{"tests/test_t.py > test_at[{p}]": "pass", "tests/test_t.py > keeps": "pass"}}"#
        )
    };
    let toml = unit_toml("");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", &stable("a")),
        ("tests/test_t.py", "def test_at(): pass\n"),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", &stable("b")),
        ("tests/test_t.py", "def test_at(): pass\n"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let gone = entry(&receipt, "unit::tests/test_t.py > test_at[a]");
    assert_eq!(gone.cause, Some(Cause::New));
    assert_eq!(
        gone.detail.as_ref().map(|d| d.as_str()),
        Some("missing from head report")
    );
}

/// The integration-trap shape (round 13, P3): a slow integration suite whose
/// command names its own test glob, beside the unit suite the agent adds
/// tests to. The transplant adds no file the integration check runs, so it
/// shares the base's runs there; the unit check's transplant runs once,
/// failing or not: a proving test fails there by design (§4.5).
#[tokio::test]
async fn a_transplant_runs_once_and_only_where_its_tests_run() {
    let sandbox = std::sync::Arc::new(scripted::Scripted::default());
    let w = World::with_sandbox(sandbox.clone());
    let toml = format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n\n[[check]]\nid = \"integration\"\nrun = [\"junit-dir\", \"int.xml\", \"integration\", \"integration/**/*.test.json\"]\nreport = {{ format = \"junit\", path = \"/aver/out/int.xml\" }}\n"
    );
    let base_files = [
        ("aver.toml", toml.as_str()),
        ("src/a.js", "1"),
        ("test/a.test.json", r#"{"adds": "pass"}"#),
        (
            "integration/mail.test.json",
            r#"{"routes replies": "pass"}"#,
        ),
    ];
    let base = w.snapshot(&base_files);
    let task = w.create_task(w.task_body(&w.admin, base, &["case"]));
    let with = |extra: &[(&'static str, &'static str)]| {
        let mut files: Vec<(&str, &str)> = base_files.to_vec();
        files.extend_from_slice(extra);
        w.snapshot(&files)
    };
    // (transplant runs, those not shared with the base side)
    let transplant_runs = |v: &aver::verify::Verified, check: &str| -> (usize, usize) {
        let of = |side| -> Vec<RunId> {
            v.runs
                .iter()
                .filter(|r| r.check.as_str() == check && r.side == side)
                .map(|r| r.run)
                .collect()
        };
        let base = of(aver::verify::Side::Base);
        let transplant = of(aver::verify::Side::Transplant);
        let own = transplant.iter().filter(|r| !base.contains(r)).count();
        (transplant.len(), own)
    };

    // A proving test fails on the transplant, as it should: no rerun.
    let head = with(&[
        ("src/new.js", "1"),
        (
            "test/case.test.json",
            r#"{"[crit:case] matches": "pass-if:src/new.js"}"#,
        ),
    ]);
    let v = w.verify(&w.admin, task, head, base).await;
    assert_eq!(v.receipt.body.verdict, Verdict::Pass);
    assert_eq!(
        entry(&v.receipt, "unit::[crit:case] matches").transplant,
        Some(Transplant::Fail)
    );
    assert_eq!(
        transplant_runs(&v, "unit"),
        (1, 1),
        "an expected failure on the transplant is not rerun"
    );
    assert_eq!(
        transplant_runs(&v, "integration").1,
        0,
        "the integration check shares the base's runs"
    );

    // A test that passes on the transplant too: one run, no reruns.
    let head = with(&[
        ("src/new.js", "2"),
        (
            "test/trivial.test.json",
            r#"{"[crit:case] trivially": "pass"}"#,
        ),
    ]);
    let before = sandbox.runs();
    let v = w.verify(&w.admin, task, head, base).await;
    assert_eq!(v.receipt.body.verdict, Verdict::Unproven);
    assert_eq!(
        transplant_runs(&v, "unit"),
        (1, 1),
        "a passing transplant run is not rerun"
    );
    assert_eq!(
        sandbox.runs() - before,
        v.runs.iter().filter(|r| !r.cache_hit).count(),
        "no sandbox started that the verify did not use"
    );
    assert_eq!(transplant_runs(&v, "integration").1, 0);
}

/// Verdict-integrity scenario B3: the head breaks the code and its build step rewrites
/// the compiled test file, so the tests that run are no longer the base's.
/// Both sides compiled the same frozen test sources, so the file's other
/// content is a `built::` entry, `CHANGED`, and the verdict `REVIEW`.
#[tokio::test]
async fn a_compiled_test_file_the_heads_build_rewrites_is_changed() {
    let w = World::new();
    let toml = unit_toml("setup = [[\"build\", \"dist/test/v4.test.js\", \"build.mjs\"]]\n");
    let tests = r#"{"v4 is a uuid": "pass"}"#;
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("build.mjs", "tsc\n"),
        ("src/v4.js", "uuid\n"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));

    let rewrites = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        (
            "build.mjs",
            "tsc && sed -i s/assert/skip/ dist/test/v4.test.js\n",
        ),
        ("src/v4.js", "broken\n"),
    ]);
    let receipt = w.verify(&w.admin, task, rewrites, base).await.receipt;
    let built = entry(&receipt, "built::dist/test/v4.test.js");
    assert_eq!(built.cause, Some(Cause::Changed));
    let said = built
        .detail
        .as_ref()
        .map(|d| d.as_str())
        .unwrap_or_default();
    assert!(said.contains("dist/test/v4.test.js"), "{said}");
    assert_eq!(receipt.body.verdict, Verdict::Review);

    let builds_the_same = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", tests),
        ("build.mjs", "tsc\n"),
        ("src/v4.js", "fixed\n"),
    ]);
    let receipt = w
        .verify(&w.admin, task, builds_the_same, base)
        .await
        .receipt;
    assert!(
        receipt
            .body
            .checks
            .iter()
            .all(|e| !e.id.as_str().starts_with("built::")),
        "{:?}",
        receipt.body.checks
    );
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

/// `built` names the built test file entries, so no check may take it.
#[tokio::test]
async fn a_check_named_built_is_refused() {
    let w = World::new();
    let toml = unit_toml("\n[[check]]\nid = \"built\"\nrun = [\"exit\", \"0\"]\n");
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", r#"{"a": "pass"}"#)]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        ("tests.json", r#"{"a": "pass"}"#),
        ("lib.js", "x"),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let err = w
        .verifier
        .verify(
            w.admin.id(),
            Request {
                task,
                head: Head::Snapshot {
                    head,
                    base: Some(base),
                },
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
        .unwrap_err();
    assert!(
        matches!(&err, VerifyError::Invalid(m) if m.contains("\"built\"")),
        "{err}"
    );
}

fn report_of(v: &aver::verify::Verified) -> String {
    aver::run::report::agent_text(&aver::run::report::ReportInput {
        receipt_id: &v.id,
        receipt: &v.receipt,
        diagnostics: Some(&v.diagnostics),
        sync: None,
        took: None,
        budget: None,
        notices: None,
        timing: None,
    })
}

/// The report leads with the verdict, then each side's test counts, then
/// the one pushback; an assertion two tests fail
/// the same way is printed once.
#[tokio::test]
async fn the_report_leads_with_counts_and_one_pushback() {
    let w = World::new();
    let toml = unit_toml("");
    let base = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"a": "pass", "b": "pass", "old": "fail", "keep": "pass"}"#,
        ),
    ]);
    let head = w.snapshot(&[
        ("aver.toml", &toml),
        (
            "tests.json",
            r#"{"a": "say-if:none:Error: boom", "b": "say-if:none:Error: boom", "old": "fail", "keep": "pass"}"#,
        ),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let v = w.verify(&w.admin, task, head, base).await;
    assert_eq!(v.receipt.body.verdict, Verdict::FailOwn);
    let text = report_of(&v);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("NOT DONE: FAIL_OWN."), "{text}");
    assert_eq!(
        lines[1],
        "Base: 3 passed, 1 failed | Yours: 1 passed, 3 failed"
    );
    assert!(
        lines[2].starts_with("Your change breaks 2 tests that pass on the original code: `"),
        "{text}"
    );
    assert!(lines[3].starts_with("Next:"), "{text}");
    assert_eq!(text.matches("Error: boom").count(), 1, "{text}");
    assert_eq!(
        text.matches("(the same failure as above)").count(),
        1,
        "{text}"
    );
}

/// A check that fails as a whole shows the end of its output under the
/// headline, fenced; an infrastructure failure is not called a sandbox's.
#[tokio::test]
async fn a_check_failing_as_a_whole_shows_its_output_tail() {
    let w = World::new();
    let toml = unit_toml("\n[[check]]\nid = \"slow\"\nrun = [\"timeout\"]\n");
    let base = w.snapshot(&[("aver.toml", &toml), ("tests.json", r#"{"t": "pass"}"#)]);
    let head = w.snapshot(&[("aver.toml", &toml)]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let v = w.verify(&w.admin, task, head, base).await;
    let text = report_of(&v);
    let tail = text
        .split("The last lines of output of each check that failed as a whole")
        .nth(1)
        .unwrap_or_else(|| panic!("{text}"));
    assert!(tail.contains("```\nunit\n"), "{text}");
    assert!(tail.contains("cannot find tests.json"), "{text}");
    assert!(!text.contains("a sandbox timed out"), "{text}");
    assert!(text.contains("a run timed out"), "{text}");
    let tail_at = text.find("The last lines of output").unwrap();
    let next_at = text.find("\nNext:").unwrap();
    assert!(
        next_at < tail_at,
        "the tail follows the first lines: {text}"
    );
}
