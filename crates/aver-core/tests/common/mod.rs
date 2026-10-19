#![allow(dead_code)]

use std::collections::BTreeMap;

use aver_core::*;

/// The fixed seed behind every sample signature and `schemas/vectors/v0.json`.
pub const SEED: [u8; 32] = {
    let mut seed = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        seed[i] = i as u8;
        i += 1;
    }
    seed
};

pub fn key() -> SigningKey {
    SigningKey::from_bytes(&SEED)
}

pub fn other_key() -> SigningKey {
    SigningKey::from_bytes(&[0xee; 32])
}

pub fn agent() -> AgentId {
    AgentId::of_public_key(&key().verifying_key())
}

pub fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

pub fn p<T: std::str::FromStr>(s: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    s.parse().unwrap()
}

pub fn blob_hello() -> BlobId {
    BlobId::of_bytes(b"hello\n")
}

pub fn sample_tree() -> Tree {
    let sub = Tree::new(
        Set::new(vec![TreeEntry {
            name: p("index.js"),
            node: Node::File(BlobId::of_bytes(b"module.exports = 1;\n")),
        }])
        .unwrap(),
    );
    Tree::new(
        Set::new(vec![
            TreeEntry {
                name: p("src"),
                node: Node::Tree(sub.id()),
            },
            TreeEntry {
                name: p("README.md"),
                node: Node::File(blob_hello()),
            },
            TreeEntry {
                name: p("run.sh"),
                node: Node::Exec(BlobId::of_bytes(b"#!/bin/sh\n")),
            },
            TreeEntry {
                name: p("link"),
                node: Node::Symlink(BlobId::of_bytes(b"README.md")),
            },
        ])
        .unwrap(),
    )
}

pub fn sample_env() -> Environment {
    Environment {
        schema: EnvSchema::V0,
        tests: Set::empty(),
        platform: Platform::LinuxArm64,
        image: Image {
            reference: "node:22-bookworm-slim".into(),
            digest: p(&format!("sha256:{}", "ab".repeat(32))),
        },
        lockfile: Some(Lockfile {
            path: p("package-lock.json"),
            blob: BlobId::of_bytes(b"{}\n"),
        }),
        install: Install {
            argv: vec!["npm".into(), "ci".into(), "--ignore-scripts".into()],
            allow_scripts: false,
            rebuild: false,
        },
        checks: Set::new(vec![
            CheckDef {
                id: p("unit"),
                argv: vec!["node".into(), "--test".into()],
                cwd: None,
                timeout_ms: Some(60_000),
                report: Some(Report {
                    format: ReportFormat::Junit,
                    path: p("/aver/out/unit.xml"),
                }),
                setup: Vec::new(),
                image: None,
                deps: None,
                covers: aver_core::Set::empty(),
                min_tests: None,
                approved: true,
            },
            CheckDef {
                id: p("typecheck"),
                argv: vec!["npx".into(), "tsc".into()],
                cwd: Some(p("packages/app")),
                timeout_ms: None,
                report: None,
                setup: Vec::new(),
                image: None,
                deps: None,
                covers: aver_core::Set::empty(),
                min_tests: None,
                approved: true,
            },
        ])
        .unwrap(),
        limits: Limits {
            cpu_millis: 2000,
            memory_mb: 2048,
            pids: 512,
            timeout_ms: 300_000,
            output_bytes: 65_536,
        },
        vars: BTreeMap::from([(p("NODE_OPTIONS"), "--max-old-space-size=1024".into())]),
        layers: aver_core::Set::empty(),
    }
}

/// [`sample_env`] declaring its test files: an `aver.env/v1`.
pub fn sample_env_v1() -> Environment {
    let tests = Set::new(vec![p("test/**"), p("**/*.spec.ts")]).unwrap();
    Environment {
        schema: EnvSchema::V1,
        tests,
        ..sample_env()
    }
}

/// [`sample_env`] with v2 features: a dependency rebuild, a setup command
/// and a Jest JSON report.
pub fn sample_env_v2() -> Environment {
    let base = sample_env();
    let mut checks: Vec<CheckDef> = base.checks.iter().cloned().collect();
    checks[1].setup = vec![vec!["npm".into(), "run".into(), "build".into()]];
    checks[1].report = Some(Report {
        format: ReportFormat::JestJson,
        path: p("/aver/out/unit.json"),
    });
    Environment {
        schema: EnvSchema::V2,
        install: Install {
            rebuild: true,
            ..base.install.clone()
        },
        checks: Set::new(checks).unwrap(),
        ..base
    }
}

/// [`sample_env`] with v3 features: a Cargo layer, a check with its own
/// image, deps, covers and a libtest report.
pub fn sample_env_v3() -> Environment {
    let base = sample_env();
    let mut checks: Vec<CheckDef> = base.checks.iter().cloned().collect();
    checks.push(CheckDef {
        image: Some(Image {
            reference: "rust:1.99-bookworm".into(),
            digest: p(&format!("sha256:{}", "cd".repeat(32))),
        }),
        deps: Some(Set::new(vec![p("Cargo.lock")]).unwrap()),
        covers: Set::new(vec![p("**/*.rs"), p("fixtures/**")]).unwrap(),
        report: Some(Report {
            format: ReportFormat::Libtest,
            path: p("/aver/out/rust.txt"),
        }),
        ..CheckDef::new(p("rust"), vec!["cargo".into(), "test".into()])
    });
    Environment {
        schema: EnvSchema::V3,
        layers: Set::new(vec![Layer {
            kind: LayerKind::Cargo,
            lockfile: Lockfile {
                path: p("Cargo.lock"),
                blob: BlobId::of_bytes(b"version = 4\n"),
            },
            install: Install {
                argv: vec!["cargo".into(), "fetch".into(), "--locked".into()],
                allow_scripts: false,
                rebuild: false,
            },
        }])
        .unwrap(),
        checks: Set::new(checks).unwrap(),
        ..base
    }
}

/// [`sample_env_v3`] with v4 fields: the Rust check's floor of 40 tests and
/// an unapproved exit-only lint.
pub fn sample_env_v4() -> Environment {
    let base = sample_env_v3();
    let mut checks: Vec<CheckDef> = base
        .checks
        .iter()
        .cloned()
        .map(|mut c| {
            if c.id.as_str() == "rust" {
                c.min_tests = std::num::NonZeroU32::new(40);
            }
            c
        })
        .collect();
    checks.push(CheckDef {
        approved: false,
        ..CheckDef::new(p("lint"), vec!["make".into(), "lint".into()])
    });
    Environment {
        schema: EnvSchema::V4,
        checks: Set::new(checks).unwrap(),
        ..base
    }
}

/// [`sample_receipt_v2`] for a head that also changed a Rust file no check
/// covers: `UNPROVEN`, with `uncovered`.
pub fn sample_receipt_v2_uncovered() -> Receipt {
    let mut body = sample_receipt_v2().body;
    body.unproven = Vec::new();
    body.criteria = Criteria::new(Vec::new()).unwrap();
    let checks: Vec<ReceiptCheck> = body
        .checks
        .iter()
        .filter(|c| c.proves.is_empty())
        .cloned()
        .collect();
    body.checks = Set::new(checks).unwrap();
    body.verdict = Verdict::Unproven;
    body.uncovered = Some(Listed::of(vec![p("crates/core/src/lib.rs")]));
    Receipt::sign(body, &key())
}

pub fn sample_snapshot() -> Snapshot {
    Snapshot::new(sample_tree().id(), sample_env().id())
}

pub fn sample_snapshot_v1() -> Snapshot {
    Snapshot::of_tree(sample_tree().id())
}

pub fn globs(patterns: &[&str]) -> Set<PathGlob> {
    Set::new(patterns.iter().map(|p| p.parse().unwrap()).collect()).unwrap()
}

pub fn spec(id: &str, run: &str, report: ReportKind, min_tests: Option<u32>) -> CheckSpec {
    CheckSpec {
        id: id.parse().unwrap(),
        run: run.parse().unwrap(),
        cwd: None,
        prepare: None,
        report,
        report_file: None,
        min_tests: min_tests.map(|n| n.try_into().unwrap()),
        paths: None,
        serial: false,
        timeout_s: None,
    }
}

/// One check of every reading: JUnit with paths and a prepare step, libtest
/// on stdout, jest-json from a file in a subdirectory, and exit-only.
pub fn sample_contract() -> Contract {
    let unit = CheckSpec {
        prepare: Some("npm run build".parse().unwrap()),
        paths: Some(globs(&["src/**", "test/**"])),
        serial: true,
        timeout_s: Some(600.try_into().unwrap()),
        ..spec(
            "unit",
            "node --test --test-reporter=junit --test-reporter-destination=\"$AVER_OUT\"/unit.xml",
            ReportKind::Junit,
            Some(1083),
        )
    };
    let rust = spec(
        "rust",
        "cargo test --workspace --no-fail-fast",
        ReportKind::Libtest,
        Some(900),
    );
    let web = CheckSpec {
        cwd: Some("web".parse().unwrap()),
        report_file: Some("jest/web.json".parse().unwrap()),
        ..spec(
            "web",
            "npx jest --json --outputFile=\"$AVER_OUT\"/jest/web.json",
            ReportKind::JestJson,
            Some(12),
        )
    };
    let lint = spec(
        "lint",
        "cargo clippy -- -D warnings",
        ReportKind::Exit,
        None,
    );
    let checks = [unit, rust, web, lint]
        .into_iter()
        .map(|s| ContractCheck::try_from(s).unwrap())
        .collect();
    Contract::new(
        Set::new(checks).unwrap(),
        Deps::new(
            globs(&["node_modules", "web/node_modules"]),
            globs(&["target"]),
            Some("npm ci && (cd web && npm ci)".parse().unwrap()),
            globs(&["package-lock.json", "web/package-lock.json"]),
        )
        .unwrap(),
        JudgeGlobs {
            tests: globs(&["e2e/**"]),
            harness: globs(&["scripts/test.sh"]),
            protected: globs(&["migrations/**"]),
            ..JudgeGlobs::default()
        },
        CiPolicy {
            review: ReviewPolicy::Neutral,
            require_recorded_base: true,
        },
    )
    .unwrap()
}

pub fn sample_transition() -> Transition {
    Transition::sign(
        TransitionBody {
            schema: Schema::new(),
            from: vec![],
            to: sample_snapshot().id(),
            via: Via::GitImport {
                repo: "file:///repos/calc".into(),
                commit: p(&"0123456789abcdef".repeat(3)[..40]),
            },
            agent: agent(),
            message: "Initial import\n".into(),
            at: ts("2026-10-10T12:00:00.000Z"),
        },
        &key(),
    )
}

pub fn workspace_transition() -> Transition {
    Transition::sign(
        TransitionBody {
            schema: Schema::new(),
            from: vec![sample_snapshot().id()],
            to: sample_snapshot().id(),
            via: Via::Workspace {
                workspace: p("ws_01K7A0000000000000000000AB"),
                attempt: Some(p("att_01K7A0000000000000000000CD")),
            },
            agent: agent(),
            message: "Fix divide by zero".into(),
            at: ts("2026-10-10T12:30:00.123Z"),
        },
        &key(),
    )
}

pub fn criteria() -> Criteria {
    Criteria::new(vec![
        Criterion {
            id: p("div-zero"),
            text: "divide throws on zero".into(),
        },
        Criterion {
            id: p("docs"),
            text: "README documents divide".into(),
        },
    ])
    .unwrap()
}

pub fn sample_task() -> Task {
    Task::sign(
        TaskBody {
            schema: TaskSchema::V0,
            title: p("Make divide throw on zero"),
            intent: p("divide(1, 0) currently returns Infinity."),
            base: sample_snapshot().id(),
            target: Some(p("trunk")),
            criteria: criteria(),
            checks: Set::new(vec![TaskCheckDef {
                id: p("task-div"),
                argv: vec!["node".into(), "--test".into(), "test/div.test.js".into()],
                cwd: None,
                timeout_ms: None,
                report: None,
                proves: Set::new(vec![p("div-zero")]).unwrap(),
            }])
            .unwrap(),
            protected_paths: Set::empty(),
            author: agent(),
            created_at: ts("2026-10-10T11:00:00.000Z"),
        },
        &key(),
    )
}

/// [`sample_task`] with protected paths, so `aver.task/v1`.
pub fn sample_task_v1() -> Task {
    let mut body = sample_task().body;
    body.protected_paths = Set::new(vec![p("package.json"), p("test/**")]).unwrap();
    body.schema = TaskSchema::for_task(&body.criteria, &body.protected_paths);
    Task::sign(body, &key())
}

/// A regression-only task: no criteria, so `aver.task/v2`. It keeps
/// protected paths, which v2 allows.
pub fn sample_task_v2() -> Task {
    let mut body = sample_task_v1().body;
    body.criteria = Criteria::new(Vec::new()).unwrap();
    body.checks = Set::empty();
    body.schema = TaskSchema::for_task(&body.criteria, &body.protected_paths);
    Task::sign(body, &key())
}

pub fn sample_run() -> CheckRun {
    CheckRun {
        schema: Schema::new(),
        snapshot: sample_snapshot().id(),
        env: sample_env().id(),
        check: p("unit"),
        index: RunIndex::FIRST,
        executor: "docker-runc-v0".into(),
        result: RunResult::Fail { exit_code: 1 },
        duration_ms: 1234,
        output: BlobId::of_bytes(b"1 failing\n"),
        report: Some(BlobId::of_bytes(b"<testsuites/>")),
        tests: Set::new(vec![
            TestResult {
                id: p("unit::calc > [crit:div-zero] divide throws on zero"),
                status: TestStatus::Fail,
                message: Some(p("Expected throw, got Infinity")),
            },
            TestResult {
                id: p("unit::calc > adds"),
                status: TestStatus::Pass,
                message: None,
            },
        ])
        .unwrap(),
        started_at: ts("2026-10-10T12:31:00.000Z"),
    }
}

pub fn sample_infra_run() -> CheckRun {
    CheckRun {
        result: RunResult::Sandbox,
        report: None,
        tests: Set::empty(),
        ..sample_run()
    }
}

pub fn sample_receipt() -> Receipt {
    let run = sample_run().id();
    Receipt::sign(
        ReceiptBody {
            schema: ReceiptSchema::V0,
            task_id: sample_task().id(),
            attempt_id: Some(p("att_01K7A0000000000000000000CD")),
            agent_id: agent(),
            base_snapshot: sample_snapshot().id(),
            head_snapshot: sample_snapshot().id(),
            environment_hash: sample_env().id(),
            env_changed: false,
            criteria: criteria(),
            checks: Set::new(vec![
                ReceiptCheck {
                    id: p("unit::calc > [crit:div-zero] divide throws on zero"),
                    base: BaseStatus::NotApplicable,
                    head: HeadStatus::Fail,
                    cause: Some(Cause::New),
                    proves: Set::new(vec![p("div-zero")]).unwrap(),
                    source: CheckSource::Env,
                    runs: SideRuns {
                        base: vec![run],
                        head: vec![run, run, run],
                        transplant: vec![],
                        head_tests: vec![],
                    },
                    detail: Some(p("Expected throw, got Infinity")),
                    transplant: None,
                    head_tests: None,
                },
                ReceiptCheck {
                    id: p("typecheck"),
                    base: BaseStatus::Pass,
                    head: HeadStatus::Pass,
                    cause: None,
                    proves: Set::empty(),
                    source: CheckSource::Env,
                    runs: SideRuns {
                        base: vec![run],
                        head: vec![run],
                        transplant: vec![],
                        head_tests: vec![],
                    },
                    detail: None,
                    transplant: None,
                    head_tests: None,
                },
            ])
            .unwrap(),
            unproven: vec![p("div-zero"), p("docs")],
            verdict: Verdict::FailOwn,
            reproduce: "aver verify --fresh".into(),
            verifier: "aver-verify/0.1.0".into(),
            created_at: ts("2026-10-10T12:32:00.000Z"),
            executor: None,
            config: None,
            test_changes: None,
            ci: None,
            uncovered: None,
            skipped: None,
            provenance: None,
            levels: None,
        },
        &key(),
    )
}

/// A CI receipt as verification writes it now: `aver.receipt/v2`, with the
/// transplant result of each proving entry and the run's commits.
pub fn sample_receipt_v2() -> Receipt {
    let run = sample_run().id();
    let mut body = sample_receipt().body;
    body.schema = ReceiptSchema::V2;
    let mut checks: Vec<ReceiptCheck> = body.checks.as_slice().to_vec();
    let div_zero = checks
        .iter_mut()
        .find(|c| !c.proves.is_empty())
        .expect("the div-zero entry");
    div_zero.runs.transplant = vec![run];
    div_zero.runs.head_tests = vec![run, run, run];
    div_zero.transplant = Some(Transplant::Absent);
    div_zero.head_tests = Some(Transplant::Fail);
    checks.push(ReceiptCheck {
        id: p("unit::docs > [crit:docs] the readme documents divide"),
        base: BaseStatus::Fail,
        head: HeadStatus::Pass,
        cause: None,
        proves: Set::new(vec![p("docs")]).unwrap(),
        source: CheckSource::Env,
        runs: SideRuns {
            base: vec![run, run, run],
            head: vec![run],
            transplant: vec![run],
            head_tests: vec![run],
        },
        detail: None,
        transplant: Some(Transplant::Pass),
        head_tests: Some(Transplant::Pass),
    });
    body.executor = Some("docker-runsc-v0".into());
    body.config = Some(ConfigSource::AverToml);
    body.test_changes = Some(TestChanges {
        files_added: Listed::of(vec![p("test/docs.test.js")]),
        files_modified: Listed::of(vec![p("test/divide.test.js")]),
        files_deleted: Listed::of(Vec::new()),
        tests_removed: Listed::of(vec![p("unit::calc > divides by one")]),
        tests_skipped: Listed::of(Vec::new()),
        harness_changed: Listed::of(Vec::new()),
    });
    body.checks = Set::new(checks).unwrap();
    body.reproduce = "aver ci --repo https://example.invalid/calc.git --base 1111111111111111111111111111111111111111 --head 2222222222222222222222222222222222222222 --criteria-sha256 0000000000000000000000000000000000000000000000000000000000000000 --criteria criteria.txt".into();
    body.ci = Some(CiInfo {
        base_commit: p("1111111111111111111111111111111111111111"),
        head_commit: p("2222222222222222222222222222222222222222"),
        config_path: Some(p("aver.toml")),
        criteria_text: Some(
            "- div-zero: divide throws on zero\n- docs: README documents divide".into(),
        ),
        subject: Some(p("example/calc#7")),
        requested_base: None,
    });
    Receipt::sign(body, &key())
}

/// The host profile of the v3 sample: a Mac with one toolchain.
pub fn sample_host_profile() -> HostProfile {
    HostProfile::of(
        HostOs::Darwin,
        HostArch::Arm64,
        &[HostTool {
            name: "node".into(),
            path: "/opt/homebrew/Cellar/node/24.2.0/bin/node".into(),
            version: b"v24.2.0\n".to_vec(),
        }],
    )
}

/// A local receipt as verification writes it since round 14:
/// `aver.receipt/v3`, run natively on the verifier's machine with the
/// network off, each check's level recorded.
pub fn sample_receipt_v3() -> Receipt {
    let mut body = sample_receipt_v2().body;
    body.schema = ReceiptSchema::V3;
    body.ci = None;
    body.reproduce = "aver verify --task task_x --head snap_y --base snap_z --fresh".into();
    let profile = sample_host_profile();
    body.executor = Some(profile.executor());
    body.provenance = Some(Provenance {
        mode: ProvenanceMode::Local,
        host_profile: Some(profile),
        network: Network::Off,
        uid: Uid::Same,
    });
    body.levels = Some(
        [(p("typecheck"), Level::Native), (p("unit"), Level::Junit)]
            .into_iter()
            .collect(),
    );
    Receipt::sign(body, &key())
}

/// A receipt for [`sample_task_v2`]: no criteria, so `aver.receipt/v1`.
pub fn sample_receipt_v1() -> Receipt {
    let mut body = sample_receipt().body;
    body.task_id = sample_task_v2().id();
    body.criteria = Criteria::new(Vec::new()).unwrap();
    body.checks = Set::new(
        body.checks
            .as_slice()
            .iter()
            .cloned()
            .map(|mut c| {
                c.proves = Set::empty();
                c
            })
            .collect(),
    )
    .unwrap();
    body.unproven = Vec::new();
    body.schema = ReceiptSchema::for_criteria(&body.criteria);
    Receipt::sign(body, &key())
}

pub fn sample_attempt() -> AttemptRecord {
    AttemptRecord {
        schema: Schema::new(),
        attempt: p("att_01K7A0000000000000000000CD"),
        task: sample_task().id(),
        agent: agent(),
        base: sample_snapshot().id(),
        head: sample_snapshot().id(),
        receipt: sample_receipt().id(),
        outcome: AttemptOutcome::Rejected,
        submitted_at: ts("2026-10-10T12:33:00.000Z"),
    }
}

pub fn sample_ledger() -> LedgerEntry {
    LedgerEntry::sign(
        LedgerBody {
            schema: Schema::new(),
            scope: "ref:trunk".into(),
            prev: None,
            kind: LedgerKind::Reject,
            task: Some(sample_task().id()),
            attempts: vec![LedgerAttempt {
                attempt: p("att_01K7A0000000000000000000CD"),
                receipt: sample_receipt().id(),
                verdict: "FAIL_OWN".into(),
            }],
            winner: None,
            base: Some(sample_snapshot().id()),
            head: None,
            summary: "Rejected attempt".into(),
            rationale: "New failure in div-zero".into(),
            lessons: vec!["divide still returns Infinity".into()],
            agent: agent(),
            at: ts("2026-10-10T12:34:00.000Z"),
        },
        &key(),
    )
}

/// Every sample object as (schema kind, id prefix, canonical bytes, id string).
pub fn all_samples() -> Vec<(&'static str, &'static str, Vec<u8>, String)> {
    fn entry<T: Object>(
        kind: &'static str,
        obj: T,
    ) -> (&'static str, &'static str, Vec<u8>, String) {
        let id = obj.id();
        (kind, id.prefix(), obj.canonical_bytes(), id.to_string())
    }
    vec![
        entry("tree", sample_tree()),
        entry("env", sample_env()),
        entry("env", sample_env_v1()),
        entry("env", sample_env_v2()),
        entry("env", sample_env_v3()),
        entry("env", sample_env_v4()),
        entry("snapshot", sample_snapshot()),
        entry("snapshot", sample_snapshot_v1()),
        entry("transition", sample_transition()),
        entry("transition", workspace_transition()),
        entry("task", sample_task()),
        entry("task", sample_task_v1()),
        entry("task", sample_task_v2()),
        entry("run", sample_run()),
        entry("run", sample_infra_run()),
        entry("receipt", sample_receipt()),
        entry("receipt", sample_receipt_v1()),
        entry("receipt", sample_receipt_v2()),
        entry("receipt", sample_receipt_v2_uncovered()),
        entry("receipt", sample_receipt_v3()),
        entry("attempt", sample_attempt()),
        entry("ledger", sample_ledger()),
        entry("contract", sample_contract()),
    ]
}
