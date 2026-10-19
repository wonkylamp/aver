//! Coordination between tasks working in one repository (ARCHITECTURE.md
//! §1.8.4, §2.11): leases and overlap notices.
//!
//! A lease reserves path globs for the workspace that took it, for a time
//! the holder's activity renews (every sync of its workspace), so a crashed
//! agent's leases lapse on their own. Leases are advisory: they conflict at
//! acquisition and Claude Code's PreToolUse hook enforces them; any other
//! write shows up as a notice on the next sync.
//!
//! A notice says that another task's workspace in the same repository
//! changed a path this workspace changed, and whether their line changes
//! overlap. Both work across tasks only: attempts at the same task are
//! alternatives, so they never see or block each other.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use aver_core::{
    AgentId, AttemptId, BlobId, LeaseId, Path, PathGlob, SnapshotId, Task, TaskId, Timestamp,
    WorkspaceId,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::disk::MatState;
use crate::store::{Store, StoreError};
use crate::workspace::merge::{hunks, merge_text, Hunk, Lines, MergeRule};
use crate::workspace::{FileEntry, FileKind, Workspace, WorkspaceError, Workspaces};

/// A lease lives this long after its last renewal unless the request says.
pub const DEFAULT_TTL_MS: u64 = 30 * 60 * 1000;
const MIN_TTL_MS: u64 = 60 * 1000;
const MAX_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_GLOBS: usize = 16;
const MAX_REASON_BYTES: usize = 200;
/// A sync renews a lease only once this much of its time has passed, so a
/// sync every few seconds does not write every time.
const RENEW_AFTER_MS: i64 = 60 * 1000;
/// Changed paths listed per other task.
const MAX_PATHS_PER_TASK: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseMode {
    /// Other tasks may not take an overlapping lease, and Claude Code may not
    /// edit under it.
    #[default]
    Exclusive,
    /// Says "I am working here"; conflicts only with an exclusive lease.
    Shared,
}

impl LeaseMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::Shared => "shared",
        }
    }
}

/// A `leases` row (§1.8.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    pub id: LeaseId,
    /// The repository the holder's directory came from, or
    /// `snapshot:<base>` for a workspace without one.
    pub scope: String,
    pub task: Option<TaskId>,
    pub attempt: Option<AttemptId>,
    pub workspace: WorkspaceId,
    pub agent: AgentId,
    pub paths: Vec<PathGlob>,
    pub mode: LeaseMode,
    pub reason: String,
    pub ttl_ms: u64,
    pub acquired_at: Timestamp,
    pub expires_at: Timestamp,
    pub renewals: u32,
    #[serde(default)]
    pub released_at: Option<Timestamp>,
}

impl Lease {
    pub fn covers(&self, path: &str) -> bool {
        self.paths.iter().any(|g| g.covers(path))
    }
}

/// A live lease as one workspace sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeldLease {
    #[serde(flatten)]
    pub lease: Lease,
    /// The holder's task title.
    pub title: Option<String>,
    /// Held by the workspace asking.
    pub mine: bool,
    /// Held by another attempt at the same task (never binding).
    pub sibling: bool,
}

impl HeldLease {
    /// Whether it binds the asking workspace: another task's.
    pub fn binds(&self) -> bool {
        !self.mine && !self.sibling
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LeaseRequest {
    pub paths: Vec<PathGlob>,
    #[serde(default)]
    pub mode: LeaseMode,
    pub reason: String,
    #[serde(default)]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum CoordError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{message}")]
    Conflict {
        message: String,
        holder: Box<HeldLease>,
    },
}

impl From<rusqlite::Error> for CoordError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e.into())
    }
}

impl CoordError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Workspace(e) => e.code(),
            Self::Store(_) => "INTERNAL",
            Self::Invalid(_) => "INVALID",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::Conflict { .. } => "LEASE_CONFLICT",
        }
    }
}

pub type Result<T, E = CoordError> = std::result::Result<T, E>;

/// How another task's change to a path relates to this workspace's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overlap {
    /// Different lines; a merge will likely combine them.
    Disjoint,
    /// Lines cannot be compared (not text, added, deleted, or another base).
    File,
    /// The changed lines overlap or touch: a merge conflicts.
    Lines,
}

/// Another task changed a path this workspace changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    pub path: String,
    pub task: Option<TaskId>,
    pub title: Option<String>,
    pub overlap: Overlap,
    /// Base lines this workspace changed (empty when not comparable).
    pub mine: Vec<Lines>,
    /// Base lines the other task changed.
    pub theirs: Vec<Lines>,
    /// Whether a three-way merge of this workspace's version with each of
    /// the other task's would be clean ([`crate::workspace::merge`]);
    /// `None` when lines cannot be compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<Merge>,
    /// The other task's hunks that meet this workspace's, each followed by
    /// this workspace's, as `-` and `+` lines with a header, at most
    /// [`MAX_DIFF_HUNKS`] of each of [`MAX_HUNK_LINES`] lines. Repository
    /// content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// The other task's versions of the path (their blob ids, `deleted`
    /// for a deletion), so a workspace can tell when their side changed
    /// ([`Notice::key`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub theirs_version: String,
    /// When the other task's workspace that changed the path last recorded
    /// a change (of any file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<Timestamp>,
    /// Set by [`crate::seen::delta`]: the other side recorded a change
    /// within [`crate::seen::SETTLE_MS`], so this may change again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub changing: bool,
}

/// What identifies one item of a workspace's notices in its seen-set
/// (`.aver/notices-seen.json`, §2.11): for an overlap, the other task, the
/// path, the overlap kind and the other side's version; for a leased path,
/// the lease, the path and the lease mode. This workspace's own edits do
/// not change a key unless they change the overlap kind, so an agent
/// editing the same lines again is not told again.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoticeKey(pub String);

impl Notice {
    pub fn key(&self) -> NoticeKey {
        let who = match (self.task, &self.title) {
            (Some(t), _) => t.to_string(),
            (None, Some(t)) => t.clone(),
            (None, None) => String::new(),
        };
        let overlap = match self.overlap {
            Overlap::Disjoint => "disjoint",
            Overlap::File => "file",
            Overlap::Lines => "lines",
        };
        NoticeKey(format!(
            "notice\t{who}\t{}\t{overlap}\t{}",
            self.path, self.theirs_version
        ))
    }
}

impl LeasedPath {
    pub fn key(&self) -> NoticeKey {
        NoticeKey(format!(
            "leased\t{}\t{}\t{}",
            self.lease,
            self.path,
            self.mode.as_str()
        ))
    }
}

/// What a trial merge of two versions of a file would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Merge {
    Clean,
    /// Conflicts.
    Conflict,
    /// Conflicts only because both sides add lines at the same place, which
    /// moving one side's lines away resolves.
    SamePlace,
}

/// A workspace's changes merged with other tasks' current work, for
/// `aver check --combined` ([`Coord::trial`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trial {
    /// The merged tree, on the workspace's base.
    pub tree: SnapshotId,
    /// The other tasks merged in, by quoted title (or id).
    pub with: Vec<String>,
    /// Paths whose versions conflict, where the trial keeps this
    /// workspace's version.
    pub conflicts: Vec<String>,
    /// Paths several sides changed, merged line by line.
    pub merged: Vec<String>,
}

/// Hunks of each side a notice's diff shows.
pub const MAX_DIFF_HUNKS: usize = 3;
/// Lines of each hunk side a notice's diff shows.
pub const MAX_HUNK_LINES: usize = 6;
/// Characters of each line a notice's diff shows.
const MAX_LINE_CHARS: usize = 160;

/// A path this workspace changed under another task's live lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeasedPath {
    pub path: String,
    pub lease: LeaseId,
    pub task: Option<TaskId>,
    pub title: Option<String>,
    pub mode: LeaseMode,
    pub expires_at: Timestamp,
}

/// Another task with live workspaces in the same repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OtherTask {
    pub task: Option<TaskId>,
    pub title: Option<String>,
    pub workspaces: u32,
    /// Paths it changed, at most [`MAX_PATHS_PER_TASK`].
    pub changed: Vec<String>,
    pub more: u32,
}

/// Everything a workspace should know about other tasks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Notices {
    pub notices: Vec<Notice>,
    pub leased: Vec<LeasedPath>,
    pub tasks: Vec<OtherTask>,
    /// Other tasks' live leases in the repository.
    pub leases: Vec<HeldLease>,
    /// Set by [`crate::seen::delta`]: overlaps and leased paths left out
    /// because this workspace was already shown them as they are.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unchanged: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Notices {
    pub fn is_empty(&self) -> bool {
        self.notices.is_empty()
            && self.leased.is_empty()
            && self.tasks.is_empty()
            && self.leases.is_empty()
    }
}

/// What an orchestrator sees from a repository's root (§2.11, the
/// orchestrator view): every task with live workspace directories in the
/// repository, the files each pair of tasks both changed, and the live
/// leases. Workspaces count as of their last sync (every verify, check,
/// hook and `aver notices` syncs one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoView {
    pub repo: PathBuf,
    pub tasks: Vec<RepoTask>,
    /// One per pair of tasks that changed a file in common, worst first.
    pub edges: Vec<OverlapEdge>,
    pub leases: Vec<HeldLease>,
}

/// A task with live workspace directories in the repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoTask {
    pub task: Option<TaskId>,
    pub title: Option<String>,
    pub workspaces: Vec<WorkspaceId>,
    /// Paths its workspaces changed, at most [`MAX_REPO_PATHS`].
    pub changed: Vec<String>,
    #[serde(default)]
    pub more: u32,
}

/// Two tasks changed the same files: compared between each task's most
/// recently updated workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlapEdge {
    pub a: Option<TaskId>,
    pub b: Option<TaskId>,
    /// The worst overlap of its paths.
    pub worst: Overlap,
    pub paths: Vec<EdgePath>,
}

/// One file of an [`OverlapEdge`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgePath {
    pub path: String,
    pub overlap: Overlap,
    /// What a trial merge of the two versions does; `None` when lines
    /// cannot be compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<Merge>,
}

/// Changed paths listed per task in a [`RepoView`].
pub const MAX_REPO_PATHS: usize = 200;

/// The workspace asking, with what scopes its coordination.
struct Holder {
    ws: Workspace,
    scope: String,
    repo: Option<PathBuf>,
    task: Option<TaskId>,
}

fn same_task(a: Option<TaskId>, b: Option<TaskId>) -> bool {
    a.is_some() && a == b
}

fn task_of(conn: &Connection, attempt: Option<AttemptId>) -> Result<Option<TaskId>, StoreError> {
    let Some(attempt) = attempt else {
        return Ok(None);
    };
    let task: Option<String> = conn
        .query_row(
            "SELECT task FROM attempts WHERE id = ?1",
            [attempt.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(task.and_then(|t| t.parse().ok()))
}

fn corrupt(what: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt {
        id: "leases".into(),
        reason: what.to_string(),
    }
}

const LEASE_COLUMNS: &str = "l.id, l.scope, l.task, l.attempt, l.workspace, l.agent, l.paths, l.mode, l.reason, l.ttl_ms, l.acquired_at, l.expires_at, l.renewals, l.released_at";

fn lease_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Lease, StoreError>> {
    let id: String = r.get(0)?;
    let scope: String = r.get(1)?;
    let task: Option<String> = r.get(2)?;
    let attempt: Option<String> = r.get(3)?;
    let workspace: String = r.get(4)?;
    let agent: String = r.get(5)?;
    let paths: String = r.get(6)?;
    let mode: String = r.get(7)?;
    let reason: String = r.get(8)?;
    let ttl_ms: i64 = r.get(9)?;
    let acquired_at: String = r.get(10)?;
    let expires_at: String = r.get(11)?;
    let renewals: i64 = r.get(12)?;
    let released_at: Option<String> = r.get(13)?;
    Ok((|| {
        Ok(Lease {
            id: id.parse().map_err(corrupt)?,
            scope,
            task: task.map(|t| t.parse()).transpose().map_err(corrupt)?,
            attempt: attempt.map(|a| a.parse()).transpose().map_err(corrupt)?,
            workspace: workspace.parse().map_err(corrupt)?,
            agent: agent.parse().map_err(corrupt)?,
            paths: serde_json::from_str(&paths).map_err(corrupt)?,
            mode: match mode.as_str() {
                "exclusive" => LeaseMode::Exclusive,
                "shared" => LeaseMode::Shared,
                other => return Err(corrupt(format!("mode {other:?}"))),
            },
            reason,
            ttl_ms: ttl_ms as u64,
            acquired_at: acquired_at.parse().map_err(corrupt)?,
            expires_at: expires_at.parse().map_err(corrupt)?,
            renewals: renewals as u32,
            released_at: released_at
                .map(|t| t.parse())
                .transpose()
                .map_err(corrupt)?,
        })
    })())
}

/// Live leases in `scope`: not released, not expired, and held by an open
/// workspace whose attempt is still open and whose directory (if any) is
/// live. Expiry is enforced here, on read.
fn live_in(conn: &Connection, scope: &str) -> Result<Vec<Lease>, StoreError> {
    let sql = format!(
        "SELECT {LEASE_COLUMNS} FROM leases l
         JOIN workspaces w ON w.id = l.workspace
         LEFT JOIN attempts a ON a.id = l.attempt
         LEFT JOIN materializations m ON m.workspace = l.workspace
         WHERE l.scope = ?1 AND l.released_at IS NULL AND w.state = 'open'
           AND (a.id IS NULL OR a.state = 'open')
           AND (m.workspace IS NULL OR m.state = 'live')
         ORDER BY l.acquired_at, l.id"
    );
    let now = Timestamp::now();
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([scope], lease_row)?;
    let mut out = Vec::new();
    for row in rows {
        let lease = row??;
        if lease.expires_at > now {
            out.push(lease);
        }
    }
    Ok(out)
}

fn load_lease(conn: &Connection, id: &LeaseId) -> Result<Option<Lease>, StoreError> {
    let sql = format!("SELECT {LEASE_COLUMNS} FROM leases l WHERE l.id = ?1");
    conn.query_row(&sql, [id.to_string()], lease_row)
        .optional()?
        .transpose()
}

/// Pushes back the expiry of the live leases workspace `ws` holds: called by
/// every sync, so leases last while their holder is active.
pub(crate) fn renew(store: &Store, ws: &WorkspaceId) -> Result<(), StoreError> {
    let now = Timestamp::now();
    let due: Vec<Lease> = store.read(|conn| {
        let sql = format!(
            "SELECT {LEASE_COLUMNS} FROM leases l WHERE l.workspace = ?1 AND l.released_at IS NULL"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([ws.to_string()], lease_row)?;
        let mut out = Vec::new();
        for row in rows {
            let lease = row??;
            let fresh = lease.expires_at.unix_millis() - lease.ttl_ms as i64;
            if lease.expires_at > now && now.unix_millis() - fresh >= RENEW_AFTER_MS {
                out.push(lease);
            }
        }
        Ok(out)
    })?;
    if due.is_empty() {
        return Ok(());
    }
    store.tx(|tx| {
        for lease in &due {
            let expires = Timestamp::from_unix_millis(now.unix_millis() + lease.ttl_ms as i64)
                .map_err(corrupt)?;
            tx.sql.execute(
                "UPDATE leases SET expires_at = ?2, renewals = renewals + 1
                 WHERE id = ?1 AND released_at IS NULL AND expires_at > ?3",
                params![lease.id.to_string(), expires.to_string(), now.to_string()],
            )?;
        }
        Ok(())
    })
}

/// The coordination service.
pub struct Coord {
    store: Arc<Store>,
    workspaces: Arc<Workspaces>,
}

impl Coord {
    pub fn new(store: Arc<Store>, workspaces: Arc<Workspaces>) -> Self {
        Self { store, workspaces }
    }

    fn holder(&self, caller: &AgentId, id: &WorkspaceId) -> Result<Holder> {
        let ws = self.workspaces.get(caller, id)?;
        let m = self
            .workspaces
            .disk()
            .materialization(id)
            .map_err(WorkspaceError::from)?;
        let repo = m.and_then(|m| m.repo);
        let scope = match &repo {
            Some(repo) => repo.display().to_string(),
            None => format!("snapshot:{}", ws.base),
        };
        let task = self.store.read(|conn| task_of(conn, ws.attempt))?;
        Ok(Holder {
            ws,
            scope,
            repo,
            task,
        })
    }

    /// Whether workspace `ws`'s latest verify said `FAIL_OWN` of the files
    /// it still holds: it broke something and has not changed since, so it
    /// is no work another task should plan around. `cache` keeps answers.
    fn failed_own(&self, ws: WorkspaceId, cache: &mut BTreeMap<WorkspaceId, bool>) -> Result<bool> {
        if let Some(known) = cache.get(&ws) {
            return Ok(*known);
        }
        let row: Option<(String, Option<String>)> = self.store.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT w.updated_at, json_extract(a.receipts, '$[#-1]')
                     FROM workspaces w LEFT JOIN attempts a ON a.id = w.attempt
                     WHERE w.id = ?1",
                    [ws.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?)
        })?;
        let failed = match row {
            Some((updated_at, Some(receipt))) => {
                let updated_at: Timestamp = updated_at.parse().map_err(corrupt)?;
                let id: aver_core::ReceiptId = receipt.parse().map_err(corrupt)?;
                let receipt = self.store.get::<aver_core::Receipt>(&id)?;
                receipt.body.verdict == aver_core::Verdict::FailOwn
                    && receipt.body.created_at >= updated_at
            }
            _ => false,
        };
        cache.insert(ws, failed);
        Ok(failed)
    }

    /// Other workspaces' changes in `repo` that still speak for their
    /// tasks: open attempts' (a rerun abandons the run it replaces), and
    /// none whose latest verify said `FAIL_OWN` of what it holds.
    fn current_changes(
        &self,
        repo: &std::path::Path,
        me: &WorkspaceId,
        failed: &mut BTreeMap<WorkspaceId, bool>,
    ) -> Result<Vec<Other>> {
        let mut out = Vec::new();
        for o in self.store.read(|conn| other_changes(conn, repo, me))? {
            if !self.failed_own(o.workspace, failed)? {
                out.push(o);
            }
        }
        Ok(out)
    }

    fn title(
        &self,
        task: Option<TaskId>,
        cache: &mut BTreeMap<TaskId, Option<String>>,
    ) -> Option<String> {
        let task = task?;
        cache
            .entry(task)
            .or_insert_with(|| {
                self.store
                    .get::<Task>(&task)
                    .ok()
                    .map(|t| t.body.title.to_string())
            })
            .clone()
    }

    fn view(&self, holder: &Holder, leases: Vec<Lease>) -> Vec<HeldLease> {
        let mut titles = BTreeMap::new();
        leases
            .into_iter()
            .map(|lease| {
                let mine = lease.workspace == holder.ws.id;
                let sibling = !mine && same_task(lease.task, holder.task);
                HeldLease {
                    title: self.title(lease.task, &mut titles),
                    mine,
                    sibling,
                    lease,
                }
            })
            .collect()
    }

    /// Takes a lease for workspace `ws`, owned by `caller`. Fails with
    /// `LEASE_CONFLICT` when another task holds a live lease on an
    /// overlapping glob and either lease is exclusive.
    pub fn acquire(
        &self,
        caller: &AgentId,
        ws: &WorkspaceId,
        req: LeaseRequest,
    ) -> Result<HeldLease> {
        if req.paths.is_empty() || req.paths.len() > MAX_GLOBS {
            return Err(CoordError::Invalid(format!(
                "a lease names 1 to {MAX_GLOBS} path globs"
            )));
        }
        let reason = req.reason.trim();
        if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
            return Err(CoordError::Invalid(format!(
                "say why in 1 to {MAX_REASON_BYTES} bytes (--reason)"
            )));
        }
        let ttl_ms = req.ttl_ms.unwrap_or(DEFAULT_TTL_MS);
        if !(MIN_TTL_MS..=MAX_TTL_MS).contains(&ttl_ms) {
            return Err(CoordError::Invalid(
                "a lease lasts between 1 minute and 24 hours".into(),
            ));
        }
        let holder = self.holder(caller, ws)?;
        if holder.ws.state != crate::workspace::WorkspaceState::Open {
            return Err(CoordError::Invalid(format!(
                "workspace {ws} is {}",
                holder.ws.state.as_str()
            )));
        }
        let mut paths = req.paths;
        paths.sort();
        paths.dedup();
        let now = Timestamp::now();
        let lease = Lease {
            id: LeaseId::generate(),
            scope: holder.scope.clone(),
            task: holder.task,
            attempt: holder.ws.attempt,
            workspace: holder.ws.id,
            agent: *caller,
            paths,
            mode: req.mode,
            reason: reason.to_owned(),
            ttl_ms,
            acquired_at: now,
            expires_at: Timestamp::from_unix_millis(now.unix_millis() + ttl_ms as i64)
                .map_err(|e| CoordError::Invalid(e.to_string()))?,
            renewals: 0,
            released_at: None,
        };
        let conflict = self.store.tx(|tx| {
            let live = live_in(&tx.sql, &holder.scope)?;
            let clash = live.into_iter().find(|other| {
                other.workspace != holder.ws.id
                    && !same_task(other.task, holder.task)
                    && (other.mode == LeaseMode::Exclusive || lease.mode == LeaseMode::Exclusive)
                    && other
                        .paths
                        .iter()
                        .any(|g| lease.paths.iter().any(|h| g.overlaps(h)))
            });
            if clash.is_some() {
                return Ok::<_, CoordError>(clash);
            }
            tx.sql.execute(
                "INSERT INTO leases (id, scope, task, attempt, workspace, agent, paths, mode, reason,
                   ttl_ms, acquired_at, expires_at, renewals, released_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, NULL)",
                params![
                    lease.id.to_string(),
                    lease.scope,
                    lease.task.map(|t| t.to_string()),
                    lease.attempt.map(|a| a.to_string()),
                    lease.workspace.to_string(),
                    lease.agent.to_string(),
                    serde_json::to_string(&lease.paths).expect("globs serialize"),
                    lease.mode.as_str(),
                    lease.reason,
                    lease.ttl_ms as i64,
                    lease.acquired_at.to_string(),
                    lease.expires_at.to_string(),
                ],
            )?;
            Ok(None)
        })?;
        if let Some(other) = conflict {
            let held = self.view(&holder, vec![other]).remove(0);
            let globs: Vec<&str> = held.lease.paths.iter().map(|g| g.as_str()).collect();
            return Err(CoordError::Conflict {
                message: format!(
                    "{} {} lease on {} is held by task {} until {} ({})",
                    article(held.lease.mode),
                    held.lease.mode.as_str(),
                    globs.join(", "),
                    held.title.as_deref().map_or_else(
                        || held.lease.task.map_or("-".into(), |t| t.to_string()),
                        |t| format!("{t:?}")
                    ),
                    held.lease.expires_at,
                    held.lease.id
                ),
                holder: Box::new(held),
            });
        }
        Ok(self.view(&holder, vec![lease]).remove(0))
    }

    /// Releases lease `id`. Its holder may release it; so may an agent with
    /// `admin` (`overseer`). Releasing twice answers the released lease.
    pub fn release(&self, caller: &AgentId, id: &LeaseId, overseer: bool) -> Result<Lease> {
        self.store.tx(|tx| {
            let mut lease = load_lease(&tx.sql, id)?
                .ok_or_else(|| CoordError::NotFound(format!("lease {id} not found")))?;
            if lease.agent != *caller && !overseer {
                return Err(CoordError::Forbidden(format!(
                    "lease {id} is held by {}",
                    lease.agent
                )));
            }
            if lease.released_at.is_none() {
                let now = Timestamp::now();
                tx.sql.execute(
                    "UPDATE leases SET released_at = ?2 WHERE id = ?1",
                    params![id.to_string(), now.to_string()],
                )?;
                lease.released_at = Some(now);
            }
            Ok(lease)
        })
    }

    /// Every live lease in workspace `ws`'s repository, marked as its own,
    /// a sibling attempt's, or another task's. Does not sync.
    pub fn leases(&self, caller: &AgentId, ws: &WorkspaceId) -> Result<Vec<HeldLease>> {
        let holder = self.holder(caller, ws)?;
        let live = self.store.read(|conn| live_in(conn, &holder.scope))?;
        Ok(self.view(&holder, live))
    }

    /// The live exclusive leases of other tasks in workspace `ws`'s
    /// repository: the ones whose paths a verification of `ws` turns into
    /// `lease::` entries (§2.11). Does not sync.
    pub fn binding_exclusive(&self, caller: &AgentId, ws: &WorkspaceId) -> Result<Vec<HeldLease>> {
        Ok(self
            .leases(caller, ws)?
            .into_iter()
            .filter(|h| h.binds() && h.lease.mode == LeaseMode::Exclusive)
            .collect())
    }

    /// What other tasks are doing in workspace `ws`'s repository, against
    /// its current changes. Syncs a directory first when `sync`.
    pub fn notices(&self, caller: &AgentId, ws: &WorkspaceId, sync: bool) -> Result<Notices> {
        if sync {
            self.workspaces.sync(caller, ws)?;
        }
        let holder = self.holder(caller, ws)?;
        let live = self.store.read(|conn| live_in(conn, &holder.scope))?;
        let mut failed = BTreeMap::new();
        let mut leases = Vec::new();
        for held in self.view(&holder, live) {
            if held.binds() && !self.failed_own(held.lease.workspace, &mut failed)? {
                leases.push(held);
            }
        }
        let (_, mine) = self.workspaces.overlay_files(ws)?;
        let mut out = Notices::default();
        for (path, _) in &mine {
            for held in leases.iter().filter(|h| h.lease.covers(path.as_str())) {
                out.leased.push(LeasedPath {
                    path: path.as_str().to_owned(),
                    lease: held.lease.id,
                    task: held.lease.task,
                    title: held.title.clone(),
                    mode: held.lease.mode,
                    expires_at: held.lease.expires_at,
                });
            }
        }
        out.leases = leases;
        let Some(repo) = &holder.repo else {
            return Ok(out);
        };
        let others = self.current_changes(repo, ws, &mut failed)?;
        // By other task: its workspaces' changes, path by path.
        let mut by_task: BTreeMap<String, Group> = BTreeMap::new();
        for o in others {
            if same_task(o.task, holder.task) {
                continue;
            }
            let key = match o.task {
                Some(t) => t.to_string(),
                None => o.workspace.to_string(),
            };
            let group = by_task.entry(key).or_insert_with(|| Group {
                task: o.task,
                workspaces: BTreeSet::new(),
                paths: BTreeMap::new(),
            });
            group.workspaces.insert(o.workspace);
            group.paths.entry(o.path.clone()).or_default().push(o);
        }
        let mut titles = BTreeMap::new();
        let mine: BTreeMap<&str, Option<FileEntry>> =
            mine.iter().map(|(p, f)| (p.as_str(), *f)).collect();
        for Group {
            task,
            workspaces,
            paths,
        } in by_task.into_values()
        {
            let title = self.title(task, &mut titles);
            let changed: Vec<String> = paths.keys().cloned().collect();
            out.tasks.push(OtherTask {
                task,
                title: title.clone(),
                workspaces: workspaces.len() as u32,
                more: changed.len().saturating_sub(MAX_PATHS_PER_TASK) as u32,
                changed: changed.into_iter().take(MAX_PATHS_PER_TASK).collect(),
            });
            for (path, theirs) in &paths {
                let Some(my_file) = mine.get(path.as_str()) else {
                    continue;
                };
                let notice = self.compare(&holder.ws.base, path, *my_file, theirs)?;
                let mut versions: Vec<String> = theirs
                    .iter()
                    .map(|o| o.file.map_or("deleted".to_owned(), |f| f.blob.to_string()))
                    .collect();
                versions.sort();
                versions.dedup();
                out.notices.push(Notice {
                    path: path.clone(),
                    task,
                    title: title.clone(),
                    theirs_version: versions.join(","),
                    changed_at: theirs.iter().map(|o| o.updated_at).max(),
                    ..notice
                });
            }
        }
        out.notices
            .sort_by(|a, b| b.overlap.cmp(&a.overlap).then_with(|| a.path.cmp(&b.path)));
        Ok(out)
    }

    /// The trial merge `aver check --combined` runs on (§2.11): snapshot
    /// `mine` of workspace `ws` merged, by [`Workspaces::three_way`] under
    /// `aver combine`'s rule ([`MergeRule::UnionInsertions`]), with the
    /// current workspace of each other task in the same repository on the
    /// same base (its newest open attempt's, among those its newest update;
    /// never a stale one, [`Coord::current_changes`]), as their overlays
    /// stand (their last sync). Paths whose versions conflict keep `mine`'s. `None` when no
    /// other task works there.
    pub fn trial(
        &self,
        caller: &AgentId,
        ws: &WorkspaceId,
        mine: &SnapshotId,
    ) -> Result<Option<Trial>> {
        let holder = self.holder(caller, ws)?;
        let Some(repo) = &holder.repo else {
            return Ok(None);
        };
        let others = self.current_changes(repo, ws, &mut BTreeMap::new())?;
        /// The current workspace of one other task: its newest attempt's,
        /// then its most recently updated.
        struct Latest {
            task: Option<TaskId>,
            at: (Option<Timestamp>, Timestamp),
            changes: Vec<(Path, Option<FileEntry>)>,
        }
        let mut latest: BTreeMap<String, Latest> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for o in &others {
            if same_task(o.task, holder.task)
                || o.base != holder.ws.base
                || !seen.insert(o.workspace)
            {
                continue;
            }
            let (row, changes) = self.workspaces.overlay_files(&o.workspace)?;
            let key = o
                .task
                .map_or_else(|| o.workspace.to_string(), |t| t.to_string());
            let at = (o.started, row.updated_at);
            if latest.get(&key).is_none_or(|l| l.at < at) {
                latest.insert(
                    key,
                    Latest {
                        task: o.task,
                        at,
                        changes,
                    },
                );
            }
        }
        if latest.is_empty() {
            return Ok(None);
        }
        let base = self.workspaces.get_snapshot(&holder.ws.base)?;
        let mut titles = BTreeMap::new();
        let with: Vec<String> = latest
            .values()
            .map(|l| task_name(self.title(l.task, &mut titles).as_deref(), l.task))
            .collect();
        let merge = |skip: &BTreeSet<String>| -> Result<crate::workspace::merge::Merged> {
            let mut heads = vec![*mine];
            for Latest { changes, .. } in latest.values() {
                let kept: Vec<(Path, Option<FileEntry>)> = changes
                    .iter()
                    .filter(|(p, _)| !skip.contains(p.as_str()))
                    .cloned()
                    .collect();
                heads.push(self.workspaces.derive_snapshot(
                    &base.root,
                    &kept,
                    crate::store::pinned_env(&base)?,
                )?);
            }
            Ok(self
                .workspaces
                .three_way(&holder.ws.base, &heads, MergeRule::UnionInsertions)?)
        };
        let first = merge(&BTreeSet::new())?;
        let conflicts: BTreeSet<String> = first.conflicts.iter().map(|c| c.path.clone()).collect();
        let merged = if conflicts.is_empty() {
            first
        } else {
            merge(&conflicts)?
        };
        let tree = self.workspaces.derive_snapshot(
            &base.root,
            &merged.changes,
            crate::store::pinned_env(&base)?,
        )?;
        Ok(Some(Trial {
            tree,
            with,
            conflicts: conflicts.into_iter().collect(),
            merged: merged.merged,
        }))
    }

    /// The orchestrator view of repository `repo` (its root, as workspace
    /// directories record it): every task with live directories there,
    /// what each changed, an edge for each pair of tasks that changed the
    /// same files (compared between each task's most recently updated
    /// workspace, as [`Coord::notices`] compares), and every live lease.
    /// Needs `admin`; syncs nothing.
    pub fn repo_view(&self, caller: &AgentId, repo: &std::path::Path) -> Result<RepoView> {
        let admin = crate::agents::get(&self.store, caller)
            .map_err(|e| CoordError::Invalid(e.to_string()))?
            .is_some_and(|a| !a.is_revoked() && a.can(crate::agents::Capability::Admin));
        if !admin {
            return Err(CoordError::Forbidden(
                "the repository view needs the admin capability".into(),
            ));
        }
        // Directories record their repository canonical.
        let canonical = std::fs::canonicalize(repo).ok();
        let repo = canonical.as_deref().unwrap_or(repo);
        let scope = repo.display().to_string();
        let (rows, live) = self
            .store
            .read(|conn| Ok((live_workspaces(conn, repo)?, live_in(conn, &scope)?)))?;
        /// One task's workspaces, the paths they changed, and its latest.
        struct ByTask {
            task: Option<TaskId>,
            workspaces: Vec<WorkspaceId>,
            changed: BTreeSet<String>,
            latest: Option<(Timestamp, Workspace, BTreeMap<String, Option<FileEntry>>)>,
        }
        let mut by_task: BTreeMap<String, ByTask> = BTreeMap::new();
        for (ws, task) in rows {
            let (row, changes) = self.workspaces.overlay_files(&ws)?;
            let key = task.map_or_else(|| ws.to_string(), |t| t.to_string());
            let entry = by_task.entry(key).or_insert_with(|| ByTask {
                task,
                workspaces: Vec::new(),
                changed: BTreeSet::new(),
                latest: None,
            });
            entry.workspaces.push(ws);
            entry
                .changed
                .extend(changes.iter().map(|(p, _)| p.as_str().to_owned()));
            if entry
                .latest
                .as_ref()
                .is_none_or(|(at, _, _)| *at < row.updated_at)
            {
                let files = changes
                    .into_iter()
                    .map(|(p, f)| (p.as_str().to_owned(), f))
                    .collect();
                entry.latest = Some((row.updated_at, row, files));
            }
        }
        let tasks: Vec<ByTask> = by_task.into_values().collect();
        let mut titles = BTreeMap::new();
        let mut edges = Vec::new();
        for (i, a) in tasks.iter().enumerate() {
            for b in &tasks[i + 1..] {
                let (Some((_, wa, fa)), Some((_, wb, fb))) = (&a.latest, &b.latest) else {
                    continue;
                };
                let mut paths = Vec::new();
                for (path, mine) in fa {
                    let Some(theirs) = fb.get(path) else {
                        continue;
                    };
                    let other = Other {
                        workspace: wb.id,
                        base: wb.base,
                        updated_at: wb.updated_at,
                        started: None,
                        task: b.task,
                        path: path.clone(),
                        file: *theirs,
                    };
                    let n = self.compare(&wa.base, path, *mine, &[other])?;
                    paths.push(EdgePath {
                        path: path.clone(),
                        overlap: n.overlap,
                        merge: n.merge,
                    });
                }
                let Some(worst) = paths.iter().map(|p| p.overlap).max() else {
                    continue;
                };
                edges.push(OverlapEdge {
                    a: a.task,
                    b: b.task,
                    worst,
                    paths,
                });
            }
        }
        edges.sort_by_key(|e| std::cmp::Reverse(e.worst));
        let leases = live
            .into_iter()
            .map(|lease| HeldLease {
                title: self.title(lease.task, &mut titles),
                mine: false,
                sibling: false,
                lease,
            })
            .collect();
        let tasks = tasks
            .into_iter()
            .map(|t| RepoTask {
                title: self.title(t.task, &mut titles),
                task: t.task,
                workspaces: t.workspaces,
                more: t.changed.len().saturating_sub(MAX_REPO_PATHS) as u32,
                changed: t.changed.into_iter().take(MAX_REPO_PATHS).collect(),
            })
            .collect();
        Ok(RepoView {
            repo: repo.to_owned(),
            tasks,
            edges,
            leases,
        })
    }

    /// How other workspaces' versions of `path` relate to this one's.
    fn compare(
        &self,
        base: &SnapshotId,
        path: &str,
        mine: Option<FileEntry>,
        theirs: &[Other],
    ) -> Result<Notice> {
        let parsed: Path = path
            .parse()
            .map_err(|e| CoordError::Invalid(format!("{path}: {e}")))?;
        let base_file = self.workspaces.file_in_snapshot(base, &parsed)?;
        let text_of = |file: Option<FileEntry>| -> Result<Option<String>> {
            let Some(file) = file.filter(|f| f.kind != FileKind::Symlink) else {
                return Ok(None);
            };
            let bytes = self.store.get_raw(&file.blob)?;
            Ok(String::from_utf8(bytes).ok().filter(|t| !t.contains('\0')))
        };
        let file_level = Notice {
            path: path.to_owned(),
            task: None,
            title: None,
            overlap: Overlap::File,
            mine: Vec::new(),
            theirs: Vec::new(),
            merge: None,
            diff: None,
            theirs_version: String::new(),
            changed_at: None,
            changing: false,
        };
        let (Some(b), Some(m)) = (text_of(base_file)?, text_of(mine)?) else {
            return Ok(file_level);
        };
        let my_hunks = hunks(&b, &m);
        let my_lines: Vec<Lines> = my_hunks.iter().map(|h| h.base).collect();
        let mut their_lines = Vec::new();
        let mut versions = Vec::new();
        for other in theirs {
            if other.base != *base {
                return Ok(file_level);
            }
            let Some(t) = text_of(other.file)? else {
                return Ok(file_level);
            };
            their_lines.extend(hunks(&b, &t).into_iter().map(|h| h.base));
            versions.push(t);
        }
        their_lines.sort_by_key(|l| (l.start, l.end));
        their_lines.dedup();
        let overlap = if my_lines
            .iter()
            .any(|a| their_lines.iter().any(|b| a.meets(b)))
        {
            Overlap::Lines
        } else {
            Overlap::Disjoint
        };
        let merge = versions
            .iter()
            .map(|t| trial_merge(&b, &m, t))
            .max_by_key(|m| match m {
                Merge::Clean => 0,
                Merge::SamePlace => 1,
                Merge::Conflict => 2,
            });
        let diff = (overlap == Overlap::Lines)
            .then(|| meeting_diff(&b, &m, &my_hunks, &versions))
            .filter(|d| !d.is_empty());
        Ok(Notice {
            overlap,
            mine: my_lines,
            theirs: their_lines,
            merge,
            diff,
            ..file_level
        })
    }
}

/// What merging `mine` and `theirs`, two versions of `base`, would do. The
/// strict rule keeps insertions at one point apart as [`Merge::SamePlace`].
fn trial_merge(base: &str, mine: &str, theirs: &str) -> Merge {
    match merge_text(base, &[mine, theirs], MergeRule::Strict) {
        Ok(_) => Merge::Clean,
        Err(conflicts) => {
            let insertions = conflicts
                .iter()
                .all(|c| c.sides.iter().all(|s| s.base.start == s.base.end));
            if insertions {
                Merge::SamePlace
            } else {
                Merge::Conflict
            }
        }
    }
}

/// The other versions' hunks that meet one of `mine`'s, each followed by
/// the hunks of `mine` it meets, bounded by [`MAX_DIFF_HUNKS`] and
/// [`MAX_HUNK_LINES`]. Repository content.
fn meeting_diff(base: &str, mine: &str, my_hunks: &[Hunk], theirs: &[String]) -> String {
    let base_lines: Vec<&str> = base.split_inclusive('\n').collect();
    let my_text: Vec<&str> = mine.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut seen: Vec<String> = Vec::new();
    let mut shown = 0;
    'versions: for version in theirs {
        let their_text: Vec<&str> = version.split_inclusive('\n').collect();
        for h in hunks(base, version) {
            let meets: Vec<&Hunk> = my_hunks.iter().filter(|m| m.base.meets(&h.base)).collect();
            if meets.is_empty() {
                continue;
            }
            let theirs = hunk_lines(&base_lines, &their_text, &h);
            if seen.contains(&theirs) {
                continue;
            }
            if shown == MAX_DIFF_HUNKS {
                out.push_str("  … more overlapping hunks\n");
                break 'versions;
            }
            shown += 1;
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!("  theirs, at {}:\n{theirs}", h.base.human()),
            );
            seen.push(theirs);
            for m in meets.iter().take(MAX_DIFF_HUNKS) {
                let _ = std::fmt::Write::write_fmt(
                    &mut out,
                    format_args!(
                        "  yours, at {}:\n{}",
                        m.base.human(),
                        hunk_lines(&base_lines, &my_text, m)
                    ),
                );
            }
        }
    }
    out
}

/// One hunk as `-` lines of the base and `+` lines of the head, each side
/// cut at [`MAX_HUNK_LINES`], indented under its header.
fn hunk_lines(base: &[&str], head: &[&str], h: &Hunk) -> String {
    let mut out = String::new();
    let side = |out: &mut String, sign: char, lines: &[&str]| {
        for line in lines.iter().take(MAX_HUNK_LINES) {
            let line = line.trim_end_matches(['\n', '\r']);
            let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
            let more = if cut.len() < line.len() { "…" } else { "" };
            let cut = crate::run::report::one_line(&cut);
            out.push_str(&format!("    {sign}{cut}{more}\n"));
        }
        if lines.len() > MAX_HUNK_LINES {
            out.push_str(&format!(
                "    {sign}… {} more lines\n",
                lines.len() - MAX_HUNK_LINES
            ));
        }
    };
    let range = |lines: &[&str], l: &Lines| -> Vec<String> {
        let (s, e) = (l.start as usize, (l.end as usize).min(lines.len()));
        lines[s.min(e)..e].iter().map(|s| (*s).to_owned()).collect()
    };
    let removed = range(base, &h.base);
    let added = range(head, &h.head);
    let removed: Vec<&str> = removed.iter().map(String::as_str).collect();
    let added: Vec<&str> = added.iter().map(String::as_str).collect();
    side(&mut out, '-', &removed);
    side(&mut out, '+', &added);
    out
}

fn article(mode: LeaseMode) -> &'static str {
    match mode {
        LeaseMode::Exclusive => "an",
        LeaseMode::Shared => "a",
    }
}

/// Another task's live workspaces and what they changed, path by path.
struct Group {
    task: Option<TaskId>,
    workspaces: BTreeSet<WorkspaceId>,
    paths: BTreeMap<String, Vec<Other>>,
}

/// One overlay row of another live disk workspace in the repository.
struct Other {
    workspace: WorkspaceId,
    base: SnapshotId,
    /// When that workspace last recorded a change.
    updated_at: Timestamp,
    /// When its attempt started.
    started: Option<Timestamp>,
    task: Option<TaskId>,
    path: String,
    file: Option<FileEntry>,
}

fn other_changes(
    conn: &Connection,
    repo: &std::path::Path,
    me: &WorkspaceId,
) -> Result<Vec<Other>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT o.workspace, w.base, a.task, o.path, o.kind, o.blob, w.updated_at, a.created_at
         FROM overlay o
         JOIN materializations m ON m.workspace = o.workspace
         JOIN workspaces w ON w.id = o.workspace
         LEFT JOIN attempts a ON a.id = w.attempt
         WHERE m.repo = ?1 AND m.state = ?2 AND w.state = 'open' AND o.workspace != ?3
           AND (a.id IS NULL OR a.state = 'open')
         ORDER BY o.workspace, o.path",
    )?;
    let rows = stmt.query_map(
        params![
            repo.display().to_string(),
            MatState::Live.as_str(),
            me.to_string()
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (ws, base, task, path, kind, blob, updated_at, started) = row?;
        let kind = match kind.as_str() {
            "delete" => None,
            "file" => Some(FileKind::File),
            "exec" => Some(FileKind::Exec),
            "symlink" => Some(FileKind::Symlink),
            other => return Err(corrupt(format!("overlay kind {other:?}"))),
        };
        let file = match (kind, blob) {
            (Some(kind), Some(blob)) => Some(FileEntry {
                kind,
                blob: blob.parse::<BlobId>().map_err(corrupt)?,
            }),
            (None, _) => None,
            (Some(_), None) => return Err(corrupt("overlay row without a blob")),
        };
        out.push(Other {
            workspace: ws.parse().map_err(corrupt)?,
            base: base.parse().map_err(corrupt)?,
            updated_at: updated_at.parse().map_err(corrupt)?,
            started: started.map(|t| t.parse()).transpose().map_err(corrupt)?,
            task: task.map(|t| t.parse()).transpose().map_err(corrupt)?,
            path,
            file,
        });
    }
    Ok(out)
}

/// The open workspaces with a live directory made from repository `repo`,
/// with their tasks, oldest first.
fn live_workspaces(
    conn: &Connection,
    repo: &std::path::Path,
) -> Result<Vec<(WorkspaceId, Option<TaskId>)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.workspace, a.task
         FROM materializations m
         JOIN workspaces w ON w.id = m.workspace
         LEFT JOIN attempts a ON a.id = w.attempt
         WHERE m.repo = ?1 AND m.state = ?2 AND w.state = 'open'
         ORDER BY m.created_at, m.workspace",
    )?;
    let rows = stmt.query_map(
        params![repo.display().to_string(), MatState::Live.as_str()],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (ws, task) = row?;
        out.push((
            ws.parse().map_err(corrupt)?,
            task.map(|t| t.parse()).transpose().map_err(corrupt)?,
        ));
    }
    Ok(out)
}

/// `in 23 min`, `in 40 s`, or `now`.
pub fn expires_in(at: Timestamp) -> String {
    let ms = at.unix_millis() - Timestamp::now().unix_millis();
    if ms <= 0 {
        "now".into()
    } else if ms < 60_000 {
        format!("in {} s", ms / 1000)
    } else {
        format!("in {} min", ms / 60_000)
    }
}

fn task_name(title: Option<&str>, task: Option<TaskId>) -> String {
    match (title, task) {
        (Some(t), _) => format!("{:?}", crate::run::report::one_line_plain(t)),
        (None, Some(t)) => t.to_string(),
        (None, None) => "a workspace without a task".into(),
    }
}

fn ranges(lines: &[Lines]) -> String {
    let parts: Vec<String> = lines.iter().map(Lines::human).collect();
    parts.join(", ")
}

/// The most bytes of notices one hook, verify or check output carries
/// (§2.11); `aver notices` prints everything.
pub const MAX_NOTICE_BYTES: usize = 2048;

/// Notices as rendered, and the items the text shows (for the seen-set).
pub struct Rendered {
    pub text: String,
    pub shown: Vec<NoticeKey>,
}

/// The notices for an agent: one fenced block of lines naming paths, tasks
/// and reasons (repository and agent text, §6.4), then what to do about
/// them. Empty when there is nothing to say. With `full`, also lists what
/// other tasks change and lease even where this workspace has not changed
/// anything yet (the brief, `aver notices`); otherwise only what touches its
/// own changes (verify, `aver check`), within [`MAX_NOTICE_BYTES`].
pub fn agent_text(n: &Notices, full: bool) -> String {
    render(n, full).text
}

/// [`agent_text`], with the items it shows. One wording for every surface:
/// verify, `aver check` and `aver notices` all print this.
pub fn render(n: &Notices, full: bool) -> Rendered {
    use crate::run::report::{fence, one_line};
    // Each item: its key, its line and the diff under it.
    let mut items: Vec<(NoticeKey, String, Option<String>)> = Vec::new();
    let (mut conflict, mut same_place, mut file, mut leased_any) = (false, false, false, false);
    for notice in &n.notices {
        let who = task_name(notice.title.as_deref(), notice.task);
        let path = one_line(&notice.path);
        let merge = match notice.merge {
            Some(Merge::Clean) => "; merging both is clean",
            Some(Merge::Conflict) => "; merging both CONFLICTS",
            Some(Merge::SamePlace) => "; you both add lines at the same place: combine keeps both",
            None => "",
        };
        conflict |= matches!(notice.merge, Some(Merge::Conflict));
        same_place |= matches!(notice.merge, Some(Merge::SamePlace));
        let mut line = match notice.overlap {
            Overlap::Lines => format!(
                "OVERLAP {path}: task {who} changed {}; you changed {}{merge}",
                ranges(&notice.theirs),
                ranges(&notice.mine)
            ),
            Overlap::File => {
                file = true;
                format!("{path}: task {who} also changes this file (lines cannot be compared)")
            }
            Overlap::Disjoint => format!(
                "{path}: task {who} changed {}; you changed {} (no overlap{merge})",
                ranges(&notice.theirs),
                ranges(&notice.mine)
            ),
        };
        if notice.changing {
            line.push_str(" (still changing: they edited within the last minute)");
        }
        let diff = notice
            .diff
            .as_ref()
            .map(|d| d.trim_end_matches('\n').to_owned());
        items.push((notice.key(), line, diff));
    }
    // A notice from before trial merges said only that lines overlap.
    let overlap = n
        .notices
        .iter()
        .any(|x| x.overlap == Overlap::Lines && x.merge.is_none());
    for leased in &n.leased {
        leased_any = true;
        items.push((
            leased.key(),
            format!(
                "LEASED {}: task {} holds {} {} lease ({}, expires {}); you changed it anyway",
                one_line(&leased.path),
                task_name(leased.title.as_deref(), leased.task),
                article(leased.mode),
                leased.mode.as_str(),
                leased.lease,
                expires_in(leased.expires_at)
            ),
            None,
        ));
    }
    let mut extra = Vec::new();
    if full {
        for task in &n.tasks {
            let mut paths: Vec<String> = task.changed.iter().map(|p| one_line(p)).collect();
            if task.more > 0 {
                paths.push(format!("and {} more", task.more));
            }
            extra.push(format!(
                "task {} changes {}",
                task_name(task.title.as_deref(), task.task),
                if paths.is_empty() {
                    "nothing yet".to_owned()
                } else {
                    paths.join(", ")
                }
            ));
        }
        for held in &n.leases {
            let globs: Vec<String> = held
                .lease
                .paths
                .iter()
                .map(|g| one_line(g.as_str()))
                .collect();
            extra.push(format!(
                "task {} holds {} {} lease on {} (expires {}): {}",
                task_name(held.title.as_deref(), held.lease.task),
                article(held.lease.mode),
                held.lease.mode.as_str(),
                globs.join(", "),
                expires_in(held.lease.expires_at),
                one_line(&held.lease.reason)
            ));
        }
    }
    let mut advice = String::new();
    if overlap || conflict {
        advice.push_str("CONFLICTS: merging your edits with theirs conflicts where shown; keep your change there minimal, move it away from their lines (new code in a new function or file merges cleanly), or leave that code to them. `aver check --combined` runs your tests on a trial merge with their current work.\n");
    }
    if same_place {
        advice.push_str("SAME PLACE: you both only add lines at the same spot; `aver combine` keeps both, in task order, but a rebase onto the other's work marks a conflict there.\n");
    }
    if file {
        advice.push_str("Where lines cannot be compared, keep your change to that file small.\n");
    }
    if leased_any {
        advice.push_str("LEASED: undo it with `aver restore <path>` unless you must change it.\n");
    }
    let unchanged = (n.unchanged > 0).then(|| {
        format!(
            "{} {} unchanged (`aver notices`).\n",
            n.unchanged,
            if n.unchanged == 1 {
                "overlap"
            } else {
                "overlaps"
            }
        )
    });
    const HEADER: &str = "Other tasks in this repository:\n";
    // What the text holds besides the items: the header, the fences (at
    // most a few ticks each), the advice, the unchanged and omitted lines,
    // and a margin.
    let fixed = HEADER.len() + 16 + advice.len() + unchanged.as_ref().map_or(0, String::len) + 72;
    let mut budget = if full {
        usize::MAX
    } else {
        MAX_NOTICE_BYTES.saturating_sub(fixed)
    };
    let mut lines = Vec::new();
    let mut shown = Vec::new();
    let mut omitted = 0;
    for (key, line, diff) in items {
        let with_diff = line.len() + 1 + diff.as_ref().map_or(0, |d| d.len() + 1);
        if with_diff <= budget {
            budget -= with_diff;
            lines.push(line);
            lines.extend(diff);
        } else if line.len() < budget {
            budget -= line.len() + 1;
            lines.push(line);
        } else {
            omitted += 1;
            continue;
        }
        shown.push(key);
    }
    lines.extend(extra);
    if lines.is_empty() {
        let text = match unchanged {
            Some(u) if !full => format!("Other tasks: {u}"),
            _ => String::new(),
        };
        return Rendered { text, shown };
    }
    let mut out = String::from(HEADER);
    out.push_str(&fence(&lines.join("\n")));
    if omitted > 0 {
        out.push_str(&format!(
            "… and {omitted} more (`aver notices` lists them all).\n"
        ));
    }
    out.push_str(&advice);
    if let Some(u) = unchanged {
        out.push_str(&u);
    }
    Rendered { text: out, shown }
}

/// The orchestrator view as `aver notices --repo` prints it: one fenced
/// block naming tasks, paths, leases and reasons (repository and agent
/// text, §6.4), then what to do about the overlaps. Never empty.
pub fn repo_text(view: &RepoView) -> String {
    use crate::run::report::{fence, one_line};
    let mut titles: BTreeMap<Option<TaskId>, String> = BTreeMap::new();
    let mut lines = Vec::new();
    for t in &view.tasks {
        let name = task_name(t.title.as_deref(), t.task);
        titles.insert(t.task, name.clone());
        let mut paths: Vec<String> = t.changed.iter().map(|p| one_line(p)).collect();
        if t.more > 0 {
            paths.push(format!("and {} more", t.more));
        }
        lines.push(format!(
            "task {name} ({} {}) changes {}",
            t.workspaces.len(),
            if t.workspaces.len() == 1 {
                "workspace"
            } else {
                "workspaces"
            },
            if paths.is_empty() {
                "nothing yet".to_owned()
            } else {
                paths.join(", ")
            }
        ));
    }
    let mut conflict = false;
    for edge in &view.edges {
        let name = |t: Option<TaskId>| {
            titles
                .get(&t)
                .cloned()
                .unwrap_or_else(|| task_name(None, t))
        };
        let pair = format!("tasks {} and {}", name(edge.a), name(edge.b));
        for p in &edge.paths {
            let merge = match p.merge {
                Some(Merge::Clean) => "; merging both is clean",
                Some(Merge::Conflict) => "; merging both CONFLICTS",
                Some(Merge::SamePlace) => {
                    "; merging both CONFLICTS: both add lines at the same place"
                }
                None => "",
            };
            conflict |= matches!(p.merge, Some(Merge::Conflict | Merge::SamePlace));
            let path = one_line(&p.path);
            lines.push(match p.overlap {
                Overlap::Lines => format!("OVERLAP {path}: {pair} change the same lines{merge}"),
                Overlap::Disjoint => format!("{path}: {pair} change different lines{merge}"),
                Overlap::File => {
                    format!("{path}: {pair} both change it (lines cannot be compared)")
                }
            });
        }
    }
    for held in &view.leases {
        let globs: Vec<String> = held
            .lease
            .paths
            .iter()
            .map(|g| one_line(g.as_str()))
            .collect();
        lines.push(format!(
            "task {} holds {} {} lease on {} ({}, expires {}): {}",
            task_name(held.title.as_deref(), held.lease.task),
            article(held.lease.mode),
            held.lease.mode.as_str(),
            globs.join(", "),
            held.lease.id,
            expires_in(held.lease.expires_at),
            one_line(&held.lease.reason)
        ));
    }
    if lines.is_empty() {
        return format!(
            "No task has a live workspace directory in {} now.\n",
            view.repo.display()
        );
    }
    let mut out = format!("Tasks working in {}:\n", view.repo.display());
    out.push_str(&fence(&lines.join("\n")));
    if conflict {
        out.push_str("CONFLICTS: these tasks' changes do not merge as they stand. Let one finish and accept it, then start the other on its branch, or tell one agent to move its change away from the other's lines.\n");
    }
    if !view.leases.is_empty() {
        out.push_str("`aver release --repo <lease id>` frees a lease.\n");
    }
    out
}
