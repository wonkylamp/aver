//! CI mode (ARCHITECTURE.md §2.9): verify one Git commit against another in
//! a single process, the way a pull request check does.
//!
//! Both commits are imported through the Git bridge, a task is signed by the
//! home's admin key with the criteria and protected paths the pull request
//! declares (none is a regression-only `aver.task/v2`), and the head is
//! verified against the base. No ref moves: the task has no target. The
//! receipt records the commits, the configuration, the criteria and the
//! caller's subject in its `ci` object, and its `reproduce` line is an
//! `aver ci` command a third party can run (§2.9).

pub mod approve;
pub mod record;
pub mod spec;
pub mod summary;
pub mod workflow;

use std::sync::Arc;

use aver_core::task::{MAX_INTENT_BYTES, MAX_TITLE_BYTES};
use aver_core::{
    AgentId, CheckDef, CiInfo, GitSha, Receipt, ReceiptId, SigningKey, Snapshot, SnapshotId,
    Subject, Task, TaskBody, TaskId, TaskSchema, Text, Timestamp, Verdict,
};
use serde::Deserialize;

pub use spec::{Spec, SpecError};

use crate::diagnostics::{Diagnostics, ImportNotes, PlaceholderNote, SecretNote, MAX_LISTING};
use crate::env::{Detection, ImageResolver, TreeEnv, CONFIG_PATH};
use crate::exec::Executor;
use crate::git::{
    self, BaseResolution, ImportError, ImportOptions, OversizePolicy, Placeholder, SecretPolicy,
};
use crate::keys::AgentKey;
use crate::secrets::SecretFinding;
use crate::store::Store;
use crate::tasks::{TaskError, Tasks};
use crate::verify::{self, CiRequest, Head, Verifier, VerifyError};
use crate::workspace::Workspaces;

/// `aver ci`'s exit code for a verdict that fails `--fail-on`.
pub const EXIT_FAIL: u8 = 1;
/// `aver ci`'s exit code, under `--fail-on unexplained`, for `REVIEW` and
/// `UNPROVEN`: nothing broke, nothing proved the change safe, a maintainer
/// decides.
pub const EXIT_NEEDS_MAINTAINER: u8 = 3;

/// Which verdicts fail the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FailOn {
    /// `FAIL_OWN`, and a `BLOCKED_NOT_OWN` the base does not explain (the
    /// default since round 13a); `REVIEW` and `UNPROVEN` exit 3.
    Unexplained,
    /// Only `FAIL_OWN`: the change broke something.
    FailOwn,
    /// Anything but `PASS`.
    NotPass,
    Never,
    /// What `unexplained` fails, and nothing else: `REVIEW` and `UNPROVEN`
    /// exit 0 (`--regression-only`, a merge queue's check, whose pull
    /// request's own check already asked for the review).
    Regressions,
}

impl FailOn {
    /// Whether a receipt fails the run. Under `unexplained`, a
    /// `BLOCKED_NOT_OWN` fails it unless a red base explains the block
    /// ([`blocked_by_the_base`]); the other modes look at the verdict only.
    pub fn fails_receipt(self, body: &aver_core::ReceiptBody) -> bool {
        match self {
            Self::Unexplained | Self::Regressions => {
                body.verdict == Verdict::FailOwn
                    || (body.verdict == Verdict::BlockedNotOwn && !blocked_by_the_base(body))
            }
            _ => self.fails(body.verdict),
        }
    }

    /// The exit code of `aver ci` for a receipt: 1 when it fails the run
    /// ([`FailOn::fails_receipt`]); under `unexplained`, 3 for `REVIEW` and
    /// `UNPROVEN`, which no regression explains but which nothing proved
    /// either (a changed test expectation, a change no check covers), so a
    /// maintainer decides; else 0.
    pub fn exit_code(self, body: &aver_core::ReceiptBody) -> u8 {
        if self.fails_receipt(body) {
            EXIT_FAIL
        } else if self == Self::Unexplained
            && matches!(body.verdict, Verdict::Review | Verdict::Unproven)
        {
            EXIT_NEEDS_MAINTAINER
        } else {
            0
        }
    }

    /// Whether a verdict alone fails the run; under `unexplained` a
    /// `BLOCKED_NOT_OWN` needs its receipt ([`FailOn::fails_receipt`]) and
    /// counts as explained here.
    pub fn fails(self, verdict: Verdict) -> bool {
        match self {
            Self::Unexplained | Self::Regressions | Self::FailOwn => verdict == Verdict::FailOwn,
            Self::NotPass => verdict != Verdict::Pass,
            Self::Never => false,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unexplained => "unexplained",
            Self::FailOwn => "fail-own",
            Self::NotPass => "not-pass",
            Self::Never => "never",
            Self::Regressions => "regressions",
        }
    }
}

/// Whether a `BLOCKED_NOT_OWN` receipt is held by the base: it has a
/// blocking entry, and every one of them (an `ENV` entry, an
/// inconclusive entry, a `FLAKY` one) fails or is flaky on the base and is
/// not `ENV`, so checks that already failed there hide what the change did.
/// One blocker the base does not explain fails the run: a red check beside
/// a check that could not run explains nothing about the second.
///
/// An `ENV` entry never is explained, whatever its base status says. A
/// head whose runs ended in infra over a base that completed red is not
/// explained by the base's failing tests: a change that provoked an infra
/// failure would exit 0 over any red base. A base whose own runs ended in
/// infra (a missing tool, a dependency layer that cannot install, a
/// setup command that failed) ran no test, so a repository whose checks
/// cannot run would pass every pull request without a test run. Either
/// way nothing showed the change is safe.
///
/// Nor is a check read by its exit code alone (an entry with no test name)
/// that fails on the base: no test result says what the base ran, so it
/// cannot tell a broken main from a runner that cannot run the suite (WP6:
/// ky's `npm test` failed on both sides for want of Playwright's browsers,
/// and its regression exited 0).
pub fn blocked_by_the_base(body: &aver_core::ReceiptBody) -> bool {
    use aver_core::{BaseStatus, Cause};
    let rule = body.schema.proof_rule();
    let blockers: Vec<_> = body
        .checks
        .iter()
        .filter(|e| {
            matches!(e.cause, Some(Cause::Env | Cause::Flaky))
                || aver_core::rules::inconclusive(e, rule)
        })
        .collect();
    !blockers.is_empty()
        && blockers.iter().all(|e| {
            e.cause != Some(Cause::Env)
                && e.id.as_str().contains("::")
                && matches!(e.base, BaseStatus::Fail | BaseStatus::Flaky)
        })
}

/// The parts of a GitHub `pull_request` event CI mode reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PullRequest {
    pub number: Option<u64>,
    pub title: Option<String>,
    /// `null` when the description is empty.
    pub body: Option<String>,
    pub html_url: Option<String>,
}

/// The pull request in a GitHub event payload, if the event has one.
pub fn pull_request(event: &[u8]) -> Result<Option<PullRequest>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Event {
        pull_request: Option<PullRequest>,
    }
    Ok(serde_json::from_slice::<Event>(event)?.pull_request)
}

pub struct Request {
    /// A local repository path or URL.
    pub repo: String,
    pub base: String,
    pub head: String,
    pub spec: Spec,
    pub title: Text<MAX_TITLE_BYTES>,
    pub intent: Text<MAX_INTENT_BYTES>,
    /// The consumer's label, recorded in the receipt.
    pub subject: Option<Subject>,
    /// Refuse a base without `aver.toml` instead of using the default
    /// environment.
    pub require_config: bool,
    /// Refuse an import with a possible secret instead of reporting it.
    pub strict_secrets: bool,
    /// Ignore cached runs: every run starts a sandbox.
    pub fresh: bool,
    /// Run every check, also those the change does not affect.
    pub all: bool,
    /// Run only this check (`--check`): the receipt covers it alone.
    pub only: Option<aver_core::CheckId>,
    /// Restore the base's runs from this record before verifying.
    pub base_record: Option<std::path::PathBuf>,
    /// Record the head's runs here after the verdict.
    pub record: Option<std::path::PathBuf>,
    /// The home's records directory ([`record::home_dir`]): the base's
    /// record is read from its `<key>` directory unless `fresh`, and the
    /// base runs of this verification are written there after it.
    pub home_records: Option<std::path::PathBuf>,
    /// The workflow file whose blob keys records, instead of the one
    /// `GITHUB_WORKFLOW_REF` names.
    pub workflow: Option<String>,
}

impl Request {
    /// A request titled after the pull request when there is one, else
    /// after the two revisions.
    pub fn new(
        repo: String,
        base: String,
        head: String,
        spec: Spec,
        pr: Option<&PullRequest>,
    ) -> Self {
        let title = match pr.and_then(|p| p.title.as_deref()) {
            Some(title) => title.to_owned(),
            None => format!("aver ci {base}..{head}"),
        };
        let intent = match pr {
            Some(p) => format!(
                "Verify pull request{}{} ({head} against {base}).",
                p.number.map(|n| format!(" #{n}")).unwrap_or_default(),
                p.html_url
                    .as_deref()
                    .map(|u| format!(" {u}"))
                    .unwrap_or_default(),
            ),
            None => format!("Verify {head} against {base}."),
        };
        Self {
            title: Text::truncated(&title),
            intent: Text::truncated(&intent),
            repo,
            base,
            head,
            spec,
            subject: None,
            require_config: false,
            strict_secrets: false,
            fresh: false,
            all: false,
            only: None,
            base_record: None,
            record: None,
            home_records: None,
            workflow: None,
        }
    }
}

/// One imported commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub commit: GitSha,
    pub snapshot: SnapshotId,
    /// The commit message's `Aver-Why:` trailers ([`why_trailer`]).
    pub why: Option<String>,
    /// Possible secrets the import reported instead of refusing.
    pub secrets: Vec<SecretFinding>,
    /// Files over the blob limit imported empty.
    pub placeholders: Vec<Placeholder>,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub task_id: TaskId,
    pub task: Task,
    pub base: Imported,
    pub head: Imported,
    pub receipt_id: ReceiptId,
    pub receipt: Receipt,
    pub diagnostics: Diagnostics,
    /// Every run the verification used, with its sandbox time.
    pub runs: Vec<verify::TimedRun>,
    /// What the two imports let through with a warning.
    pub import: ImportNotes,
    /// The base `--base` named and the one the run compared with.
    pub resolution: BaseResolution,
    /// The base environment's checks, which every run used.
    pub checks: Vec<CheckDef>,
    /// Why a default `npm test` check is read by its exit code alone.
    pub exit_only: std::collections::BTreeMap<aver_core::CheckId, String>,
    /// The diff of each file behind a `CHANGED` entry, for review.
    pub review: Vec<ReviewDiff>,
    /// The output of every run the verification used.
    pub logs: Vec<RunLog>,
    /// The one check `--check` selected; the receipt covers it alone.
    pub only: Option<aver_core::CheckId>,
    /// What `--base-record` restored and how much of the base it served.
    pub base_record: Option<BaseRecord>,
    /// How many runs `--record` wrote.
    pub recorded: Option<usize>,
    /// Where the home's record of the base was written, and how many runs
    /// it holds.
    pub home_recorded: Option<(std::path::PathBuf, usize)>,
    /// Where the checks ran.
    pub place: CiPlace,
}

/// The parts of a [`Request`] that records use.
struct Records {
    repo: String,
    workflow: Option<String>,
    base_record: Option<std::path::PathBuf>,
    record: Option<std::path::PathBuf>,
    home: Option<std::path::PathBuf>,
}

/// Where a restored base record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordSource {
    /// `--base-record DIR`.
    Flag,
    /// The home's records, written by an earlier local `aver ci`.
    Home,
}

/// A restored base record and the base runs it served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseRecord {
    pub source: RecordSource,
    pub restored: record::Restored,
    /// Base-side runs the verdict read from the record.
    pub served: usize,
    /// Base-side runs this job started anyway: checks the record did not
    /// hold, or a record a fresh run contradicted.
    pub started: usize,
    /// Fresh base runs that checked a `NEW` resting on the record alone.
    pub confirmed: u64,
}

/// Where `aver ci` ran the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiPlace {
    /// In the job's checkout, swapped between the trees and restored.
    InPlace,
    /// In slots cloned from it; `why` when in place was asked for and the
    /// checkout could not be adopted.
    Slots { why: Option<String> },
}

/// The change to one test or harness file that a `CHANGED` entry stands on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDiff {
    pub path: String,
    /// A unified diff of the base's and the head's file, cut to
    /// [`MAX_REVIEW_DIFF`] bytes; repository content.
    pub diff: String,
}

/// One run's bounded output, named `<check>.<side>.<n>.log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLog {
    pub name: String,
    pub output: Vec<u8>,
}

/// Bytes of one file's diff that the review section shows.
pub const MAX_REVIEW_DIFF: usize = 4000;
/// Files whose diffs the review section shows.
pub const MAX_REVIEW_FILES: usize = 10;

#[derive(Debug, thiserror::Error)]
pub enum CiError {
    #[error("importing the {side} revision {rev:?}: {source}")]
    Import {
        side: &'static str,
        rev: String,
        source: ImportError,
    },
    #[error(transparent)]
    Task(#[from] TaskError),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error("--require-config: the base commit {0} has no aver.toml; add one, or drop --require-config to verify with the default environment (one `npm test` check)")]
    NoConfig(GitSha),
    #[error("ci: {0}")]
    Internal(String),
    #[error("ci record: {0}")]
    Record(#[from] record::RecordError),
}

pub struct Ci {
    store: Arc<Store>,
    server_key: Arc<SigningKey>,
    admin: AgentId,
    admin_key: SigningKey,
    resolver: Arc<dyn ImageResolver>,
    tasks: Tasks,
    verifier: Verifier,
}

impl Ci {
    pub fn new(
        store: Arc<Store>,
        server_key: Arc<SigningKey>,
        admin: &AgentKey,
        resolver: Arc<dyn ImageResolver>,
        executor: Executor,
    ) -> Self {
        let workspaces = Arc::new(Workspaces::new(
            store.clone(),
            server_key.clone(),
            Arc::new(TreeEnv(resolver.clone(), Detection::Presets)),
        ));
        Self {
            tasks: Tasks::new(store.clone(), workspaces.clone(), server_key.clone()),
            verifier: Verifier::new(store.clone(), workspaces, executor, server_key.clone()),
            store,
            server_key,
            admin: admin.id(),
            admin_key: admin.signing_key.clone(),
            resolver,
        }
    }

    /// Imports both revisions, signs the task and verifies head against base.
    pub async fn run(&self, req: Request) -> Result<Outcome, CiError> {
        let secrets = if req.strict_secrets {
            SecretPolicy::Reject
        } else {
            SecretPolicy::Warn
        };
        let resolution = self.resolve_base(&req).await?;
        let base = self
            .import(&req.repo, resolution.effective.as_str(), "base", secrets)
            .await?;
        let config_path = {
            let store = self.store.clone();
            let base = base.snapshot;
            tokio::task::spawn_blocking(move || -> Result<_, CiError> {
                let snapshot: Snapshot = store
                    .get(&base)
                    .map_err(|e| CiError::Internal(e.to_string()))?;
                let path: aver_core::Path = CONFIG_PATH.parse().expect("a valid path");
                let found = crate::env::lookup(&*store, &snapshot.root, &path)
                    .map_err(|e| CiError::Internal(e.to_string()))?;
                Ok(found.is_some().then_some(path))
            })
            .await
            .map_err(|e| CiError::Internal(e.to_string()))??
        };
        if req.require_config && config_path.is_none() {
            return Err(CiError::NoConfig(base.commit));
        }
        let head = self
            .import(&req.repo, resolution.head.as_str(), "head", secrets)
            .await?;
        let records = Records {
            repo: req.repo.clone(),
            workflow: req.workflow.clone(),
            base_record: req.base_record.clone(),
            record: req.record.clone(),
            home: req.home_records.clone(),
        };
        let ci = CiRequest {
            reproduce: {
                let mut command = reproduce(&req.repo, &base.commit, &head.commit, &req.spec);
                if req.all {
                    command.push_str(" --all");
                }
                if let Some(only) = &req.only {
                    command.push_str(&format!(" --check {}", shell_quote(only.as_str())));
                }
                command
            },
            only: req.only.clone(),
            info: CiInfo {
                base_commit: base.commit.clone(),
                head_commit: head.commit.clone(),
                config_path,
                criteria_text: req.spec.normalized(),
                subject: req.subject,
                requested_base: resolution.moved().then(|| resolution.requested.clone()),
            },
        };
        let body = TaskBody {
            schema: TaskSchema::for_task(&req.spec.criteria, &req.spec.protected_paths),
            title: req.title,
            intent: req.intent,
            base: base.snapshot,
            target: None,
            criteria: req.spec.criteria,
            checks: aver_core::Set::empty(),
            protected_paths: req.spec.protected_paths,
            author: self.admin,
            created_at: Timestamp::now(),
        };
        let task = Task::sign(body, &self.admin_key);
        let task_id = self.tasks.create(&task)?;
        let home = match &records.home {
            Some(root) => self.home_record(&records, root, &base, &head),
            None => None,
        };
        let restored = match (&records.base_record, &home) {
            (Some(dir), _) => {
                let expected = self.base_identity(&records, &base, &head)?;
                let label = format!("--base-record {}", dir.display());
                let restored = self.restore(dir, expected.0, &base, &label).await?;
                Some((RecordSource::Flag, restored))
            }
            (None, Some((dir, identity, _)))
                if !req.fresh && dir.join(record::RECORD_FILE).is_file() =>
            {
                let label = format!("the base record in {}", dir.display());
                match self.restore(dir, identity.clone(), &base, &label).await {
                    Ok(restored) => Some((RecordSource::Home, restored)),
                    Err(e) => {
                        eprintln!("aver: {label}: not read, {e}; the base runs here");
                        None
                    }
                }
            }
            _ => None,
        };
        let place = self.adopt(&base, &head).await?;
        // Natively, the base's runs all end before any of the head's code
        // runs, so the head cannot shape them (§2.9).
        let mut warmed = Vec::new();
        if self.verifier.executor().is_native() && !req.fresh {
            warmed = self
                .verifier
                .warm_only(self.admin, task_id, req.only.clone())
                .await?;
        }
        let verified = self
            .verifier
            .verify(
                self.admin,
                verify::Request {
                    task: task_id,
                    head: Head::Snapshot {
                        head: head.snapshot,
                        base: Some(base.snapshot),
                    },
                    attempt: None,
                    fresh: req.fresh,
                    all: req.all,
                    progress: None,
                    ci: Some(ci),
                    adhoc: None,
                    reason: head.why.clone(),
                },
            )
            .await?;
        let mut verified = verified;
        // The warm's runs are the base's runs this job started: listed as
        // started, with their time, where the verification first read them.
        for warm in &warmed {
            if let Some(r) = verified.runs.iter_mut().find(|r| r.run == warm.id) {
                r.cache_hit = false;
                r.install = warm.timing.install;
                r.sandbox = warm.timing.check;
            }
        }
        let (checks, exit_only, review, logs) = {
            let store = self.store.clone();
            let (base, head) = (base.snapshot, head.snapshot);
            let receipt = verified.receipt.body.clone();
            let diagnostics = verified.diagnostics.clone();
            let runs = verified.runs.clone();
            tokio::task::spawn_blocking(move || -> Result<_, CiError> {
                let internal = |e: crate::store::StoreError| CiError::Internal(e.to_string());
                let base: Snapshot = store.get(&base).map_err(internal)?;
                let head: Snapshot = store.get(&head).map_err(internal)?;
                let env: aver_core::Environment = store
                    .get(&crate::store::pinned_env(&base).map_err(internal)?)
                    .map_err(internal)?;
                let review =
                    review_diffs(&store, &base, &head, &receipt, &diagnostics).map_err(internal)?;
                let logs = run_logs(&store, &runs).map_err(internal)?;
                let mut exit_only = std::collections::BTreeMap::new();
                for check in env
                    .checks
                    .iter()
                    .filter(|c| c.report.is_none() && c.argv == crate::env::DEFAULT_CHECK_ARGV)
                {
                    let read = |name: &str| -> Result<Option<Vec<u8>>, CiError> {
                        let path = match &check.cwd {
                            Some(dir) => format!("{}/{name}", dir.as_str()),
                            None => name.to_owned(),
                        };
                        match path.parse() {
                            Ok(path) => {
                                crate::env::read_file(&*store, &base.root, &path).map_err(internal)
                            }
                            Err(_) => Ok(None),
                        }
                    };
                    let (manifest, lock) = (read("package.json")?, read("package-lock.json")?);
                    let root_install = match (&check.cwd, &lock) {
                        (Some(_), None) => Some(
                            crate::presets::workspace_root_install(&*store, &base.root)
                                .map_err(internal)?,
                        ),
                        _ => None,
                    };
                    if let Some(why) = crate::presets::exit_only(
                        manifest.as_deref(),
                        lock.as_deref(),
                        root_install,
                    ) {
                        exit_only.insert(check.id.clone(), why);
                    }
                }
                Ok((
                    env.checks.iter().cloned().collect(),
                    exit_only,
                    review,
                    logs,
                ))
            })
            .await
            .map_err(|e| CiError::Internal(e.to_string()))??
        };
        let recorded = match &records.record {
            Some(dir) => Some(self.save_record(&records, dir, &head).await?),
            None => None,
        };
        let home_recorded = match home {
            Some((dir, identity, runner)) => {
                let (store, at, snapshot) = (self.store.clone(), dir.clone(), base.snapshot);
                let written = tokio::task::spawn_blocking(move || {
                    record::write(&store, &at, &snapshot, identity, runner)
                })
                .await
                .map_err(|e| CiError::Internal(e.to_string()))?;
                match written {
                    Ok(n) => Some((dir, n)),
                    Err(e) => {
                        eprintln!(
                            "aver: the base record in {}: not written, {e}",
                            dir.display()
                        );
                        None
                    }
                }
            }
            None => None,
        };
        let base_record = restored.map(|(source, restored)| {
            let base = verified
                .runs
                .iter()
                .filter(|r| r.side == verify::Side::Base && !r.confirmation);
            let executor = self.verifier.executor();
            BaseRecord {
                source,
                served: base
                    .clone()
                    .filter(|r| executor.is_recorded(&r.run))
                    .count(),
                started: warmed.len()
                    + base
                        .filter(|r| !r.cache_hit && !warmed.iter().any(|w| w.id == r.run))
                        .count(),
                confirmed: executor.confirmations(),
                restored,
            }
        });
        Ok(Outcome {
            task_id,
            task,
            receipt_id: verified.id,
            receipt: verified.receipt,
            diagnostics: verified.diagnostics,
            runs: verified.runs,
            import: import_notes(&base, &head),
            base,
            head,
            resolution,
            checks,
            exit_only,
            review,
            logs,
            only: req.only,
            base_record,
            recorded,
            home_recorded,
            place,
        })
    }

    /// Hands the checkout to the runner at the head (ARCHITECTURE.md §2.9),
    /// keeping what either side's environment installs, when the runner was
    /// set up to run in place.
    async fn adopt(&self, base: &Imported, head: &Imported) -> Result<CiPlace, CiError> {
        let Some(runner) = self.verifier.executor().slot_runner() else {
            return Ok(CiPlace::Slots { why: None });
        };
        if !runner.in_place() {
            return Ok(CiPlace::Slots { why: None });
        }
        let store = self.store.clone();
        let (base, head) = (base.snapshot, head.clone());
        let executor = self.verifier.executor().clone();
        tokio::task::spawn_blocking(move || -> Result<CiPlace, CiError> {
            let internal = |e: crate::store::StoreError| CiError::Internal(e.to_string());
            let deps = |snapshot: &SnapshotId| -> Result<_, CiError> {
                let snap: Snapshot = store.get(snapshot).map_err(internal)?;
                let env: aver_core::Environment = store
                    .get(&crate::store::pinned_env(&snap).map_err(internal)?)
                    .map_err(internal)?;
                Ok((snap.root, crate::place::sides::deps_of(&env)))
            };
            let (base_tree, base_deps) = deps(&base)?;
            let (tree, head_deps) = deps(&head.snapshot)?;
            let mut keep = base_deps.union(&head_deps);
            // Only the base's own pyproject names what is kept: a head that
            // could name any untracked path could keep a file of its own
            // for the base's runs too.
            let pyproject = "pyproject.toml".parse().expect("a valid path");
            if let Some(bytes) =
                crate::env::read_file(&*store, &base_tree, &pyproject).map_err(internal)?
            {
                for file in crate::place::sides::version_files(&String::from_utf8_lossy(&bytes)) {
                    if !keep.keep.contains(&file) {
                        keep.keep.push(file);
                    }
                }
            }
            let runner = executor.slot_runner().expect("a slot executor");
            Ok(
                match runner.adopt((head.commit.clone(), tree), base_tree, keep) {
                    Ok(()) => CiPlace::InPlace,
                    Err(why) => CiPlace::Slots { why: Some(why) },
                },
            )
        })
        .await
        .map_err(|e| CiError::Internal(e.to_string()))?
    }

    /// The runner this job keys records under, with the workflow file's
    /// blob in the head commit.
    fn runner_key(&self, req: &Records, head: &Imported) -> Result<record::RunnerKey, CiError> {
        let workflow = record::workflow_blob(
            std::path::Path::new(&req.repo),
            head.commit.as_str(),
            req.workflow.as_deref(),
        )?;
        Ok(record::RunnerKey::here(
            self.verifier.executor().executor(),
            workflow,
        ))
    }

    /// The identity of the base's runs in this job, and the runner key.
    fn base_identity(
        &self,
        req: &Records,
        base: &Imported,
        head: &Imported,
    ) -> Result<(record::Identity, record::RunnerKey), CiError> {
        let runner = self.runner_key(req, head)?;
        let repo = std::path::Path::new(&req.repo);
        let identity = record::identity(repo, base.commit.as_str(), head.commit.as_str(), &runner)?;
        Ok((identity, runner))
    }

    /// The home's record directory of this base under `root`, with what
    /// keys it; `None`, said on stderr, when the key cannot be computed.
    fn home_record(
        &self,
        req: &Records,
        root: &std::path::Path,
        base: &Imported,
        head: &Imported,
    ) -> Option<(std::path::PathBuf, record::Identity, record::RunnerKey)> {
        match self.base_identity(req, base, head) {
            Ok((identity, runner)) => Some((root.join(identity.cache_key()), identity, runner)),
            Err(e) => {
                eprintln!("aver: no base record in {} this run: {e}", root.display());
                None
            }
        }
    }

    /// Restores `dir`'s record of the base into the run cache when it was
    /// made under `expected`: on this runner for this base tree and the
    /// head's dependency inputs; says on stderr when it was not, and the
    /// base runs here.
    async fn restore(
        &self,
        dir: &std::path::Path,
        expected: record::Identity,
        base: &Imported,
        label: &str,
    ) -> Result<record::Restored, CiError> {
        let (store, dir, snapshot) = (self.store.clone(), dir.to_owned(), base.snapshot);
        let restored =
            tokio::task::spawn_blocking(move || record::load(&store, &dir, &expected, &snapshot))
                .await
                .map_err(|e| CiError::Internal(e.to_string()))??;
        match &restored {
            record::Restored::Loaded { runs, .. } => self
                .verifier
                .executor()
                .adopt_recorded(runs.iter().copied()),
            record::Restored::Ignored(why) => {
                eprintln!("aver: {label}: ignored, {why}; the base runs here")
            }
        }
        Ok(restored)
    }

    /// Runs every check on the head as a later job's base side would (cache
    /// hits where the verification already ran it so), then writes those
    /// runs to `dir` under the head's identity. Returns how many it wrote.
    async fn save_record(
        &self,
        req: &Records,
        dir: &std::path::Path,
        head: &Imported,
    ) -> Result<usize, CiError> {
        let runner = self.runner_key(req, head)?;
        let repo = std::path::Path::new(&req.repo);
        let identity = record::identity(repo, head.commit.as_str(), head.commit.as_str(), &runner)?;
        let spec = Spec::regression_only();
        let body = TaskBody {
            schema: TaskSchema::for_task(&spec.criteria, &spec.protected_paths),
            title: Text::truncated("aver ci --record"),
            intent: Text::truncated("Run the head's checks as a base, to record them."),
            base: head.snapshot,
            target: None,
            criteria: spec.criteria,
            checks: aver_core::Set::empty(),
            protected_paths: spec.protected_paths,
            author: self.admin,
            created_at: Timestamp::now(),
        };
        let task_id = self.tasks.create(&Task::sign(body, &self.admin_key))?;
        self.verifier.warm(self.admin, task_id).await?;
        let (store, dir, snapshot) = (self.store.clone(), dir.to_owned(), head.snapshot);
        Ok(tokio::task::spawn_blocking(move || {
            record::write(&store, &dir, &snapshot, identity, runner)
        })
        .await
        .map_err(|e| CiError::Internal(e.to_string()))??)
    }

    /// The commits to compare: `--head`, and `--base` or, when that is not
    /// an ancestor of the head, their merge base.
    async fn resolve_base(&self, req: &Request) -> Result<BaseResolution, CiError> {
        let store = self.store.clone();
        let (repo, base, head) = (req.repo.clone(), req.base.clone(), req.head.clone());
        tokio::task::spawn_blocking(move || {
            git::resolve_base(&store, &repo, &base, &head).map_err(|source| {
                let (side, rev) = match &source {
                    ImportError::Rev(rev) if *rev == head && *rev != base => ("head", head),
                    _ => ("base", base),
                };
                CiError::Import { side, rev, source }
            })
        })
        .await
        .map_err(|e| CiError::Internal(e.to_string()))?
    }

    async fn import(
        &self,
        repo: &str,
        rev: &str,
        side: &'static str,
        secrets: SecretPolicy,
    ) -> Result<Imported, CiError> {
        let (store, server_key, resolver) = (
            self.store.clone(),
            self.server_key.clone(),
            self.resolver.clone(),
        );
        let (repo, rev, admin) = (repo.to_owned(), rev.to_owned(), self.admin);
        tokio::task::spawn_blocking(move || {
            let mut options = ImportOptions::new(&repo, &rev, admin, &server_key, &*resolver);
            options.secrets = secrets;
            options.oversize = OversizePolicy::Placeholder;
            options.detection = Detection::Presets;
            git::import(&store, &options)
                .map(|r| Imported {
                    commit: r.commit,
                    snapshot: r.snapshot,
                    why: why_trailer(&r.message),
                    secrets: r.secrets,
                    placeholders: r.placeholders,
                })
                .map_err(|source| CiError::Import { side, rev, source })
        })
        .await
        .map_err(|e| CiError::Internal(e.to_string()))?
    }
}

/// The values of a commit message's `Aver-Why:` trailers (the key in any
/// case), in its last paragraph, one per line: the author's stated reason
/// for what the change rewrites.
pub fn why_trailer(message: &str) -> Option<String> {
    let last = message.trim_end().rsplit("\n\n").next()?;
    let values: Vec<&str> = last
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("Aver-Why")
                .then(|| value.trim())
        })
        .filter(|v| !v.is_empty())
        .collect();
    (!values.is_empty()).then(|| values.join("\n"))
}

/// The diffs a reviewer of the `CHANGED` entries reads: the file each
/// entry's diagnostics name, the harness files of `judging::` entries, and,
/// when an entry's file is unknown (a whole check, or a runner that names
/// no files), every existing test file the head modified or deleted. At most
/// [`MAX_REVIEW_FILES`], each cut to [`MAX_REVIEW_DIFF`] bytes.
pub(crate) fn review_diffs(
    store: &Store,
    base: &Snapshot,
    head: &Snapshot,
    receipt: &aver_core::ReceiptBody,
    diagnostics: &Diagnostics,
) -> Result<Vec<ReviewDiff>, crate::store::StoreError> {
    use std::collections::BTreeSet;
    let mut paths: BTreeSet<String> = BTreeSet::new();
    let mut unknown = false;
    for entry in receipt.checks.iter() {
        if entry.cause != Some(aver_core::Cause::Changed) {
            continue;
        }
        if let Some((crate::verify::JUDGING_CHECK | crate::verify::SENSITIVE_CHECK, target)) =
            entry.id.as_str().split_once("::")
        {
            let path = target.split_once('#').map_or(target, |(p, _)| p);
            paths.insert(path.trim_end_matches('…').to_owned());
            continue;
        }
        match diagnostics
            .failures
            .iter()
            .find(|f| f.id == entry.id)
            .and_then(|f| f.file.as_ref())
        {
            Some(file) => {
                paths.insert(file.as_str().to_owned());
            }
            None => unknown = true,
        }
    }
    if unknown {
        if let Some(tc) = &receipt.test_changes {
            for p in tc
                .files_modified
                .items
                .iter()
                .chain(tc.files_deleted.items.iter())
            {
                paths.insert(p.as_str().to_owned());
            }
        }
    }
    let read = |snapshot: &Snapshot, path: &str| -> Result<String, crate::store::StoreError> {
        let Ok(path) = path.parse::<aver_core::Path>() else {
            return Ok(String::new());
        };
        Ok(match crate::env::lookup(store, &snapshot.root, &path)? {
            Some(aver_core::Node::File(blob) | aver_core::Node::Exec(blob)) => {
                String::from_utf8_lossy(&store.get_raw(&blob)?).into_owned()
            }
            _ => String::new(),
        })
    };
    let mut out = Vec::new();
    for path in paths.into_iter().take(MAX_REVIEW_FILES) {
        let (old, new) = (read(base, &path)?, read(head, &path)?);
        let diff = similar::TextDiff::from_lines(&old, &new)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string();
        out.push(ReviewDiff {
            diff: cut(&diff, MAX_REVIEW_DIFF),
            path,
        });
    }
    Ok(out)
}

/// `text` cut to `max` bytes on a line or character boundary, with a line
/// saying how much was left out.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').map_or(end, |i| i + 1);
    format!(
        "{}[aver] {} more bytes in this diff\n",
        &text[..end],
        text.len() - end
    )
}

/// Every run's output, once, named `<check>.<side>.<n>.log`; a retried
/// infra run keeps both, the retry with `-retry`.
fn run_logs(
    store: &Store,
    runs: &[verify::TimedRun],
) -> Result<Vec<RunLog>, crate::store::StoreError> {
    let mut names = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for run in runs {
        let side = match run.side {
            verify::Side::Base => "base",
            verify::Side::Head => "head",
            verify::Side::HeadTests => "head_tests",
            verify::Side::Transplant => "transplant",
            verify::Side::Ablation => "ablation",
        };
        let confirm = if run.confirmation { "-confirm" } else { "" };
        let mut name = format!("{}.{side}.{}{confirm}", run.check, run.index);
        while !names.insert(name.clone()) {
            name.push_str("-retry");
        }
        let stored: aver_core::CheckRun = store.get(&run.run)?;
        out.push(RunLog {
            name: format!("{name}.log"),
            output: store.get_raw(&stored.output)?,
        });
    }
    Ok(out)
}

/// The secrets and placeholders both imports reported, merged by path (and
/// pattern) with the sides that reported them.
fn import_notes(base: &Imported, head: &Imported) -> ImportNotes {
    use std::collections::BTreeMap;
    let mut secrets: BTreeMap<(String, &'static str), SecretNote> = BTreeMap::new();
    let mut placeholders: BTreeMap<String, PlaceholderNote> = BTreeMap::new();
    for (side, imported) in [("base", base), ("head", head)] {
        for f in &imported.secrets {
            secrets
                .entry((f.path.clone(), f.pattern))
                .or_insert_with(|| SecretNote {
                    path: f.path.clone(),
                    pattern: f.pattern.to_owned(),
                    severity: f.severity.as_str().to_owned(),
                    sides: Vec::new(),
                })
                .sides
                .push(side.to_owned());
        }
        for p in &imported.placeholders {
            placeholders
                .entry(p.path.clone())
                .or_insert_with(|| PlaceholderNote {
                    path: p.path.clone(),
                    bytes: p.bytes,
                    sides: Vec::new(),
                })
                .sides
                .push(side.to_owned());
        }
    }
    let cap = |total: usize| total.saturating_sub(MAX_LISTING) as u64;
    ImportNotes {
        secrets_omitted: cap(secrets.len()),
        secrets: secrets.into_values().take(MAX_LISTING).collect(),
        placeholders_omitted: cap(placeholders.len()),
        placeholders: placeholders.into_values().take(MAX_LISTING).collect(),
    }
}

/// The file a reproducer writes `ci.criteria_text` to.
pub const CRITERIA_FILE: &str = "criteria.txt";

/// The `aver ci` command that repeats a run from its receipt alone (§2.9):
/// the repository when it is a URL someone else can fetch, else `<repo>`;
/// the resolved commits; and `--criteria-sha256`, so the run refuses
/// criteria other than the receipt's, read from [`CRITERIA_FILE`] holding
/// `ci.criteria_text` when there are any.
pub fn reproduce(repo: &str, base: &GitSha, head: &GitSha, spec: &Spec) -> String {
    let repo = if is_remote(repo) {
        shell_quote(&crate::git::redact_url(repo))
    } else {
        "<repo>".to_owned()
    };
    let mut command = format!(
        "aver ci --repo {repo} --base {base} --head {head} --criteria-sha256 {}",
        spec.sha256()
    );
    if spec.normalized().is_some() {
        command.push_str(&format!(" --criteria {CRITERIA_FILE}"));
    }
    command
}

/// Whether `repo` names a repository by URL (`scheme://…` or `user@host:path`)
/// rather than a path on this machine.
fn is_remote(repo: &str) -> bool {
    match repo.split_once("://") {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
                && scheme != "file"
        }
        None => repo
            .split_once(':')
            .is_some_and(|(host, _)| host.contains('@') && !host.contains('/')),
    }
}

/// `word` as one shell word.
pub fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/-:@".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}
