//! Executor behavior with a scripted in-process runner: caching, single
//! flight, the concurrency bound, infra handling and report parsing.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aver::exec::output::{Exit, SandboxOutput};
use aver::exec::seam::{BoxFuture, Sandbox, SandboxError, SandboxRequest};
use aver::exec::{task_check, ExecConfig, Executor};
use aver::store::Store;
use aver_core::*;
use common::*;
use parking_lot::Mutex;

type Behavior = Box<dyn Fn(&SandboxRequest) -> Result<SandboxOutput, SandboxError> + Send + Sync>;

struct Fake {
    executor: &'static str,
    delay: Duration,
    behavior: Behavior,
    requests: Mutex<Vec<SandboxRequest>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl Fake {
    fn new(
        behavior: impl Fn(&SandboxRequest) -> Result<SandboxOutput, SandboxError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            executor: "fake-v0",
            delay: Duration::ZERO,
            behavior: Box::new(behavior),
            requests: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }

    fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    fn runs(&self) -> usize {
        self.requests.lock().len()
    }

    fn requests(&self) -> Vec<SandboxRequest> {
        self.requests.lock().clone()
    }
}

impl Sandbox for Fake {
    fn executor(&self) -> &str {
        self.executor
    }

    fn run(&self, request: SandboxRequest) -> BoxFuture<'_, Result<SandboxOutput, SandboxError>> {
        Box::pin(async move {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            let result = (self.behavior)(&request);
            self.requests.lock().push(request);
            self.active.fetch_sub(1, Ordering::SeqCst);
            result
        })
    }
}

fn exited(code: i32, output: &str) -> Result<SandboxOutput, SandboxError> {
    Ok(SandboxOutput {
        exit: Exit::Exited(code),
        output: output.as_bytes().to_vec(),
        files: BTreeMap::new(),
        duration: Duration::from_millis(42),
        built: None,
    })
}

fn with_report(code: i32, xml: &str) -> Result<SandboxOutput, SandboxError> {
    let mut out = exited(code, "ran")?;
    out.files.insert("unit.xml".into(), xml.as_bytes().to_vec());
    Ok(out)
}

const REPORT: &str = r#"<testsuites>
  <testsuite name="math"><testcase name="adds"/></testsuite>
  <testcase name="[crit:div-zero] divide throws"><failure message="not implemented"/></testcase>
</testsuites>"#;

fn env_toml(lockfile: bool) -> String {
    format!(
        r#"
[env]
image = "{NODE_IMAGE}"
{}
[limits]
cpus = 1
memory_mb = 256
timeout_s = 30
[[check]]
id = "unit"
run = ["node", "--test"]
report = {{ format = "junit", path = "/aver/out/unit.xml" }}
[[check]]
id = "lint"
run = ["node", "lint.js"]
cwd = "src"
timeout_ms = 1500
"#,
        if lockfile {
            r#"lockfile = "package-lock.json""#
        } else {
            ""
        }
    )
}

struct Setup {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    snapshot: SnapshotId,
    env: Environment,
}

impl Setup {
    fn new(lockfile: bool) -> Self {
        let (dir, store) = open_store();
        let lock = "{\"lockfileVersion\":3}\n";
        let root = put_files(
            &store,
            &[
                ("package.json", "{}\n"),
                ("package-lock.json", lock),
                ("src/math.js", "exports.add = (a, b) => a + b;\n"),
                ("test/math.test.js", "require('node:test');\n"),
            ],
        );
        let env = parse_aver_toml(&env_toml(lockfile), Some(BlobId::of_bytes(lock.as_bytes())));
        let snapshot = put_snapshot(&store, root, &env);
        Self {
            _dir: dir,
            store,
            snapshot,
            env,
        }
    }

    fn executor(&self, fake: &Arc<Fake>) -> Executor {
        self.executor_with(fake, ExecConfig::default())
    }

    fn executor_with(&self, fake: &Arc<Fake>, config: ExecConfig) -> Executor {
        Executor::new(self.store.clone(), fake.clone(), config)
    }

    fn check(&self, id: &str) -> CheckDef {
        self.env.checks.get(id).unwrap().clone()
    }
}

#[tokio::test]
async fn second_run_is_a_cache_hit_that_starts_no_sandbox() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|_| with_report(1, REPORT)));
    let exec = s.executor(&fake);
    let unit = s.check("unit");

    let first = exec
        .run(&s.snapshot, &s.env, &unit, RunIndex::FIRST)
        .await
        .unwrap();
    assert!(!first.cache_hit);
    assert_eq!(first.run.result, RunResult::Fail { exit_code: 1 });
    assert_eq!(exec.sandbox_starts(), 1);

    let second = exec
        .run(&s.snapshot, &s.env, &unit, RunIndex::FIRST)
        .await
        .unwrap();
    assert!(second.cache_hit);
    assert_eq!(second.id, first.id);
    assert_eq!(second.run, first.run);
    assert_eq!(exec.sandbox_starts(), 1);

    let restarted = s.executor(&fake);
    let cached = restarted
        .cached(&s.snapshot, &s.env, &unit, RunIndex::FIRST)
        .unwrap()
        .unwrap();
    assert_eq!(cached.id, first.id);
    let third = restarted
        .run(&s.snapshot, &s.env, &unit, RunIndex::FIRST)
        .await
        .unwrap();
    assert!(third.cache_hit);
    assert_eq!(restarted.sandbox_starts(), 0);
    assert_eq!(fake.runs(), 1);
}

#[tokio::test]
async fn the_cache_key_separates_index_env_check_and_executor() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|_| exited(0, "ok")));
    let exec = s.executor(&fake);
    let unit = s.check("unit");

    let a = exec
        .run(&s.snapshot, &s.env, &unit, RunIndex::FIRST)
        .await
        .unwrap();
    let b = exec
        .run(&s.snapshot, &s.env, &unit, RunIndex::SECOND)
        .await
        .unwrap();
    assert_ne!(a.id, b.id);
    exec.run(&s.snapshot, &s.env, &s.check("lint"), RunIndex::FIRST)
        .await
        .unwrap();

    let mut other_env = s.env.clone();
    other_env.vars.insert(p("FEATURE_FLAG"), "on".into());
    let c = exec
        .run(&s.snapshot, &other_env, &unit, RunIndex::FIRST)
        .await
        .unwrap();
    assert!(!c.cache_hit);
    assert_eq!(c.run.env, other_env.id());
    assert!(
        s.store.has(&other_env.id()).unwrap(),
        "the env actually used is stored"
    );
    assert_eq!(exec.sandbox_starts(), 4);

    let mut stronger = Fake::new(|_| exited(0, "ok"));
    stronger.executor = "fake-strong-v0";
    let stronger = Arc::new(stronger);
    let d = s
        .executor(&stronger)
        .run(&s.snapshot, &s.env, &unit, RunIndex::FIRST)
        .await
        .unwrap();
    assert!(
        !d.cache_hit,
        "runs from another executor profile are not served"
    );
    assert_eq!(d.run.executor, "fake-strong-v0");
}

#[tokio::test]
async fn task_checks_with_the_same_id_but_different_definitions_do_not_share_results() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|r| exited(0, &r.argv.join(" "))));
    let exec = s.executor(&fake);
    let def = |argv: &str| TaskCheckDef {
        id: p("crit"),
        argv: argv.split(' ').map(String::from).collect(),
        cwd: None,
        timeout_ms: None,
        report: None,
        proves: Set::new(vec![p("div-zero")]).unwrap(),
    };
    let first = task_check(&def("node a.js"));
    let second = task_check(&def("node b.js"));

    let a = exec
        .run(&s.snapshot, &s.env, &first, RunIndex::FIRST)
        .await
        .unwrap();
    let b = exec
        .run(&s.snapshot, &s.env, &second, RunIndex::FIRST)
        .await
        .unwrap();
    assert!(!b.cache_hit);
    assert_eq!(output_text(&s.store, &b.run), "node b.js");
    let again = exec
        .run(&s.snapshot, &s.env, &first, RunIndex::FIRST)
        .await
        .unwrap();
    assert!(again.cache_hit);
    assert_eq!(again.id, a.id);
}

#[tokio::test]
async fn concurrent_identical_runs_share_one_sandbox() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|_| exited(0, "ok")).delayed(Duration::from_millis(300)));
    let exec = s.executor(&fake);
    let unit = s.check("unit");
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (exec, snapshot, env, unit) = (exec.clone(), s.snapshot, s.env.clone(), unit.clone());
        tasks.spawn(async move {
            exec.run(&snapshot, &env, &unit, RunIndex::FIRST)
                .await
                .unwrap()
        });
    }
    let mut ids = Vec::new();
    while let Some(run) = tasks.join_next().await {
        ids.push(run.unwrap().id);
    }
    ids.dedup();
    assert_eq!(ids.len(), 1);
    assert_eq!(exec.sandbox_starts(), 1);
    assert_eq!(fake.runs(), 1);
}

/// The slots go round-robin between requesters (§2.4): with one slot,
/// task B's single run starts right after the run of task A that holds it
/// and the next one, not after A's six. A run whose timeout is shorter
/// than its wait still passes: the timeout starts with its slot.
#[tokio::test]
async fn slots_are_shared_fairly_between_tasks() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|_| exited(0, "ok")).delayed(Duration::from_millis(30)));
    let exec = s.executor_with(
        &fake,
        ExecConfig {
            max_sandboxes: 1,
            ..ExecConfig::default()
        },
    );
    let mut tasks = tokio::task::JoinSet::new();
    let a = exec.for_requester("task-a");
    for check in ["unit", "lint"] {
        for index in [RunIndex::FIRST, RunIndex::SECOND, RunIndex::THIRD] {
            let (exec, snapshot, env, check) =
                (a.clone(), s.snapshot, s.env.clone(), s.check(check));
            tasks.spawn(async move { exec.run(&snapshot, &env, &check, index).await.unwrap() });
        }
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    let mut other = s.check("unit");
    other.id = "other".parse().unwrap();
    other.timeout_ms = Some(1);
    let b = exec.for_requester("task-b");
    let (snapshot, env) = (s.snapshot, s.env.clone());
    let single = tokio::spawn(async move { b.run(&snapshot, &env, &other, RunIndex::FIRST).await });
    while tasks.join_next().await.is_some() {}
    let run = single.await.unwrap().unwrap();
    assert!(
        matches!(run.run.result, RunResult::Pass { .. }),
        "a run that waited is not infra: {:?}",
        run.run.result
    );
    let order: Vec<String> = fake.requests().iter().map(|r| r.label.clone()).collect();
    let at = order.iter().position(|l| l.contains(":other:")).unwrap();
    assert!(at <= 2, "B's run started {}th of {order:?}", at + 1);
}

#[tokio::test]
async fn concurrency_is_bounded_by_max_sandboxes() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|_| exited(0, "ok")).delayed(Duration::from_millis(100)));
    let exec = s.executor_with(
        &fake,
        ExecConfig {
            max_sandboxes: 2,
            ..ExecConfig::default()
        },
    );
    let mut tasks = tokio::task::JoinSet::new();
    for check in ["unit", "lint"] {
        for index in [RunIndex::FIRST, RunIndex::SECOND, RunIndex::THIRD] {
            let (exec, snapshot, env, check) =
                (exec.clone(), s.snapshot, s.env.clone(), s.check(check));
            tasks.spawn(async move { exec.run(&snapshot, &env, &check, index).await.unwrap() });
        }
    }
    while tasks.join_next().await.is_some() {}
    assert_eq!(fake.runs(), 6);
    assert_eq!(fake.max_active.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn infra_runs_are_stored_but_never_served_from_cache() {
    let cases: Vec<(Behavior, RunResult, &str)> = vec![
        (
            Box::new(|_| Err(SandboxError::Malformed("no result".into()))),
            RunResult::Sandbox,
            "[aver] sandbox error: malformed sandbox result: no result",
        ),
        (
            Box::new(|_| {
                let mut out = exited(0, "allocating")?;
                out.exit = Exit::Oom(137);
                Ok(out)
            }),
            RunResult::Oom { exit_code: 137 },
            "allocating\n[aver] killed: out of memory (limit 256 MiB)",
        ),
        (
            Box::new(|_| {
                let mut out = exited(0, "")?;
                out.exit = Exit::Timeout(137);
                Ok(out)
            }),
            RunResult::Timeout { exit_code: 137 },
            "[aver] killed: exceeded the 1500 ms timeout",
        ),
    ];
    for (behavior, expected, note) in cases {
        let s = Setup::new(false);
        let fake = Arc::new(Fake {
            behavior,
            ..Fake::new(|_| unreachable!())
        });
        let exec = s.executor(&fake);
        let lint = s.check("lint");
        let first = exec
            .run(&s.snapshot, &s.env, &lint, RunIndex::FIRST)
            .await
            .unwrap();
        assert_eq!(first.run.result, expected);
        assert_eq!(first.run.result.outcome(), Outcome::Infra);
        assert!(
            output_text(&s.store, &first.run).starts_with(note),
            "{}",
            output_text(&s.store, &first.run)
        );
        assert_eq!(
            s.store.get::<CheckRun>(&first.id).unwrap(),
            first.run,
            "stored for audit"
        );
        assert_matches_schema("run", &first.run);
        assert!(exec
            .cached(&s.snapshot, &s.env, &lint, RunIndex::FIRST)
            .unwrap()
            .is_none());

        let second = exec
            .run(&s.snapshot, &s.env, &lint, RunIndex::FIRST)
            .await
            .unwrap();
        assert!(!second.cache_hit);
        assert_eq!(exec.sandbox_starts(), 2, "infra is retried");
    }
}

/// A run that lost its report because a write found the disk full ends as
/// infra (cause `ENV`), never as the change's failure; with its report, a
/// test merely named after the error still fails as itself.
#[tokio::test]
async fn a_report_lost_to_a_full_disk_is_infra_crit_k2_disk_full() {
    let s = Setup::new(false);
    let full = Arc::new(Fake::new(|_| {
        exited(1, "ok adds\nprintf: write error: No space left on device\n")
    }));
    let run = s
        .executor(&full)
        .run(&s.snapshot, &s.env, &s.check("unit"), RunIndex::FIRST)
        .await
        .unwrap()
        .run;
    assert_eq!(run.result, RunResult::Sandbox);
    assert_eq!(run.result.outcome(), Outcome::Infra);
    let text = output_text(&s.store, &run);
    assert!(text.contains("[aver] the disk is full"), "{text}");

    let full = Arc::new(Fake::new(|_| {
        Err(SandboxError::DiskFull("no space left on device".into()))
    }));
    let run = s
        .executor(&full)
        .run(&s.snapshot, &s.env, &s.check("unit"), RunIndex::SECOND)
        .await
        .unwrap()
        .run;
    assert_eq!(run.result.outcome(), Outcome::Infra);

    let named = Arc::new(Fake::new(|_| {
        let mut out = with_report(1, REPORT)?;
        out.output = b"FAIL handles ENOSPC\n".to_vec();
        Ok(out)
    }));
    let run = s
        .executor(&named)
        .run(&s.snapshot, &s.env, &s.check("unit"), RunIndex::THIRD)
        .await
        .unwrap()
        .run;
    assert_eq!(run.result, RunResult::Fail { exit_code: 1 });
}

#[tokio::test]
async fn junit_reports_become_test_results() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|r| {
        if r.argv[1] == "--test" {
            with_report(1, REPORT)
        } else {
            let mut out = exited(0, "lint ok")?;
            out.files
                .insert("unit.xml".into(), b"<testsuites/>".to_vec());
            Ok(out)
        }
    }));
    let exec = s.executor(&fake);

    let unit = exec
        .run(&s.snapshot, &s.env, &s.check("unit"), RunIndex::FIRST)
        .await
        .unwrap()
        .run;
    assert_eq!(
        test_statuses(&unit),
        vec![
            ("[crit:div-zero] divide throws".to_owned(), TestStatus::Fail),
            ("math > adds".to_owned(), TestStatus::Pass),
        ]
    );
    assert_eq!(
        s.store.get_raw(&unit.report.unwrap()).unwrap(),
        REPORT.as_bytes()
    );
    assert_eq!(unit.duration_ms, 42);

    let lint = exec
        .run(&s.snapshot, &s.env, &s.check("lint"), RunIndex::FIRST)
        .await
        .unwrap()
        .run;
    assert!(lint.report.is_none(), "lint declares no report");
    assert!(lint.tests.is_empty());

    let broken = Arc::new(Fake::new(|_| with_report(1, "<html>")));
    let run = s
        .executor(&broken)
        .run(&s.snapshot, &s.env, &s.check("unit"), RunIndex::SECOND)
        .await
        .unwrap()
        .run;
    assert!(run.report.is_none());
    assert!(run.tests.is_empty());
    assert!(output_text(&s.store, &run)
        .contains("[aver] report /aver/out/unit.xml: malformed JUnit XML"));

    let missing = Arc::new(Fake::new(|_| exited(2, "crashed before reporting")));
    let run = s
        .executor(&missing)
        .run(&s.snapshot, &s.env, &s.check("unit"), RunIndex::THIRD)
        .await
        .unwrap()
        .run;
    assert_eq!(run.result, RunResult::Fail { exit_code: 2 });
    assert!(run.report.is_none());
    assert_eq!(output_text(&s.store, &run), "crashed before reporting");
}

#[tokio::test]
async fn requests_carry_the_check_and_the_snapshot_files() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|_| exited(0, "")));
    let exec = s.executor(&fake);
    exec.run(&s.snapshot, &s.env, &s.check("lint"), RunIndex::THIRD)
        .await
        .unwrap();
    exec.run(&s.snapshot, &s.env, &s.check("unit"), RunIndex::FIRST)
        .await
        .unwrap();
    let requests = fake.requests();
    let (lint, unit) = (&requests[0], &requests[1]);

    assert_eq!(lint.argv, vec!["node", "lint.js"]);
    assert_eq!(lint.cwd, Some(p("src")));
    assert_eq!(lint.limits.timeout, Duration::from_millis(1500));
    assert_eq!(unit.limits.timeout, Duration::from_secs(30));
    assert_eq!(lint.limits.memory_mb, 256);
    assert_eq!(lint.limits.cpu_millis, 1000);
    assert!(lint.label.ends_with(":lint:3") && unit.label.ends_with(":unit:1"));
    assert_eq!(
        lint.env, unit.env,
        "every run gets the same environment, whatever its index"
    );
    assert!(!lint.env.contains_key("AVER_RUN_ATTEMPT"));
    assert!(lint.label.starts_with(&s.snapshot.to_string()));
    let paths: Vec<&String> = lint.input.keys().collect();
    assert_eq!(
        paths,
        vec![
            "package-lock.json",
            "package.json",
            "src/math.js",
            "test/math.test.js"
        ]
    );
}

/// `aver.toml` is repository content, so its limits cannot exceed the
/// executor's ceiling: a fork bomb or a week-long sleep must stay bounded.
#[tokio::test]
async fn repository_limits_are_clamped_to_the_executor_ceiling() {
    let (_dir, store) = open_store();
    let toml = format!(
        "[env]\nimage = \"{NODE_IMAGE}\"\n[limits]\ncpus = 1024\nmemory_mb = 4000000\npids = 4294967295\ntimeout_s = 4294967\noutput_bytes = 4294967295\n[[check]]\nid = \"unit\"\nrun = [\"true\"]\ntimeout_ms = 4294967295\n"
    );
    let env = parse_aver_toml(&toml, None);
    let snapshot = put_snapshot(&store, put_files(&store, &[("a", "b")]), &env);
    let fake = Arc::new(Fake::new(|_| exited(0, "ok")));
    let exec = Executor::new(store.clone(), fake.clone(), ExecConfig::default());
    let unit = env.checks.get("unit").unwrap().clone();
    exec.run(&snapshot, &env, &unit, RunIndex::FIRST)
        .await
        .unwrap();
    assert_eq!(fake.requests()[0].limits, aver::exec::MAX_LIMITS);
}

#[tokio::test]
async fn run_env_checks_runs_every_check_of_the_snapshot_env() {
    let s = Setup::new(false);
    let fake = Arc::new(Fake::new(|r| exited(i32::from(r.argv[1] == "--test"), "")));
    let exec = s.executor(&fake);
    let runs = exec
        .run_env_checks(&s.snapshot, RunIndex::FIRST)
        .await
        .unwrap();
    let summary: Vec<(&str, Outcome)> = runs
        .iter()
        .map(|r| (r.run.check.as_str(), r.run.result.outcome()))
        .collect();
    assert_eq!(
        summary,
        vec![("lint", Outcome::Pass), ("unit", Outcome::Fail)]
    );
    for r in &runs {
        assert_eq!(r.run.snapshot, s.snapshot);
        assert_eq!(r.run.env, s.env.id());
        assert_eq!(r.run.executor, "fake-v0");
    }
    let again = exec
        .run_env_checks(&s.snapshot, RunIndex::FIRST)
        .await
        .unwrap();
    assert!(again.iter().all(|r| r.cache_hit));
    assert_eq!(exec.sandbox_starts(), 2);
}
