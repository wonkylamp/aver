//! Typed calls to the services of an `$AVER_HOME` (§2.7.1) for `aver run`,
//! `aver accept` and every other command, in the calling process. Each
//! call acts as the key it is given: the admin for task, key and
//! materialization work, a per-attempt key for what only the workspace's
//! owner may do.

use std::path::Path;
use std::sync::Arc;

use aver_core::{
    AgentId, AttemptId, Receipt, ReceiptId, SigningKey, SnapshotId, Task, TaskId, WorkspaceId,
};
use serde::Deserialize;

use crate::agents::{self, AgentError, AgentKind, Capability, NewAgent};
use crate::coord::{CoordError, HeldLease, LeaseRequest, Notices};
use crate::diagnostics::Diagnostics;
use crate::disk::{Materialization, SyncReport};
use crate::git::{ExportError, ImportError};
use crate::services::Services;
use crate::store::StoreError;
use crate::tasks::{Attempt, Claim, TaskError};
use crate::verify::{Head, Request, VerifyError};
use crate::workspace::merge::{Conflict, MergeRule};
use crate::workspace::{CommitOutcome, DiffChange, MaterializeOptions, WorkspaceError};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// A refusal, with the code `run/*` matches on (`SNAPSHOT_MOVED`,
    /// `BRANCH_EXISTS`, `NOT_RECEIPT_HEAD`, …).
    #[error("{message} ({code})")]
    Refused { code: String, message: String },
}

/// The refusal of a verify with no check to run.
pub const NOT_CHECKED: &str = "NOT_CHECKED";

impl ApiError {
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Refused { code, .. } => Some(code),
        }
    }

    fn refused(code: &str, message: impl std::fmt::Display) -> Self {
        Self::Refused {
            code: code.to_owned(),
            message: message.to_string(),
        }
    }

    fn invalid(message: impl std::fmt::Display) -> Self {
        Self::refused("INVALID", message)
    }

    fn not_found(message: impl std::fmt::Display) -> Self {
        Self::refused("NOT_FOUND", message)
    }

    fn forbidden(message: impl std::fmt::Display) -> Self {
        Self::refused("FORBIDDEN", message)
    }

    fn internal(message: impl std::fmt::Display) -> Self {
        Self::refused("INTERNAL", message)
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(id) => Self::not_found(format!("object {id} not found")),
            StoreError::BlobTooLarge(_) => Self::refused("TOO_LARGE", e),
            StoreError::RefConflict { .. } => Self::refused("REF_CONFLICT", e),
            other => Self::internal(other),
        }
    }
}

impl From<AgentError> for ApiError {
    fn from(e: AgentError) -> Self {
        match e {
            AgentError::Exists(_) => Self::refused("AGENT_EXISTS", e),
            AgentError::NotFound(_) => Self::not_found(e),
            AgentError::InvalidName(_) | AgentError::UnknownCapability(_) => Self::invalid(e),
            AgentError::Store(e) => e.into(),
        }
    }
}

impl From<WorkspaceError> for ApiError {
    fn from(e: WorkspaceError) -> Self {
        Self::refused(e.code(), e)
    }
}

impl From<TaskError> for ApiError {
    fn from(e: TaskError) -> Self {
        match e {
            TaskError::NotFound(m) => Self::not_found(m),
            TaskError::Forbidden(m) => Self::forbidden(m),
            TaskError::Invalid(m) => Self::invalid(m),
            TaskError::AttemptClosed { .. } => Self::refused("ATTEMPT_CLOSED", e),
            TaskError::StaleReceipt(m) => Self::refused("STALE_RECEIPT", m),
            TaskError::AlreadyDecided(m) => Self::refused("ALREADY_DECIDED", m),
            TaskError::Workspace(e) => e.into(),
            TaskError::Store(e) => e.into(),
        }
    }
}

impl From<VerifyError> for ApiError {
    fn from(e: VerifyError) -> Self {
        match e {
            VerifyError::NotFound(m) => Self::not_found(m),
            VerifyError::Invalid(m) => Self::invalid(m),
            VerifyError::Task(e) => e.into(),
            VerifyError::Workspace(e) => e.into(),
            VerifyError::Store(e) => e.into(),
            VerifyError::Exec(_) | VerifyError::Internal(_) => Self::internal(e),
            VerifyError::Interrupted => Self::refused("INTERRUPTED", e),
            VerifyError::NotChecked(m) => Self::refused(NOT_CHECKED, m),
        }
    }
}

impl From<ImportError> for ApiError {
    fn from(e: ImportError) -> Self {
        match e {
            ImportError::Rejected(_) => Self::refused("IMPORT_REJECTED", e),
            ImportError::Source(_)
            | ImportError::Rev(_)
            | ImportError::Commit { .. }
            | ImportError::Git(_) => Self::invalid(e),
            ImportError::Env(_) => Self::refused("ENV_UNRESOLVED", e),
            ImportError::Store(e) => e.into(),
        }
    }
}

impl From<ExportError> for ApiError {
    fn from(e: ExportError) -> Self {
        match e {
            ExportError::InvalidBranch(_) | ExportError::NoLineage(_) | ExportError::Cycle(_) => {
                Self::invalid(e)
            }
            ExportError::NonFastForward { .. } => Self::refused("REF_CONFLICT", e),
            ExportError::BranchExists { .. } => Self::refused("BRANCH_EXISTS", e),
            ExportError::NotReviewed { .. } => Self::refused("NOT_RECEIPT_HEAD", e),
            ExportError::Store(StoreError::NotFound(id)) => {
                Self::not_found(format!("object {id} not found"))
            }
            ExportError::Git(_) | ExportError::Store(_) | ExportError::FastImport(_) => {
                Self::internal(e)
            }
        }
    }
}

impl From<CoordError> for ApiError {
    fn from(e: CoordError) -> Self {
        match e {
            CoordError::Workspace(e) => e.into(),
            CoordError::Store(e) => e.into(),
            CoordError::Invalid(m) => Self::invalid(m),
            CoordError::NotFound(m) => Self::not_found(m),
            CoordError::Forbidden(m) => Self::forbidden(m),
            CoordError::Conflict { message, .. } => Self::refused("LEASE_CONFLICT", message),
        }
    }
}

/// How a verify runs ([`Api::verify_with`]).
#[derive(Debug, Clone, Default)]
pub struct VerifyFlags {
    /// Every run starts afresh instead of reading the run cache.
    pub fresh: bool,
    /// Every check runs, also those the change does not affect.
    pub all: bool,

    /// `aver check`: the same runs, nothing signed or stored.
    pub dry_run: bool,
    /// `aver check -- <cmd>`: this one check instead of the task's (dry
    /// run only; refused otherwise).
    pub adhoc: Option<aver_core::CheckDef>,
    /// `aver verify --reason`: why the change rewrites what it does, kept in
    /// the diagnostics for a reviewer.
    pub reason: Option<String>,
}

/// A verify's result: the receipt, its diagnostics and the sync before it.
#[derive(Debug, Clone, Deserialize)]
pub struct Verification {
    pub receipt_id: ReceiptId,
    pub receipt: Receipt,
    #[serde(default)]
    pub diagnostics: Option<Diagnostics>,
    #[serde(default)]
    pub sync: Option<SyncReport>,
    /// Other tasks' changes to the same files (a disk workspace).
    #[serde(default)]
    pub notices: Option<Notices>,
    /// Where the verification's time went (absent for a stored receipt).
    #[serde(default)]
    pub timing: Option<crate::verify::Timing>,
}

/// One changed file of a workspace diff.
#[derive(Debug, Clone, Deserialize)]
pub struct DiffFile {
    pub path: String,
    pub change: DiffChange,
    #[serde(default)]
    pub binary: bool,
    pub diff: String,
}

impl DiffFile {
    /// Lines added and removed, counted from the unified diff.
    pub fn lines(&self) -> (u32, u32) {
        let (mut add, mut del) = (0, 0);
        for line in self.diff.lines() {
            if line.starts_with("+++") || line.starts_with("---") {
                continue;
            }
            if line.starts_with('+') {
                add += 1;
            } else if line.starts_with('-') {
                del += 1;
            }
        }
        (add, del)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Imported {
    pub snapshot: SnapshotId,
    pub commit: aver_core::GitSha,
    pub files: u64,
    #[serde(default)]
    pub already_imported: bool,
    /// Files with secret-like content the import let through.
    #[serde(default)]
    pub secrets: Vec<String>,
}

/// The services, seen through typed calls.
#[derive(Clone)]
pub struct Api {
    services: Arc<Services>,
}

/// The agent that signs with `key`: whose workspaces and attempts it may
/// act on.
fn caller(key: &SigningKey) -> AgentId {
    AgentId::of_public_key(&key.verifying_key())
}

/// Runs blocking store, disk and Git work off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(ApiError::internal)?
}

/// Whether agent `id` holds `capability` (directly or as `admin`) and is
/// not revoked: an overseer reads attempts and releases leases that are
/// not its own.
fn holds(s: &Services, id: &AgentId, capability: Capability) -> Result<bool, ApiError> {
    Ok(agents::get(&s.store, id)?.is_some_and(|a| !a.is_revoked() && a.can(capability)))
}

/// An ad-hoc check names the command, its directory, timeout and report
/// only: its image, layers and setup are the base environment's
/// ([`crate::adhoc::fit`]), so a caller cannot pick what it runs in.
fn adhoc_valid(check: &aver_core::CheckDef) -> Result<(), ApiError> {
    if check.id.as_str() != crate::adhoc::ID {
        return Err(ApiError::invalid(format!(
            "an ad-hoc check has id {:?}",
            crate::adhoc::ID
        )));
    }
    if check.argv.is_empty() || check.argv.iter().any(|a| a.contains('\0')) {
        return Err(ApiError::invalid(
            "an ad-hoc check needs a command without NUL bytes",
        ));
    }
    if check.image.is_some()
        || check.deps.is_some()
        || !check.setup.is_empty()
        || !check.covers.is_empty()
    {
        return Err(ApiError::invalid(
            "an ad-hoc check takes its image, dependency layers, setup and covers from the base environment",
        ));
    }
    Ok(())
}

impl Api {
    /// The services in this process.
    pub fn new(services: Arc<Services>) -> Self {
        Self { services }
    }

    /// The services of `home`, opened in this process.
    pub fn open(home: &crate::keys::Home) -> Result<Self, crate::keys::KeyError> {
        Ok(Self::new(Arc::new(Services::open(home)?)))
    }

    /// The services this `Api` calls.
    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    pub async fn import(
        &self,
        key: &SigningKey,
        repo: &std::path::Path,
        rev: &str,
    ) -> Result<Imported, ApiError> {
        let s = self.services.clone();
        let (agent, repo, rev) = (
            caller(key),
            repo.to_string_lossy().into_owned(),
            rev.to_owned(),
        );
        blocking(move || {
            let mut options =
                crate::git::ImportOptions::new(&repo, &rev, agent, &s.server_key, &*s.images);
            options.secrets = s.import_secrets;
            let report = crate::git::import(&s.store, &options)?;
            Ok(Imported {
                snapshot: report.snapshot,
                commit: report.commit,
                files: report.files as u64,
                already_imported: report.already_imported,
                secrets: report.secrets.into_iter().map(|f| f.path).collect(),
            })
        })
        .await
    }

    pub async fn create_task(&self, task: &Task) -> Result<TaskId, ApiError> {
        let s = self.services.clone();
        let task = task.clone();
        blocking(move || {
            s.tasks.create(&task)?;
            Ok(aver_core::Object::id(&task))
        })
        .await
    }

    pub async fn task(&self, id: &TaskId) -> Result<Task, ApiError> {
        let s = self.services.clone();
        let id = *id;
        blocking(move || Ok(s.tasks.get(&id)?)).await
    }

    pub async fn register_agent(
        &self,
        admin: &SigningKey,
        name: &str,
        public_key: &str,
        caps: &[Capability],
    ) -> Result<(), ApiError> {
        let s = self.services.clone();
        let bytes: [u8; 32] = aver_core::decode_base64url(public_key)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| {
                ApiError::invalid("public_key must be 32 bytes of unpadded base64url")
            })?;
        let public_key = aver_core::VerifyingKey::from_bytes(&bytes)
            .map_err(|_| ApiError::invalid("public_key is not a valid Ed25519 key"))?;
        let name: agents::AgentName = name.parse()?;
        let new = NewAgent {
            public_key,
            name,
            kind: AgentKind::Agent,
            capabilities: caps.iter().copied().collect(),
            issued_by: Some(caller(admin)),
        };
        blocking(move || {
            agents::register(&s.store, new)?;
            Ok(())
        })
        .await
    }

    pub async fn revoke_agent(&self, id: &AgentId) -> Result<(), ApiError> {
        let s = self.services.clone();
        let id = *id;
        blocking(move || {
            let agent = agents::get(&s.store, &id)?
                .ok_or_else(|| ApiError::not_found(format!("agent {id} is not registered")))?;
            if agent.kind == AgentKind::Server {
                return Err(ApiError::forbidden("the server key cannot be revoked"));
            }
            agents::revoke(&s.store, &id)?;
            Ok(())
        })
        .await
    }

    pub async fn claim(
        &self,
        key: &SigningKey,
        task: &TaskId,
        base: Option<SnapshotId>,
    ) -> Result<Claim, ApiError> {
        let s = self.services.clone();
        let (agent, task) = (caller(key), *task);
        blocking(move || Ok(s.tasks.claim(&agent, &task, base)?)).await
    }

    /// Starts the task's checks on its base (§2.10) in a detached
    /// `aver internal warm`, so the agents' first verifications find them
    /// cached. Nothing waits for it, and a command that exits leaves it
    /// running; its output goes to `logs/warm-<ulid>.log` in the home.
    pub fn warm(&self, aver: &Path, task: &TaskId) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;
        let home = self.services.home.root();
        let logs = home.join("logs");
        std::fs::create_dir_all(&logs)?;
        let log = std::fs::File::create(logs.join(format!("warm-{}.log", ulid::Ulid::generate())))?;
        let mut child = std::process::Command::new(aver)
            .args(["internal", "warm", &task.to_string()])
            .env("AVER_HOME", home)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn()?;
        std::thread::spawn(move || child.wait());
        Ok(())
    }

    pub async fn materialize(
        &self,
        ws: &WorkspaceId,
        o: &MaterializeOptions,
    ) -> Result<Materialization, ApiError> {
        let s = self.services.clone();
        let ws = *ws;
        let options = o.clone();
        blocking(move || Ok(s.workspaces.materialize(&ws, &options)?)).await
    }

    pub async fn release(&self, ws: &WorkspaceId) -> Result<(), ApiError> {
        let s = self.services.clone();
        let ws = *ws;
        blocking(move || Ok(s.workspaces.release(&ws, false)?)).await
    }

    pub async fn sync(&self, owner: &SigningKey, ws: &WorkspaceId) -> Result<SyncReport, ApiError> {
        let s = self.services.clone();
        let (agent, ws) = (caller(owner), *ws);
        blocking(move || {
            s.workspaces
                .sync(&agent, &ws)?
                .ok_or_else(|| ApiError::invalid(format!("workspace {ws} is not materialized")))
        })
        .await
    }

    /// Syncs the workspace and prepares what `aver check` runs (§2.3.1),
    /// with `combined` also the trial merge with other tasks' work.
    /// The trial merge of workspace `ws` at `mine` with other tasks'
    /// current work on its base (§2.11), for `aver check --combined`;
    /// `None` when no other task works there.
    pub async fn trial(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
        mine: &SnapshotId,
    ) -> Result<Option<crate::coord::Trial>, ApiError> {
        let s = self.services.clone();
        let (agent, ws, mine) = (caller(owner), *ws, *mine);
        blocking(move || Ok(s.coord().trial(&agent, &ws, &mine)?)).await
    }

    pub async fn restore(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
        paths: &[aver_core::Path],
    ) -> Result<Vec<String>, ApiError> {
        let s = self.services.clone();
        let (agent, ws, paths) = (caller(owner), *ws, paths.to_vec());
        blocking(move || Ok(s.workspaces.restore(&agent, &ws, &paths)?)).await
    }

    /// Puts back what the workspace's last restore replaced.
    pub async fn restore_undo(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
    ) -> Result<Vec<String>, ApiError> {
        let s = self.services.clone();
        let (agent, ws) = (caller(owner), *ws);
        blocking(move || Ok(s.workspaces.restore_undo(&agent, &ws)?)).await
    }

    pub async fn diff(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
    ) -> Result<Vec<DiffFile>, ApiError> {
        let s = self.services.clone();
        let (agent, ws) = (caller(owner), *ws);
        blocking(move || {
            let (_, files) = s.workspaces.diff(&agent, &ws)?;
            serde_json::to_value(files)
                .and_then(serde_json::from_value)
                .map_err(ApiError::internal)
        })
        .await
    }

    /// The workspace's current snapshot (a disk workspace syncs first).
    pub async fn snapshot(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
    ) -> Result<SnapshotId, ApiError> {
        let s = self.services.clone();
        let (agent, ws) = (caller(owner), *ws);
        blocking(move || Ok(s.workspaces.snapshot(&agent, &ws)?)).await
    }

    pub async fn commit(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
        message: &str,
    ) -> Result<CommitOutcome, ApiError> {
        self.commit_if(owner, ws, message, None).await
    }

    /// [`Self::commit`] only if the directory's changes still make the
    /// snapshot `reviewed`, else `SNAPSHOT_MOVED` (`aver accept` passes the
    /// verified one).
    pub async fn commit_if(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
        message: &str,
        reviewed: Option<&SnapshotId>,
    ) -> Result<CommitOutcome, ApiError> {
        let s = self.services.clone();
        let (agent, ws) = (caller(owner), *ws);
        let request = crate::workspace::CommitRequest {
            message: message.to_owned(),
            if_version: None,
            if_snapshot: reviewed.copied(),
        };
        blocking(move || Ok(s.workspaces.commit(&agent, &ws, request)?)).await
    }

    pub async fn discard(&self, owner: &SigningKey, ws: &WorkspaceId) -> Result<(), ApiError> {
        let s = self.services.clone();
        let (agent, ws) = (caller(owner), *ws);
        blocking(move || {
            s.workspaces.discard(&agent, &ws)?;
            Ok(())
        })
        .await
    }

    pub async fn verify(
        &self,
        key: &SigningKey,
        task: &TaskId,
        ws: &WorkspaceId,
        fresh: bool,
    ) -> Result<Verification, ApiError> {
        self.verify_with(
            key,
            task,
            ws,
            VerifyFlags {
                fresh,
                ..VerifyFlags::default()
            },
            None,
        )
        .await
    }

    /// [`Api::verify`], also running checks the change does not affect
    /// with `all`, and logging each run's start and end to `progress`.
    pub async fn verify_with(
        &self,
        key: &SigningKey,
        task: &TaskId,
        ws: &WorkspaceId,
        flags: VerifyFlags,
        progress: Option<Arc<crate::progress::Log>>,
    ) -> Result<Verification, ApiError> {
        let VerifyFlags {
            fresh,
            all,
            dry_run,
            adhoc,
            reason,
        } = flags;
        let s = self.services.clone();
        if adhoc.is_some() && !dry_run {
            return Err(ApiError::invalid(
                "adhoc goes with dry_run: an ad-hoc check is never signed",
            ));
        }
        if let Some(check) = &adhoc {
            adhoc_valid(check)?;
        }
        let agent = caller(key);
        // The directory is synced first so the result can say what the
        // receipt's head contains; verify's own read then finds it current.
        let sync = {
            let (s, ws) = (s.clone(), *ws);
            blocking(move || Ok(s.workspaces.sync(&agent, &ws)?)).await?
        };
        let request = Request {
            task: *task,
            head: Head::Workspace(*ws),
            attempt: None,
            fresh,
            all,
            progress,
            ci: None,
            adhoc,
            reason,
        };
        let verified = if dry_run {
            s.verifier.dry_run(agent, request).await?
        } else {
            s.verifier.verify(agent, request).await?
        };
        // What other tasks in the repository do to the same files; the sync
        // above made the overlay current.
        let notices = match &sync {
            Some(_) => {
                let (s, ws) = (s.clone(), *ws);
                blocking(move || Ok(s.coord.notices(&agent, &ws, false)?))
                    .await
                    .ok()
            }
            None => None,
        };
        Ok(Verification {
            receipt_id: verified.id,
            receipt: verified.receipt,
            diagnostics: Some(verified.diagnostics),
            sync,
            notices,
            timing: Some(verified.timing),
        })
    }

    /// An attempt: its owner's, or anyone's for a key holding
    /// `task:decide` (the supervisor of `aver run` reads every attempt).
    pub async fn attempt(&self, key: &SigningKey, id: &AttemptId) -> Result<Attempt, ApiError> {
        let s = self.services.clone();
        let (agent, id) = (caller(key), *id);
        blocking(move || {
            let overseer = holds(&s, &agent, Capability::TaskDecide)?;
            Ok(s.tasks.attempt_for(&agent, &id, overseer)?)
        })
        .await
    }

    pub async fn receipt(&self, id: &ReceiptId) -> Result<Verification, ApiError> {
        let s = self.services.clone();
        let id = *id;
        blocking(move || {
            Ok(Verification {
                receipt_id: id,
                receipt: s.store.get(&id)?,
                diagnostics: crate::diagnostics::load(&s.store, &id)?,
                sync: None,
                notices: None,
                timing: None,
            })
        })
        .await
    }

    /// Records which attempt of a task won (§2.6): frozen attempt records
    /// and one ledger `accept` entry, no ref moved.
    pub async fn decide(
        &self,
        admin: &SigningKey,
        task: &TaskId,
        decision: &crate::tasks::Decision,
    ) -> Result<crate::tasks::Decided, ApiError> {
        let s = self.services.clone();
        let (agent, task, decision) = (caller(admin), *task, decision.clone());
        blocking(move || Ok(s.tasks.decide(&agent, &task, &decision)?)).await
    }

    pub async fn export(
        &self,
        snapshot: &SnapshotId,
        repo: &std::path::Path,
        branch: &str,
        author: Option<&crate::git::Identity>,
    ) -> Result<String, ApiError> {
        self.export_as(
            snapshot,
            repo,
            branch,
            crate::git::ExportOptions {
                identity: author.cloned(),
                new_branch: false,
                receipt: None,
            },
        )
        .await
    }

    /// [`Api::export`] of the snapshot `receipt` verified, to a new
    /// `branch`: any other snapshot is refused, and so is a branch that
    /// exists unless it already holds this export's commit, which is then
    /// adopted (an accept run again after its branch was written).
    pub async fn export_reviewed(
        &self,
        snapshot: &SnapshotId,
        receipt: &aver_core::ReceiptId,
        repo: &std::path::Path,
        branch: &str,
        author: Option<&crate::git::Identity>,
    ) -> Result<String, ApiError> {
        self.export_as(
            snapshot,
            repo,
            branch,
            crate::git::ExportOptions {
                identity: author.cloned(),
                new_branch: true,
                receipt: Some(*receipt),
            },
        )
        .await
    }

    async fn export_as(
        &self,
        snapshot: &SnapshotId,
        repo: &std::path::Path,
        branch: &str,
        options: crate::git::ExportOptions,
    ) -> Result<String, ApiError> {
        let s = self.services.clone();
        // A relative path would resolve against this process's working
        // directory, and one starting with `-` could read as an option to git.
        if !repo.is_absolute() {
            return Err(ApiError::invalid("repo must be an absolute path"));
        }
        let (snapshot, repo, branch) = (*snapshot, repo.to_owned(), branch.to_owned());
        blocking(move || {
            let report = crate::git::export_with(&s.store, &snapshot, &repo, &branch, &options)?;
            Ok(report.commit.to_string())
        })
        .await
    }

    pub async fn acquire_lease(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
        req: &LeaseRequest,
    ) -> Result<HeldLease, ApiError> {
        let s = self.services.clone();
        let (agent, ws, req) = (caller(owner), *ws, req.clone());
        blocking(move || Ok(s.coord.acquire(&agent, &ws, req)?)).await
    }

    pub async fn release_lease(
        &self,
        key: &SigningKey,
        id: &aver_core::LeaseId,
    ) -> Result<(), ApiError> {
        let s = self.services.clone();
        let (agent, id) = (caller(key), *id);
        blocking(move || {
            let admin = holds(&s, &agent, Capability::Admin)?;
            s.coord.release(&agent, &id, admin)?;
            Ok(())
        })
        .await
    }

    /// Every live lease in the workspace's repository (no sync).
    pub async fn leases(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
    ) -> Result<Vec<HeldLease>, ApiError> {
        let s = self.services.clone();
        let (agent, ws) = (caller(owner), *ws);
        blocking(move || Ok(s.coord.leases(&agent, &ws)?)).await
    }

    /// What other tasks do in the workspace's repository (syncs first).
    pub async fn notices(&self, owner: &SigningKey, ws: &WorkspaceId) -> Result<Notices, ApiError> {
        let s = self.services.clone();
        let (agent, ws) = (caller(owner), *ws);
        blocking(move || Ok(s.coord.notices(&agent, &ws, true)?)).await
    }

    /// Three-way merges `heads` onto `base` as `aver combine` does, taking
    /// every head's lines where they only add lines at one point
    /// ([`MergeRule::UnionInsertions`]); with `open`, a clean merge opens a
    /// workspace holding it, owned by `key`.
    pub async fn merge(
        &self,
        key: &SigningKey,
        base: &SnapshotId,
        heads: &[SnapshotId],
        open: bool,
    ) -> Result<MergeOutcome, ApiError> {
        let s = self.services.clone();
        if heads.len() < 2 || heads.len() > 16 {
            return Err(ApiError::invalid("merge takes 2 to 16 heads"));
        }
        let (agent, base, heads) = (caller(key), *base, heads.to_vec());
        blocking(move || {
            let merged = s
                .workspaces
                .three_way(&base, &heads, MergeRule::UnionInsertions)?;
            let mut out = MergeOutcome {
                clean: merged.conflicts.is_empty(),
                merged: merged.merged,
                conflicts: merged.conflicts,
                changes: merged.changes.len(),
                workspace: None,
                snapshot: None,
            };
            if out.clean && open {
                let ws = s.workspaces.open_merged(&agent, &base, &merged.changes)?;
                out.snapshot = Some(s.workspaces.snapshot(&agent, &ws.id)?);
                out.workspace = Some(ws.id);
            }
            Ok(out)
        })
        .await
    }

    /// Verifies snapshot `head` against `base` for `task` (no attempt).
    pub async fn verify_head(
        &self,
        key: &SigningKey,
        task: &TaskId,
        head: &SnapshotId,
        base: &SnapshotId,
    ) -> Result<Verification, ApiError> {
        self.verify_snapshot(key, task, head, base, false).await
    }

    /// [`Api::verify_head`] as a dry run: the same runs, nothing stored,
    /// no budget spent.
    pub async fn verify_head_dry(
        &self,
        key: &SigningKey,
        task: &TaskId,
        head: &SnapshotId,
        base: &SnapshotId,
    ) -> Result<Verification, ApiError> {
        self.verify_snapshot(key, task, head, base, true).await
    }

    async fn verify_snapshot(
        &self,
        key: &SigningKey,
        task: &TaskId,
        head: &SnapshotId,
        base: &SnapshotId,
        dry_run: bool,
    ) -> Result<Verification, ApiError> {
        let s = &self.services;
        let request = Request {
            task: *task,
            head: Head::Snapshot {
                head: *head,
                base: Some(*base),
            },
            attempt: None,
            fresh: false,
            all: false,
            progress: None,
            ci: None,
            adhoc: None,
            reason: None,
        };
        let agent = caller(key);
        let verified = if dry_run {
            s.verifier.dry_run(agent, request).await?
        } else {
            s.verifier.verify(agent, request).await?
        };
        Ok(Verification {
            receipt_id: verified.id,
            receipt: verified.receipt,
            diagnostics: Some(verified.diagnostics),
            sync: None,
            notices: None,
            timing: Some(verified.timing),
        })
    }

    /// Opens an in-store workspace on `base`, owned by `key`.
    pub async fn open_workspace(
        &self,
        key: &SigningKey,
        base: &SnapshotId,
    ) -> Result<WorkspaceId, ApiError> {
        let s = self.services.clone();
        let (agent, base) = (caller(key), *base);
        blocking(move || Ok(s.workspaces.open(&agent, &base)?.id)).await
    }

    /// The bytes of `path` in snapshot `snapshot`.
    pub async fn read_file(&self, snapshot: &SnapshotId, path: &str) -> Result<Vec<u8>, ApiError> {
        let s = self.services.clone();
        let path: aver_core::Path = path.parse().map_err(ApiError::invalid)?;
        let snapshot = *snapshot;
        blocking(move || Ok(s.workspaces.read_snapshot(&snapshot, &path)?.bytes)).await
    }

    /// Writes each `(path, bytes)` into workspace `ws` in one edit batch at
    /// its current version.
    pub async fn edit(
        &self,
        key: &SigningKey,
        ws: &WorkspaceId,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), ApiError> {
        let s = self.services.clone();
        let ops = files
            .iter()
            .map(|(path, bytes)| {
                Ok(crate::workspace::EditOp::Write {
                    path: path.parse().map_err(ApiError::invalid)?,
                    content: None,
                    content_b64: Some(data_encoding::BASE64.encode(bytes)),
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let (agent, ws) = (caller(key), *ws);
        blocking(move || {
            let if_version = s.workspaces.get(&agent, &ws)?.version;
            s.workspaces.edit(
                &agent,
                &ws,
                crate::workspace::EditRequest { if_version, ops },
            )?;
            Ok(())
        })
        .await
    }

    /// The stored run `id` (`aver check -- <cmd>` reads its transplant runs).
    pub async fn run(&self, id: &aver_core::RunId) -> Result<aver_core::CheckRun, ApiError> {
        let s = self.services.clone();
        let id = *id;
        blocking(move || Ok(s.store.get(&id)?)).await
    }

    /// Moves a disk workspace onto `onto`.
    pub async fn rebase(
        &self,
        owner: &SigningKey,
        ws: &WorkspaceId,
        onto: &SnapshotId,
    ) -> Result<crate::workspace::rebase::RebaseOutcome, ApiError> {
        let s = self.services.clone();
        let (agent, ws, onto) = (caller(owner), *ws, *onto);
        blocking(move || Ok(s.workspaces.rebase(&agent, &ws, &onto)?)).await
    }
}

/// What a three-way merge found ([`Api::merge`]).
#[derive(Debug, Clone, Deserialize)]
pub struct MergeOutcome {
    pub clean: bool,
    #[serde(default)]
    pub merged: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<Conflict>,
    pub changes: usize,
    #[serde(default)]
    pub workspace: Option<WorkspaceId>,
    #[serde(default)]
    pub snapshot: Option<SnapshotId>,
}
