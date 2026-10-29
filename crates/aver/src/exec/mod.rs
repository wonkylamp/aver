//! The execution service (ARCHITECTURE.md §2.4): runs one check on one
//! snapshot under one environment, in a slot on this machine
//! ([`crate::place::runner`]) or through the scripted runner seam
//! ([`seam`]), and stores the result as a [`CheckRun`].
//!
//! [`Executor::run`] is a function of `(snapshot, env, check, index)` and the
//! runner's executor profile. `pass` and `fail` runs are cached under that
//! key in `run_cache` and served without running again; `infra` runs are
//! stored for audit but never served from the cache, so the next request
//! retries. Concurrent identical requests share one in-flight run, and at most
//! [`ExecConfig::max_sandboxes`] runs go at once.
//!
//! The environment passed in is the one the run uses and keys; it may differ
//! from the snapshot's own (verification runs every check in the base's
//! environment, §2.5.1).
//!
//! Infra runs carry a final `[aver] …` line in their output saying why
//! (out of memory, timeout, or the runner's error), since the command's own
//! output is often missing in those cases.

pub mod built;
pub(crate) mod cache;
pub mod cargo;
pub mod findings;
pub mod gojson;
pub mod jest;
pub mod junit;
pub mod libtest;
pub mod output;
#[cfg(feature = "scripted")]
pub mod scripted;
pub mod seam;
pub mod slots;
pub mod tap;
pub mod toolchain;
pub mod tsc;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aver_core::{
    BlobId, CheckDef, CheckRun, Environment, Network, Object, Provenance, ProvenanceMode, RunId,
    RunIndex, RunResult, Schema, Set, Snapshot, SnapshotId, TaskCheckDef, TestResult, Timestamp,
    TreeId, Uid, MAX_BLOB_BYTES,
};
use parking_lot::Mutex;
use tokio::sync::OnceCell;

use self::cache::CacheKey;
use self::output::{check_env, Exit, SandboxLimits, SandboxOutput};
use self::seam::{Sandbox, SandboxRequest};
pub use self::slots::{Slot, SlotEvent, SlotWatch, Slots};
use crate::place::proc::ProcFailure;
use crate::place::runner::{Place, SlotRequest, SlotRunner};
use crate::store::{Store, StoreError};

const REPORT_DIR: &str = "/aver/out/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecConfig {
    /// Most runs going at once.
    pub max_sandboxes: usize,
    /// Ceiling on every run's limits. `aver.toml` is repository content,
    /// so an environment's own limits are clamped to these.
    pub max_limits: SandboxLimits,
}

/// The default [`ExecConfig::max_limits`]: 4 CPUs, 8 GiB, 4096 pids, 30
/// minutes and 1 MiB of output per run.
pub const MAX_LIMITS: SandboxLimits = SandboxLimits {
    cpu_millis: 4000,
    memory_mb: 8192,
    pids: 4096,
    timeout: Duration::from_secs(30 * 60),
    output_bytes: 1024 * 1024,
};

impl Default for ExecConfig {
    /// Half the host's cores as runs, which matches the default
    /// environment limit of two CPUs per run.
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        Self {
            max_sandboxes: (cores / 2).max(1),
            max_limits: MAX_LIMITS,
        }
    }
}

/// A failure of Aver's own. A run that could not start is not an error: it
/// is stored as an `infra` run.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ExecError {
    #[error(transparent)]
    Store(Arc<StoreError>),
    #[error("execution: {0}")]
    Internal(String),
    /// A signal asked this process to stop while the run ran: nothing of
    /// it is stored.
    #[error("interrupted")]
    Interrupted,
}

impl From<StoreError> for ExecError {
    fn from(e: StoreError) -> Self {
        Self::Store(Arc::new(e))
    }
}

/// A stored run, whether it came from the cache, and the sandbox time it
/// took (zero for a cache hit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedRun {
    pub id: RunId,
    pub run: CheckRun,
    pub cache_hit: bool,
    pub timing: RunTiming,
}

/// Wall time a run took, for a consumer that tracks compute.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunTiming {
    /// Dependency work this run did itself; always zero since the Docker
    /// sandbox's layers went in 0.4.
    pub install: Duration,
    /// The check's command, from start to end.
    pub check: Duration,
}

/// What a slot run needs to be looked up, built and stored on the
/// blocking pool.
struct SlotRun {
    key: CacheKey,
    snapshot: SnapshotId,
    env: Environment,
    check: CheckDef,
    index: RunIndex,
    started_at: Timestamp,
}

/// The `CheckDef` part of a task-defined check.
pub fn task_check(def: &TaskCheckDef) -> CheckDef {
    CheckDef {
        cwd: def.cwd.clone(),
        timeout_ms: def.timeout_ms,
        report: def.report.clone(),
        ..CheckDef::new(def.id.clone(), def.argv.clone())
    }
}

type Flight = Arc<OnceCell<Result<ExecutedRun, ExecError>>>;

/// Runs and caches checks. Cheap to clone; clones share the cache, the
/// in-flight map, the concurrency limit and the sandbox-start counter.
/// A slot executor's clone may name the slot its runs use
/// ([`Executor::in_slot`]).
#[derive(Clone)]
pub struct Executor {
    inner: Arc<Inner>,
    place: Option<Place>,
    /// Whose queue this clone's runs wait in ([`slots`]): a task id for
    /// verification.
    requester: Option<Arc<str>>,
    /// Hears this clone's runs wait for and get a slot.
    watch: Option<SlotWatch>,
    /// Where this clone's runs wait in their requester's queue ([`slots`]).
    rank: u8,
}

/// What executes a check: the host (§2.4.6), or the scripted runner of
/// [`seam`] in tests.
enum Runner {
    Sandbox(Arc<dyn Sandbox>),
    Slots(Arc<SlotRunner>),
}

struct Inner {
    store: Arc<Store>,
    runner: Runner,
    slots: Arc<Slots>,
    flights: Mutex<HashMap<CacheKey, Flight>>,
    starts: AtomicU64,
    max_limits: SandboxLimits,
    /// Runs restored from a CI record (`aver ci --base-record`): cache
    /// hits that no run on this machine produced.
    recorded: Mutex<std::collections::HashSet<RunId>>,
    /// Fresh base runs that checked a `NEW` resting on recorded runs.
    confirmations: AtomicU64,
}

/// Why a run ended as `infra: sandbox`, with whatever output there was.
struct SandboxFailure {
    message: String,
    output: Vec<u8>,
}

impl Executor {
    pub fn new(store: Arc<Store>, sandbox: Arc<dyn Sandbox>, config: ExecConfig) -> Self {
        Self::with_runner(store, Runner::Sandbox(sandbox), config)
    }

    /// An executor that runs checks in slots on this machine (§2.4.6).
    /// A runner whose runs must not overlap ([`SlotRunner::serial`]) gets
    /// one at a time.
    pub fn slots(store: Arc<Store>, runner: SlotRunner, mut config: ExecConfig) -> Self {
        if runner.serial() {
            config.max_sandboxes = 1;
        }
        Self::with_runner(store, Runner::Slots(Arc::new(runner)), config)
    }

    fn with_runner(store: Arc<Store>, runner: Runner, config: ExecConfig) -> Self {
        Self {
            place: None,
            requester: None,
            watch: None,
            rank: 0,
            inner: Arc::new(Inner {
                store,
                runner,
                slots: Slots::new(config.max_sandboxes),
                flights: Mutex::new(HashMap::new()),
                starts: AtomicU64::new(0),
                max_limits: config.max_limits,
                recorded: Mutex::new(std::collections::HashSet::new()),
                confirmations: AtomicU64::new(0),
            }),
        }
    }

    /// The executor profile id recorded on runs, e.g. `native-v0.<16 hex>`.
    pub fn executor(&self) -> &str {
        match &self.inner.runner {
            Runner::Sandbox(s) => s.executor(),
            Runner::Slots(n) => n.executor(),
        }
    }

    /// Whether checks run on the host rather than through the seam.
    pub fn is_native(&self) -> bool {
        matches!(self.inner.runner, Runner::Slots(_))
    }

    /// The slot runner, for an executor on this machine.
    pub fn slot_runner(&self) -> Option<&SlotRunner> {
        match &self.inner.runner {
            Runner::Slots(n) => Some(n),
            Runner::Sandbox(_) => None,
        }
    }

    /// This executor with its runs in `place`. Shares everything else,
    /// the cache included.
    pub fn in_slot(&self, place: Place) -> Self {
        Self {
            place: Some(place),
            ..self.clone()
        }
    }

    /// This executor with its runs queued as `requester`'s (a task id):
    /// slots go round-robin between requesters ([`slots`]).
    pub fn for_requester(&self, requester: impl std::fmt::Display) -> Self {
        Self {
            requester: Some(Arc::from(requester.to_string())),
            ..self.clone()
        }
    }

    /// This executor with its runs waiting ahead of its requester's runs of
    /// a higher rank, behind those of a lower.
    pub fn ranked(&self, rank: u8) -> Self {
        Self {
            rank,
            ..self.clone()
        }
    }

    /// This executor with `watch` told when its runs wait for a slot and
    /// when they start (cache hits and joined runs do neither).
    pub fn watched(&self, watch: SlotWatch) -> Self {
        Self {
            watch: Some(watch),
            ..self.clone()
        }
    }

    async fn slot(&self) -> Slot {
        let who = self.requester.as_deref().unwrap_or(slots::ANONYMOUS);
        self.inner
            .slots
            .acquire(who, self.rank, self.watch.as_ref())
            .await
    }

    /// What a receipt records about where this executor's runs execute
    /// (§1.9.3, v3).
    pub fn provenance(&self) -> Provenance {
        match &self.inner.runner {
            Runner::Sandbox(_) => Provenance {
                mode: ProvenanceMode::Sandbox,
                host_profile: None,
                network: Network::Off,
                uid: Uid::Separate,
            },
            Runner::Slots(n) => Provenance {
                mode: n.mode(),
                host_profile: Some(n.profile().clone()),
                network: Network::On,
                uid: n.uid(),
            },
        }
    }

    /// Marks runs as restored from a CI record, so a verdict resting on
    /// them alone is confirmed by a fresh run.
    pub fn adopt_recorded(&self, runs: impl IntoIterator<Item = RunId>) {
        self.inner.recorded.lock().extend(runs);
    }

    /// Whether `run` was restored from a CI record.
    pub fn is_recorded(&self, run: &RunId) -> bool {
        self.inner.recorded.lock().contains(run)
    }

    /// Counts one fresh base run that checked a `NEW` resting on recorded
    /// runs.
    pub fn note_confirmation(&self) {
        self.inner.confirmations.fetch_add(1, Ordering::SeqCst);
    }

    /// How many fresh base runs checked a `NEW` resting on recorded runs.
    pub fn confirmations(&self) -> u64 {
        self.inner.confirmations.load(Ordering::SeqCst)
    }

    /// How many runs this executor has started.
    pub fn sandbox_starts(&self) -> u64 {
        self.inner.starts.load(Ordering::SeqCst)
    }

    /// The run cache key of a run here. A slot run's executor component
    /// also names its provenance and the dependency inputs its slot is
    /// cloned with ([`SlotRunner::key_executor`]), so a receipt never
    /// claims a mode or uid its reused runs did not have.
    fn key(
        &self,
        snapshot: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
    ) -> Result<CacheKey, ExecError> {
        let executor = match &self.inner.runner {
            Runner::Sandbox(s) => s.executor().to_owned(),
            Runner::Slots(n) => n.key_executor(self.place.as_ref(), env),
        };
        Ok(CacheKey::new(snapshot, env, check, &executor, index))
    }

    /// The cached `pass` or `fail` run for this key, if any.
    pub fn cached(
        &self,
        snapshot: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
    ) -> Result<Option<ExecutedRun>, ExecError> {
        let key = self.key(snapshot, env, check, index)?;
        self.cached_by_key(&key)
    }

    /// Runs `check` on `snapshot` under `env`, or returns the cached run.
    pub async fn run(
        &self,
        snapshot: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
    ) -> Result<ExecutedRun, ExecError> {
        self.run_from(snapshot, snapshot, env, check, index).await
    }

    /// [`Executor::run`], for verification, which names the side whose
    /// environment the check runs in as `deps`; only the cache key's
    /// callers still tell them apart.
    pub async fn run_from(
        &self,
        snapshot: &SnapshotId,
        deps: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
    ) -> Result<ExecutedRun, ExecError> {
        let key = self.key(snapshot, env, check, index)?;
        let flight = self
            .inner
            .flights
            .lock()
            .entry(key.clone())
            .or_default()
            .clone();
        let result = flight
            .get_or_init(|| self.lookup_or_execute(&key, snapshot, deps, env, check, index))
            .await
            .clone();
        let mut flights = self.inner.flights.lock();
        if flights.get(&key).is_some_and(|f| Arc::ptr_eq(f, &flight)) {
            flights.remove(&key);
        }
        result
    }

    /// Runs `check` again without consulting the cache or joining an
    /// in-flight run, to reproduce a result. The run is stored, and
    /// becomes the cached run for its key only if there was none.
    pub async fn run_fresh(
        &self,
        snapshot: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
    ) -> Result<ExecutedRun, ExecError> {
        self.run_fresh_from(snapshot, snapshot, env, check, index)
            .await
    }

    /// [`Executor::run_fresh`], for a side named as in [`Executor::run_from`].
    pub async fn run_fresh_from(
        &self,
        snapshot: &SnapshotId,
        deps: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
    ) -> Result<ExecutedRun, ExecError> {
        let key = self.key(snapshot, env, check, index)?;
        self.execute(&key, snapshot, deps, env, check, index, true)
            .await
    }

    /// Runs every check of the snapshot's own environment, concurrently, and
    /// returns the runs in check id order.
    pub async fn run_env_checks(
        &self,
        snapshot: &SnapshotId,
        index: RunIndex,
    ) -> Result<Vec<ExecutedRun>, ExecError> {
        let snap: Snapshot = self.inner.store.get(snapshot)?;
        let env: Arc<Environment> =
            Arc::new(self.inner.store.get(&crate::store::pinned_env(&snap)?)?);
        let mut tasks = tokio::task::JoinSet::new();
        for check in env.checks.iter() {
            let check = check.clone();
            let this = self.clone();
            let env = env.clone();
            let snapshot = *snapshot;
            tasks.spawn(async move { this.run(&snapshot, &env, &check, index).await });
        }
        let mut runs = Vec::with_capacity(env.checks.len());
        while let Some(joined) = tasks.join_next().await {
            runs.push(joined.map_err(|e| ExecError::Internal(e.to_string()))??);
        }
        runs.sort_by(|a, b| a.run.check.cmp(&b.run.check));
        Ok(runs)
    }

    fn cached_by_key(&self, key: &CacheKey) -> Result<Option<ExecutedRun>, ExecError> {
        let Some(id) = key.lookup(&self.inner.store)? else {
            return Ok(None);
        };
        let run = self.inner.store.get(&id)?;
        Ok(Some(ExecutedRun {
            id,
            run,
            cache_hit: true,
            timing: RunTiming::default(),
        }))
    }

    async fn lookup_or_execute(
        &self,
        key: &CacheKey,
        snapshot: &SnapshotId,
        deps: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
    ) -> Result<ExecutedRun, ExecError> {
        if let Some(hit) = self.cached_by_key(key)? {
            return Ok(hit);
        }
        self.execute(key, snapshot, deps, env, check, index, false)
            .await
    }

    /// Runs `check` and stores the run. Unless `fresh`, a run another
    /// process stored for `key` while this one waited for the slot is
    /// returned instead: the in-process flight cannot see other processes.
    /// An interrupted run stores nothing, not even `infra`.
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        key: &CacheKey,
        snapshot: &SnapshotId,
        _deps: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
        fresh: bool,
    ) -> Result<ExecutedRun, ExecError> {
        let snap: Snapshot = self.inner.store.get(snapshot)?;
        // The run's timeout starts in the runner, once it has its slot.
        let _slot = self.slot().await;
        let started_at = Timestamp::now();
        if let Runner::Slots(runner) = &self.inner.runner {
            let place = self
                .place
                .clone()
                .unwrap_or_else(|| runner.place_of(snapshot));
            let run = SlotRun {
                key: key.clone(),
                snapshot: *snapshot,
                env: env.clone(),
                check: check.clone(),
                index,
                started_at,
            };
            return self.in_place(runner, place, snap.root, run, fresh).await;
        }
        let mut timing = RunTiming::default();
        let sandboxed = self
            .sandboxed(&snap, snapshot, env, check, index, &mut timing)
            .await?;
        let run = self.build_run(snapshot, env, check, index, started_at, sandboxed);
        let mut executed = self.store_run(key, env, run)?;
        executed.timing = timing;
        Ok(executed)
    }

    /// The limits a check runs with: the environment's, with the check's own
    /// timeout, clamped to the executor's ceiling.
    fn limits(&self, env: &Environment, check: &CheckDef) -> SandboxLimits {
        let timeout_ms = check.timeout_ms.unwrap_or(env.limits.timeout_ms);
        SandboxLimits::of(&env.limits, timeout_ms).clamped(&self.inner.max_limits)
    }

    /// One run in this executor's slot (when none was named, a slot of
    /// the snapshot's own: only a verification's base side, or a warm-up
    /// of a task's base, runs in a base slot), on the blocking pool. The
    /// run is looked up and stored under the slot's lock, so a process
    /// waiting for the slot finds this run instead of running it again.
    async fn in_place(
        &self,
        runner: &Arc<SlotRunner>,
        place: Place,
        root: TreeId,
        run: SlotRun,
        fresh: bool,
    ) -> Result<ExecutedRun, ExecError> {
        let (this, runner) = (self.clone(), runner.clone());
        let limits = self.limits(&run.env, &run.check);
        self.inner.starts.fetch_add(1, Ordering::SeqCst);
        let started = std::time::Instant::now();
        tokio::task::spawn_blocking(move || {
            let req = SlotRequest {
                root,
                env: &run.env,
                check: &run.check,
                limits,
                place: &place,
            };
            runner.run_cached(
                req,
                || (!fresh).then(|| this.cached_by_key(&run.key).transpose())?,
                |result| {
                    let sandboxed = match result {
                        Ok(output) => Ok(output),
                        Err(ProcFailure::Failed { message, output }) => {
                            Err(SandboxFailure { message, output })
                        }
                        Err(ProcFailure::Interrupted) => return Err(ExecError::Interrupted),
                    };
                    let check = started.elapsed();
                    let pending = this.build_run(
                        &run.snapshot,
                        &run.env,
                        &run.check,
                        run.index,
                        run.started_at,
                        sandboxed,
                    );
                    let mut executed = this.store_run(&run.key, &run.env, pending)?;
                    executed.timing = RunTiming {
                        check,
                        ..RunTiming::default()
                    };
                    Ok(executed)
                },
            )
        })
        .await
        .map_err(|e| ExecError::Internal(e.to_string()))?
    }

    async fn sandboxed(
        &self,
        snap: &Snapshot,
        snapshot: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
        timing: &mut RunTiming,
    ) -> Result<Result<SandboxOutput, SandboxFailure>, ExecError> {
        let inner = &self.inner;
        let Runner::Sandbox(sandbox) = &inner.runner else {
            return Err(ExecError::Internal(
                "a native executor has no scripted runner".into(),
            ));
        };
        let input = {
            let store = inner.store.clone();
            let root = snap.root;
            tokio::task::spawn_blocking(move || seam::tree_input(&store, &root))
                .await
                .map_err(|e| ExecError::Internal(e.to_string()))??
        };
        let request = SandboxRequest {
            label: format!("{snapshot}:{}:{}", check.id, index.get()),
            argv: check.argv.clone(),
            cwd: check.cwd.clone(),
            env: check_env(env),
            limits: self.limits(env, check),
            input: Arc::new(input),
            setup: check.setup.clone(),
        };
        inner.starts.fetch_add(1, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let result = sandbox.run(request).await;
        timing.check = started.elapsed();
        Ok(result.map_err(|e| SandboxFailure {
            message: e.to_string(),
            output: Vec::new(),
        }))
    }

    /// The run result, output (with an `[aver]` line for infra outcomes),
    /// result files and duration of a finished run.
    fn conclude(
        &self,
        env: &Environment,
        check: &CheckDef,
        sandboxed: Result<SandboxOutput, SandboxFailure>,
    ) -> Concluded {
        match sandboxed {
            // A run that failed without its report because a write found the
            // disk full lost its result to the host, not to the change. With
            // the report there, its tests' results stand, whatever the output
            // says (a test may be named after ENOSPC).
            Ok(out)
                if out.exit != Exit::Exited(0)
                    && output::says_disk_full(&out.output)
                    && check.report.as_ref().is_none_or(|d| {
                        !out.files
                            .contains_key(d.path.as_str().trim_start_matches(REPORT_DIR))
                    }) =>
            {
                let mut output = out.output;
                note(
                    &mut output,
                    &format!(
                        "{}: the run's output says a write failed for lack of space; free some and verify again",
                        output::DISK_FULL
                    ),
                );
                Concluded::infra(output)
            }
            Ok(out) => {
                let mut output = out.output;
                let result = match out.exit {
                    Exit::Exited(0) => RunResult::Pass { exit_code: 0 },
                    Exit::Exited(exit_code) => RunResult::Fail { exit_code },
                    Exit::Oom(exit_code) => {
                        note(
                            &mut output,
                            &format!(
                                "killed: out of memory (limit {} MiB)",
                                self.limits(env, check).memory_mb
                            ),
                        );
                        RunResult::Oom { exit_code }
                    }
                    Exit::Timeout(exit_code) => {
                        let ms = self.limits(env, check).timeout.as_millis();
                        note(
                            &mut output,
                            &format!("killed: exceeded the {ms} ms timeout"),
                        );
                        RunResult::Timeout { exit_code }
                    }
                };
                Concluded {
                    result,
                    output,
                    files: out.files,
                    duration: out.duration,
                    built: out.built,
                }
            }
            Err(failure) => {
                let mut output = failure.output;
                if failure.message.starts_with(output::DISK_FULL) {
                    note(&mut output, &failure.message);
                } else {
                    note(
                        &mut output,
                        &format!("could not run the check: {}", failure.message),
                    );
                }
                Concluded::infra(output)
            }
        }
    }

    fn build_run(
        &self,
        snapshot: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        index: RunIndex,
        started_at: Timestamp,
        sandboxed: Result<SandboxOutput, SandboxFailure>,
    ) -> PendingRun {
        let Concluded {
            mut result,
            mut output,
            files,
            duration,
            built,
        } = self.conclude(env, check, sandboxed);
        let (report, tests) = match check.report.as_ref() {
            Some(declared) => {
                let rel = declared.path.as_str().trim_start_matches(REPORT_DIR);
                match files.get(rel) {
                    // Aver's limit lost the result, not the code under test:
                    // the run ends in infra (`ENV`), never as the change's.
                    Some(xml) if xml.len() > MAX_BLOB_BYTES => {
                        note(
                            &mut output,
                            &format!(
                                "report {} is {} bytes, over Aver's limit of {MAX_BLOB_BYTES}, so Aver dropped it: the run has no result",
                                declared.path,
                                xml.len()
                            ),
                        );
                        result = RunResult::Sandbox;
                        (None, Set::empty())
                    }
                    Some(xml) => match junit::parse_as(&check.id, xml, declared.format) {
                        Ok((tests, _)) => (Some(xml.clone()), tests),
                        Err(e) => {
                            let e = toolchain_error(e, &output);
                            note(&mut output, &format!("report {}: {e}", declared.path));
                            (None, Set::empty())
                        }
                    },
                    None => {
                        if let Some(t) = toolchain::classify(&String::from_utf8_lossy(&output)) {
                            note(
                                &mut output,
                                &format!(
                                    "report {}: {}",
                                    declared.path,
                                    junit::JunitError::Toolchain(t.to_string())
                                ),
                            );
                        }
                        (None, Set::empty())
                    }
                }
            }
            None => (None, Set::empty()),
        };
        PendingRun {
            snapshot: *snapshot,
            check: check.clone(),
            index,
            executor: self.executor().to_owned(),
            result,
            duration,
            output,
            report,
            tests,
            started_at,
            built,
        }
    }

    fn store_run(
        &self,
        key: &CacheKey,
        env: &Environment,
        pending: PendingRun,
    ) -> Result<ExecutedRun, ExecError> {
        let (id, run) = self.inner.store.tx(|tx| {
            let env_id = tx.put(env)?;
            let output = tx.put_blob(&pending.output)?;
            let report = pending
                .report
                .as_deref()
                .map(|xml| tx.put_blob(xml))
                .transpose()?;
            let run = CheckRun {
                schema: Schema::new(),
                snapshot: pending.snapshot,
                env: env_id,
                check: pending.check.id.clone(),
                index: pending.index,
                executor: pending.executor.clone(),
                result: pending.result,
                duration_ms: u32::try_from(pending.duration.as_millis()).unwrap_or(u32::MAX),
                output,
                report,
                tests: pending.tests.clone(),
                started_at: pending.started_at,
            };
            let id = tx.put(&run)?;
            if run.result.outcome() != aver_core::Outcome::Infra {
                key.insert(tx, &id)?;
            }
            if let Some(built) = &pending.built {
                built::insert(tx, &id, built)?;
            }
            Ok::<_, StoreError>((id, run))
        })?;
        debug_assert_eq!(id, run.id());
        Ok(ExecutedRun {
            id,
            run,
            cache_hit: false,
            timing: RunTiming::default(),
        })
    }
}

/// What [`Executor::conclude`] read from a finished run.
struct Concluded {
    result: RunResult,
    output: Vec<u8>,
    files: BTreeMap<String, Vec<u8>>,
    duration: Duration,
    built: Option<BTreeMap<String, BlobId>>,
}

impl Concluded {
    /// A run with no result: nothing it left counts.
    fn infra(output: Vec<u8>) -> Self {
        Self {
            result: RunResult::Sandbox,
            output,
            files: BTreeMap::new(),
            duration: Duration::ZERO,
            built: None,
        }
    }
}

struct PendingRun {
    snapshot: SnapshotId,
    check: CheckDef,
    index: RunIndex,
    executor: String,
    result: RunResult,
    duration: Duration,
    output: Vec<u8>,
    report: Option<Vec<u8>>,
    tests: Set<TestResult>,
    started_at: Timestamp,
    built: Option<BTreeMap<String, BlobId>>,
}

/// A report that could not be read, as the toolchain error the run's
/// output names when the parser saw only a malformed report.
pub fn toolchain_error(e: junit::JunitError, output: &[u8]) -> junit::JunitError {
    match e {
        junit::JunitError::Json(_) | junit::JunitError::Text(_) | junit::JunitError::Xml(_) => {
            match toolchain::classify(&String::from_utf8_lossy(output)) {
                Some(t) => junit::JunitError::Toolchain(t.to_string()),
                None => e,
            }
        }
        e => e,
    }
}

fn note(output: &mut Vec<u8>, line: &str) {
    if !output.is_empty() && !output.ends_with(b"\n") {
        output.push(b'\n');
    }
    output.extend_from_slice(format!("[aver] {line}\n").as_bytes());
}
