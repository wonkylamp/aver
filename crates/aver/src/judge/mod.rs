//! The verification engine (ARCHITECTURE.md §2.5): runs a task's checks on
//! base and head and signs a receipt (§1.9.3) attributing every failure.
//!
//! Everything that judges comes from the base: the check set is
//! `B.checks ∪ task.checks`, where `B` is the base snapshot's environment,
//! and every check runs in `B` on both sides; head-side runs use `B` with
//! the head's lockfile, so a dependency change installs. The head runs as
//! its *judged head*: the head tree with the judging set
//! ([`crate::judging`]) put back as the base has it: frozen harness files,
//! the frozen fields of each `package.json`, and every test file that exists
//! at base. A head change to frozen harness is a failing `judging::<path>`
//! (or `judging::<path>#<field>`) entry with cause `CHANGED`, for a person
//! to review; a change to an existing test is not, because the base's copy
//! still judges regressions, and is `CHANGED` where that copy fails on
//! the head while the head's own version passes, or where the head skips
//! or deletes a test that still passes.
//!
//! Runs go through the execution service and its cache, with the rerun
//! policy of [`rules::needs_rerun`]. A check whose runs carry a JUnit report
//! on both sides becomes one entry per test, plus a `<check>::(exit)` entry
//! when a head run exits non-zero without a failing test or reports no tests
//! where the base reported some; otherwise it is one command-level entry
//! classified by exit code, where a declared report that is missing or
//! malformed counts as a failure. A test proves the criteria whose
//! `[crit:<id>]` tag its name carries, among this task's criteria only; a
//! task check proves its `proves` list. Causes and the verdict come from
//! [`judge::judge`] and [`rules::verdict`].
//!
//! A task's protected paths add one entry per file the head adds, changes
//! or deletes under them, `protected::<path>` with head `fail` and cause
//! `NEW`, so such a head can never verify as `PASS`.
//!
//! An environment check that no changed file can affect
//! ([`crate::coverage::affects`]) runs on neither side and is listed in the
//! receipt's `skipped`, unless the head changed nothing or the request asks
//! for every check (`all`).
//!
//! Receipts are `aver.receipt/v2`, whose proof rule is the transplant
//! (§1.9.3): a check that can prove a criterion also runs on the base tree
//! plus the head's new and modified test files, and a proving entry proves
//! only if it failed (or was absent) there and passed in the head-tests run
//! (the head with its own versions of the test files). The receipt's
//! `test_changes` reports what the head did to the tests. Each receipt gets
//! unsigned diagnostics ([`crate::diagnostics`]) stored in the same
//! transaction.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aver_core::evidence::{self, Blocker, SideEvidence, SideFault};
use aver_core::judge::{self, Base, Judged, Origin, Own};
use aver_core::receipt::MAX_DETAIL_BYTES;
use aver_core::rules::{self, Classification, UnprovenReason};
use aver_core::{
    AgentId, AttemptId, BaseStatus, BlobId, Cause, CheckDef, CheckId, CheckRun, CheckSource,
    CiInfo, ConfigSource, Criteria, CriterionId, Ending, EntryId, EnvId, EnvSchema, Environment,
    HeadStatus, Layer, Level, Listed, Lockfile, Node, Path, ProofRule, Receipt, ReceiptBody,
    ReceiptCheck, ReceiptId, ReceiptSchema, RunId, RunIndex, Set, SideRuns, SigningKey, SkipReason,
    Skipped, Snapshot, SnapshotId, Task, TaskId, TestChanges, TestCheckId, TestStatus, Text,
    Timestamp, Transplant, WorkspaceId,
};

use crate::diagnostics::{self, Diagnostics, Failure, TestChangeLists, Unproven};
use crate::exec::junit::{self, EXIT_TEST_PATH};
use crate::exec::{task_check, toolchain, ExecError, ExecutedRun, Executor};
use crate::judging::{self, Content, JudgingChange, RetargetedTest, TestFiles};
use crate::place::runner::Place;
use crate::place::sides::SlotName;
use crate::place::workdir::Changed;
use crate::store::{Store, StoreError};
use crate::tasks::{self, TaskError};
use crate::workspace::{FileEntry, FileKind};
use crate::workspace::{WorkspaceError, Workspaces};

mod ablate;
mod finish;
mod focus;
mod hunks;
mod narrow;
mod plan;
mod run;

use finish::*;
pub use finish::{
    reproduce, DELETED_SOURCE_TEST_PATH, EVIDENCE_TEST_PATH, NOT_RERUN_TEST_PATH, SETUP_TEST_PATH,
    UNBUILT_TEST_PATH,
};
use plan::*;
use run::*;

/// The verifier version recorded on every receipt.
pub const VERIFIER: &str = concat!("aver-verify/", env!("CARGO_PKG_VERSION"));

const MISSING_FROM_HEAD: &str = "missing from head report";

const SKIPPED_ON_HEAD: &str = "skipped on head";

/// The check id under which protected-path entries are reported. A task with
/// protected paths may not define a check with this id, and verifying one
/// fails if the base environment defines it.
pub const PROTECTED_CHECK: &str = "protected";
/// The check id under which built-artifact entries are reported. No check
/// of the base environment or the task may use it.
pub use aver_core::receipt::BUILT_CHECK;
/// The check id under which judging-file entries are reported. No check of
/// the base environment or the task may use it.
pub use aver_core::receipt::JUDGING_CHECK;

/// The check id under which edits under another task's exclusive lease are
/// reported (§2.11). A verification with such an edit fails if a check
/// uses this id.
pub const LEASE_CHECK: &str = "lease";

/// The check id under which changes to the base contract's sensitive paths
/// are reported (§1.13). A verification with such a change fails if a
/// check uses this id.
pub const SENSITIVE_CHECK: &str = "sensitive";

/// The largest head test file searched for a criterion's tag when the
/// diagnostics explain an unclaimed criterion.
const MAX_TAG_SEARCH_BYTES: usize = 1024 * 1024;

/// Most changed files [`Verifier::changed_names`] reads.
const MAX_NAME_FILES: usize = 500;

/// The longest path an entry id names in full; longer ones are cut and
/// marked, and their entry's detail names them in full.
const MAX_ENTRY_PATH_BYTES: usize = 1000;

/// What to verify.
#[derive(Debug, Clone)]
pub enum Head {
    /// The workspace's current state against its base.
    Workspace(WorkspaceId),
    /// A snapshot against `base`, by default the attempt's base if an attempt
    /// is given, else the task's base.
    Snapshot {
        head: SnapshotId,
        base: Option<SnapshotId>,
    },
}

/// The slots of one verification's sides; a side without one runs in a
/// slot of its own snapshot.
#[derive(Debug, Clone)]
struct Places {
    base: Place,
    head: Option<Place>,
    transplant: Option<Place>,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub task: TaskId,
    pub head: Head,
    /// The attempt whose receipt list the receipt is appended to.
    pub attempt: Option<AttemptId>,
    /// Run every check in a new sandbox instead of reading the cache.
    pub fresh: bool,
    /// Run every check, also those the change does not affect (§1.9.3).
    pub all: bool,
    /// Where to log each run as it starts and ends, for progress lines.
    pub progress: Option<Arc<crate::progress::Log>>,
    /// Set by CI mode: what the receipt records about the run, and the
    /// `aver ci` command that reproduces it.
    pub ci: Option<CiRequest>,
    /// `aver check -- <cmd>`: this one check instead of the task's, in a
    /// dry run only (§2.7.1).
    pub adhoc: Option<CheckDef>,
    /// Why the change rewrites what it does, as the agent (`aver verify
    /// --reason`) or the head commit (`Aver-Why:`) states it: unchecked
    /// data for a reviewer, kept in the diagnostics, never in the verdict.
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CiRequest {
    pub info: CiInfo,
    pub reproduce: String,
    /// `aver ci --check <id>`: the one check this receipt covers, as one
    /// job of a matrix; the others are left out of it.
    pub only: Option<CheckId>,
}

#[derive(Debug, Clone)]
pub struct Verified {
    pub id: ReceiptId,
    pub receipt: Receipt,
    pub diagnostics: Diagnostics,
    /// Every run the verification used, once each, with its sandbox time.
    pub runs: Vec<TimedRun>,
    /// Where the wall time went.
    pub timing: Timing,
}

/// Where a verification's wall time went: planning (snapshots of the judged
/// head, head-tests and transplant trees), each side's runs (the slowest
/// check's; sides run concurrently), and judging and signing.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Timing {
    pub plan_ms: u64,
    pub sides: Vec<SideTime>,
    pub finish_ms: u64,
    pub total_ms: u64,
}

/// One side's wall time and how many of its runs started a sandbox or came
/// from the cache.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SideTime {
    pub side: Side,
    pub ms: u64,
    #[serde(default)]
    pub started: u32,
    #[serde(default)]
    pub cached: u32,
    /// When the oldest run this side read from the run cache started: the
    /// same code in the same environment ran then (an earlier verify, a
    /// background warm, or `aver check`), and its results are
    /// what this verification reports. `None` when nothing came from the
    /// cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_from: Option<Timestamp>,
}

impl SideTime {
    fn of(side: Side, took: Duration) -> Self {
        Self {
            side,
            ms: ms(took),
            started: 0,
            cached: 0,
            cached_from: None,
        }
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Which tree a run executed (§2.5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Base,
    /// The judged head.
    Head,
    /// The head with its own versions of the test files.
    HeadTests,
    Transplant,
    /// The head-tests tree with one hunk of the change reverted (§1.9.4).
    Ablation,
}

/// One run of a verification and the sandbox time it cost: zero for a cache
/// hit or a run another side already counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedRun {
    pub side: Side,
    pub check: CheckId,
    pub index: u8,
    pub run: RunId,
    pub cache_hit: bool,
    /// Read from the run cache, not run by this verification: an earlier
    /// one, a warm or `aver check` ran it (`started_at`).
    pub from_cache: bool,
    pub started_at: Timestamp,
    pub install: Duration,
    pub sandbox: Duration,
    /// A fresh base run that checked a recorded base, judging nothing.
    pub confirmation: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Task(#[from] TaskError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Exec(ExecError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("verification: {0}")]
    Internal(String),
    /// Nothing to run: no check in the base's (or a proposed) `aver.toml`,
    /// none in the task and no `aver check -- <cmd>`. The message says how
    /// to name one.
    #[error("{0}")]
    NotChecked(String),
    /// A signal asked this process to stop: no receipt, and nothing of
    /// the interrupted runs is stored.
    #[error("interrupted")]
    Interrupted,
}

impl From<ExecError> for VerifyError {
    fn from(e: ExecError) -> Self {
        match e {
            ExecError::Interrupted => Self::Interrupted,
            ExecError::Store(_) | ExecError::Internal(_) => Self::Exec(e),
        }
    }
}

/// Passes crash point `name` ([`crate::fault`]).
fn fault(name: &str) -> Result<(), VerifyError> {
    crate::fault::point(name).map_err(|e| VerifyError::Internal(format!("{name}: {e}")))
}

/// A check to run on both sides, with who defined it and what it proves.
#[derive(Debug, Clone)]
struct Planned {
    def: CheckDef,
    source: CheckSource,
    proves: Set<CriterionId>,
}

/// Everything verification needs, resolved before any sandbox starts.
struct Plan {
    task_id: TaskId,
    /// The attempt the receipt is appended to: the requested one, else the
    /// open attempt at this task that owns the verified workspace.
    attempt: Option<AttemptId>,
    task: Task,
    base: SnapshotId,
    head: SnapshotId,
    /// The head tree with the judging set put back as the base has it
    /// (frozen files, frozen `package.json` fields, existing tests): what
    /// head runs execute.
    judged_head: SnapshotId,
    /// The judged head without the test files the head added: what the
    /// head runs of the base environment's per-test checks execute (§2.5.1).
    judged_base_tests: SnapshotId,
    /// The judged head with the head's own versions of the test files.
    head_tests: SnapshotId,
    /// The base environment, which every check runs in on both sides.
    env: Arc<Environment>,
    env_id: EnvId,
    /// The base environment with the head's lockfile: what the judged head
    /// and the head-tests tree run in. Base and transplant runs use `env`.
    head_env: Arc<Environment>,
    env_changed: bool,
    checks: Vec<Planned>,
    protected: Vec<ReceiptCheck>,
    /// One `CHANGED` entry per path the head changed under another task's
    /// live exclusive lease (§2.11), for a workspace head.
    leased: Vec<ReceiptCheck>,
    /// One `CHANGED` entry per path the head changed under the base
    /// contract's `[judge] sensitive` globs.
    sensitive: Vec<ReceiptCheck>,
    judging: Vec<JudgingChange>,
    tests: TestFiles,
    /// The head's `[[test]]` targets the base's frozen manifests lack.
    retargeted: Vec<RetargetedTest>,
    /// Rust source files the head deleted with the tests inside them.
    deleted_sources: Vec<Path>,
    /// Whether the base has an `aver.toml`.
    config: ConfigSource,
    /// Whether the checks are the head's own proposed `aver.toml`
    /// (`Approval::Proposed`), which caps the verdict at `REVIEW`.
    proposed: bool,
    /// The request's stated reason, cut to [`diagnostics::MAX_REASON_BYTES`].
    reason: Option<String>,
    ci: Option<CiRequest>,
    /// The base tree plus the head's new and modified test files, in the
    /// base environment; `None` when a judging file changed, so nothing
    /// proves.
    transplant: Option<SnapshotId>,
    /// The head's new and modified test files, which the transplant adds.
    transplant_files: Vec<String>,
    /// The source files the head changed that no check covers (§1.9.3).
    uncovered: Vec<Path>,
    /// The uncovered files whose change is behind Cargo features no check
    /// enables, with what would build them.
    unbuilt: Vec<crate::cargo_features::Unbuilt>,
    /// Environment checks the change does not affect, which run on neither
    /// side (§1.9.3).
    skipped: Vec<CheckId>,
    /// The changed source files whose hunks the necessity phase may
    /// revert (§1.9.4).
    sources: Vec<Source>,
    /// Every check was asked for (`--all`).
    all: bool,
}

/// A changed source file a check covers: its base and head contents.
#[derive(Debug, Clone)]
struct Source {
    path: Path,
    /// `None` when the head added it.
    base: Option<BlobId>,
    head: BlobId,
}

/// One check's runs on each side, in run order. `head_tests` are the head
/// runs themselves when the head-tests tree is the judged head;
/// `transplant` is empty when the check cannot prove anything or the plan
/// has no transplant.
#[derive(Clone)]
struct CheckRuns {
    planned: Planned,
    base: Vec<ExecutedRun>,
    head: Vec<ExecutedRun>,
    head_tests: Vec<ExecutedRun>,
    /// Whether `head_tests` ran on their own tree.
    separate: bool,
    transplant: Vec<ExecutedRun>,
    /// Set when the judged head lost its report and Aver probed its build
    /// ([`rebuild`]).
    rebuild: Option<Rebuild>,
    /// The transplant and head-tests sides Aver tried to narrow.
    narrowed: Vec<diagnostics::Narrowed>,
    /// Fresh base runs that checked a recorded base and were not kept
    /// ([`confirm_record`]): listed with the timings and logs only.
    confirmations: Vec<ExecutedRun>,
}

impl CheckRuns {
    fn rebuilt(&self) -> Option<&Rebuilt> {
        match &self.rebuild {
            Some(Rebuild::Rebuilt(r)) => Some(r),
            _ => None,
        }
    }
}

/// What probing a judged head that lost its report found.
#[derive(Clone)]
enum Rebuild {
    /// `head` are runs over the test targets that built, because restored
    /// base test files did not compile.
    Rebuilt(Rebuilt),
    /// Aver lost the probe's result (an infra run, or a stream that stops
    /// before cargo's `build-finished`), so it cannot tell whether restored
    /// test files or the change stopped the build: the check is `ENV`
    /// (§1.9.3, who lost the result).
    Lost(ExecutedRun),
}

/// The judged head run again without the test targets that did not build
/// (§1.9.3, restored test files that do not build).
#[derive(Clone)]
struct Rebuilt {
    /// The `cargo build --keep-going` run that found them.
    probe: ExecutedRun,
    /// The head's first runs, which lost the report.
    lost: Vec<ExecutedRun>,
    unbuilt: crate::exec::cargo::Unbuilt,
}

/// The run a receipt entry's detail came from, and what else its
/// diagnostics say about it.
#[derive(Debug, Clone, Default)]
struct Evidence {
    run: Option<RunId>,
    test: Option<TestCheckId>,
    /// How the head's own version of the test fared, for a `NEW` or
    /// `CHANGED` entry of a check whose head-tests tree is its own.
    own: Option<Transplant>,
    /// The existing test file the head changed that defines a `CHANGED`
    /// entry, when the test's path names it.
    file: Option<Path>,
    /// Aver's words on why the entry has its cause.
    note: Option<String>,
    /// The build errors its transplant result rests on, when the
    /// transplant failed to build over a base that built (§1.9.4,
    /// `build_error`).
    build: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct Verifier {
    store: Arc<Store>,
    workspaces: Arc<Workspaces>,
    executor: Executor,
    server_key: Arc<SigningKey>,
    /// A dry run ([`Verifier::dry_run`]): the same plan and runs, a receipt
    /// signed by a throwaway key, nothing stored.
    dry: bool,
}

impl Verifier {
    pub fn new(
        store: Arc<Store>,
        workspaces: Arc<Workspaces>,
        executor: Executor,
        server_key: Arc<SigningKey>,
    ) -> Self {
        Self {
            store,
            workspaces,
            executor,
            server_key,
            dry: false,
        }
    }

    /// `aver check` (§3.4): what [`Verifier::verify`] would sign for `req`,
    /// from the same plan, rerun policy and run cache keys, so a verify of
    /// the same tree right after starts no run. The receipt is signed by a
    /// key made for this call and discarded, so it is evidence of nothing;
    /// neither it nor its diagnostics is stored, and no attempt's receipt
    /// list (the verify budget) changes.
    pub async fn dry_run(&self, caller: AgentId, req: Request) -> Result<Verified, VerifyError> {
        let this = Self {
            dry: true,
            ..self.clone()
        };
        let started = Instant::now();
        let adhoc = req.adhoc.is_some();
        let mut verified = this
            .verify(
                caller,
                Request {
                    attempt: None,
                    ..req
                },
            )
            .await?;
        // `aver check -- <cmd>` reads its transplant runs back by id.
        if adhoc {
            verified.diagnostics.timings =
                Some(diagnostics::Timings::of(started.elapsed(), &verified.runs));
        }
        Ok(verified)
    }

    /// The base, head and transplant slots of a verification of `head` on
    /// this machine (§1.8.5): a workspace's sides are cloned from its
    /// directory and its base from the repository it came from; a head
    /// snapshot's sides run in slots of their own snapshots.
    fn places(&self, head: &Head, base: SnapshotId) -> Result<Option<Places>, VerifyError> {
        let Some(runner) = self.executor.slot_runner() else {
            return Ok(None);
        };
        let tree = self.store.get::<Snapshot>(&base)?.root;
        Ok(Some(match head {
            Head::Workspace(ws) => {
                let m = self.workspaces.disk().materialization(ws).ok().flatten();
                let dir = m.as_ref().map(|m| m.dir.clone());
                let repo = m.and_then(|m| m.repo);
                let base = runner.base_place(repo.as_deref().or(dir.as_deref()), tree);
                let head = Place {
                    slot: SlotName::Judged(*ws),
                    source: dir.clone(),
                    ..base.clone()
                };
                // The transplant is the base tree plus the head's test
                // files: the base slot's build caches are what it needs.
                let transplant = Place {
                    slot: SlotName::Transplant(*ws),
                    seed: Some(base.slot.clone()),
                    nice: true,
                    ..head.clone()
                };
                Places {
                    base,
                    head: Some(head),
                    transplant: Some(transplant),
                }
            }
            Head::Snapshot { .. } => Places {
                base: runner.base_place(None, tree),
                head: None,
                transplant: None,
            },
        }))
    }

    /// The base slot a warm-up of `base` runs in: that of the repository
    /// it was imported from, found through the workspaces made here
    /// (before any exists on this base), else the default one.
    fn warm_place(&self, base: SnapshotId) -> Result<Option<Place>, VerifyError> {
        let Some(runner) = self.executor.slot_runner() else {
            return Ok(None);
        };
        let disk = self.workspaces.disk();
        let repo = disk.repo_of_base(&base).or_else(|| {
            crate::git::imported_commit(&self.store, &base)
                .ok()
                .flatten()
                .and_then(|sha| disk.repo_with_commit(&sha))
        });
        let tree = self.store.get::<Snapshot>(&base)?.root;
        Ok(Some(runner.base_place(repo.as_deref(), tree)))
    }

    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    fn planner(&self) -> Planner {
        Planner::new(self.store.clone(), self.workspaces.clone())
    }

    /// Verifies on behalf of `caller` and stores the signed receipt.
    pub async fn verify(&self, caller: AgentId, req: Request) -> Result<Verified, VerifyError> {
        let started = Instant::now();
        let this = self.clone();
        let plan_req = req.clone();
        let plan = Arc::new(blocking(move || this.planner().plan(&caller, &plan_req)).await?);
        fault("verify.planned")?;
        let planned_at = Instant::now();
        let pools = self.places(&req.head, plan.base)?;
        // The directory the head's slots are cloned from, whose kept
        // dependency directories every head run shares.
        let source = pools
            .as_ref()
            .and_then(|p| p.head.as_ref().unwrap_or(&p.base).source.clone());
        let (results, mut sides) = self
            .run_checks(&plan, req.fresh, req.progress.clone(), pools.clone())
            .await?;
        let ran_at = Instant::now();

        let (this, judged_plan) = (self.clone(), plan.clone());
        let judgment =
            blocking(move || this.judge_runs(&caller, &judged_plan, &results, source.as_deref()))
                .await?;
        let judged = ran_at.elapsed();
        // Hunk necessity (§1.9.4) only ever lowers a PASS, so it runs for
        // nothing else.
        let ablation_started = Instant::now();
        let (necessity, ablation) = if judgment.body.verdict == aver_core::Verdict::Pass {
            self.necessity(
                &plan,
                &judgment,
                req.fresh,
                req.progress.clone(),
                pools.as_ref(),
            )
            .await?
        } else {
            (None, Vec::new())
        };
        if !ablation.is_empty() {
            sides.push(SideTime::of(Side::Ablation, ablation_started.elapsed()));
        }
        let sealing = Instant::now();
        let this = self.clone();
        let sealed_plan = plan.clone();
        let mut verified =
            blocking(move || this.seal(&sealed_plan, judgment, necessity, ablation)).await?;
        let finish = judged + sealing.elapsed();
        for side in &mut sides {
            for r in verified.runs.iter().filter(|r| r.side == side.side) {
                if r.cache_hit {
                    side.cached += 1;
                } else {
                    side.started += 1;
                }
                if r.from_cache {
                    side.cached_from = Some(match side.cached_from {
                        Some(t) if t.unix_millis() <= r.started_at.unix_millis() => t,
                        _ => r.started_at,
                    });
                }
            }
        }
        sides.retain(|s| s.ms > 0 || s.started + s.cached > 0);
        sides.sort_by_key(|s| s.side as u8);
        verified.timing = Timing {
            plan_ms: ms(planned_at - started),
            sides,
            finish_ms: ms(finish),
            total_ms: ms(started.elapsed()),
        };
        Ok(verified)
    }

    /// Runs every check on the task's base, as a verification would, and
    /// leaves the runs in the cache without signing anything, so the first
    /// verify of an attempt waits only for its own sides (`aver run` calls
    /// this while its agents start). Returns how many sandboxes it started.
    pub async fn warm(&self, caller: AgentId, task: TaskId) -> Result<u32, VerifyError> {
        Ok(self.warm_only(caller, task, None).await?.len() as u32)
    }

    /// [`Verifier::warm`] for one check only, when `only` names one.
    pub async fn warm_only(
        &self,
        caller: AgentId,
        task: TaskId,
        only: Option<CheckId>,
    ) -> Result<Vec<ExecutedRun>, VerifyError> {
        let this = self.clone();
        let plan = blocking(move || {
            let base = this
                .store
                .get::<Task>(&task)
                .map_err(|e| match e {
                    StoreError::NotFound(_) => {
                        VerifyError::NotFound(format!("task {task} not found"))
                    }
                    other => other.into(),
                })?
                .body
                .base;
            this.planner().plan(
                &caller,
                &Request {
                    task,
                    head: Head::Snapshot {
                        head: base,
                        base: None,
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
        })
        .await?;
        let exec = match self.warm_place(plan.base)? {
            Some(place) => self.executor.in_slot(place),
            None => self.executor.clone(),
        };
        let mut started = Vec::new();
        for planned in plan
            .checks
            .iter()
            .filter(|p| only.as_ref().is_none_or(|id| *id == p.def.id))
        {
            let runs = run_side(
                &exec,
                &self.store,
                &plan.env,
                &plan.base,
                &planned.def,
                Reruns::BASE,
                false,
                false,
                None,
                None,
                None,
            )
            .await?;
            started.extend(runs.into_iter().filter(|r| !r.cache_hit));
        }
        Ok(started)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An edited expectation in a workspace crate's test file is found as
    /// the file that defines its libtest entries, so it can be `CHANGED`
    /// (`REVIEW`, exit 3 in CI) rather than `NEW`.
    #[test]
    fn a_crate_relative_libtest_target_names_its_changed_file_crit_ci_trust_review_exit3() {
        let changed: BTreeSet<String> = ["crates/aver/tests/ci.rs", "crates/core/tests/rules.rs"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            crate_target(&changed, "tests/ci.rs").map(String::as_str),
            Some("crates/aver/tests/ci.rs")
        );
        assert_eq!(crate_target(&changed, "tests/other.rs"), None);
        let both: BTreeSet<String> = ["a/tests/ci.rs", "b/tests/ci.rs"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(crate_target(&both, "tests/ci.rs"), None, "ambiguous");
    }

    #[test]
    fn command_globs_are_read_relative_to_the_checks_directory() {
        let def = |argv: &[&str], cwd: Option<&str>| {
            let mut def = CheckDef::new(
                "unit".parse().unwrap(),
                argv.iter().map(|a| (*a).to_owned()).collect(),
            );
            def.cwd = cwd.map(|c| c.parse().unwrap());
            def
        };
        let globs = command_globs(&def(
            &[
                "node",
                "--test",
                "--test-reporter=junit",
                "./integration/**/*.test.ts",
            ],
            None,
        ))
        .unwrap();
        assert!(globs[0].covers("integration/mail/inbound.test.ts"));
        assert!(!globs[0].covers("test/accounts.test.ts"));
        let nested =
            command_globs(&def(&["node", "--test", "test/*.test.js"], Some("site"))).unwrap();
        assert!(nested[0].covers("site/test/a.test.js"));
        assert_eq!(command_globs(&def(&["npm", "test"], None)), None);
        assert_eq!(
            command_globs(&def(&["sh", "-c", "go test ./... *_test.go"], None)),
            None,
            "a shell script's globs are not read"
        );
    }
}
