//! Workspace service (ARCHITECTURE.md §1.8, §2.3).
//!
//! A workspace is a mutable row plus a copy-on-write overlay of file-level
//! changes over an immutable base snapshot. Opening inserts one row. Reads
//! resolve a path against the overlay, then the base tree (through an
//! in-memory cache of parsed trees). An edit batch is validated and applied in
//! one transaction, guarded by `if_version`, and every
//! byte it writes passes the secret scanner. The overlay always describes a
//! valid tree: no path is both a file and a directory, and rows that would
//! restore the base state are dropped.
//!
//! [`Workspaces::snapshot`] only creates content-addressed objects and returns
//! the base snapshot when nothing changed. [`Workspaces::commit`] writes the
//! snapshot, a server-signed `workspace` transition, the lineage index and
//! the committed state in one transaction.
//! Every operation except [`Workspaces::open`] is restricted to the owner.

mod cache;
pub mod checkpoint;
mod diff;
mod disk;
mod edit;
pub mod merge;
pub mod rebase;
mod search;
mod tree;

use std::sync::Arc;

use aver_core::{
    AgentId, AttemptId, BlobId, Environment, Node, Object, Path, Schema, SigningKey, Snapshot,
    SnapshotId, Timestamp, Transition, TransitionBody, TransitionId, Tree, TreeEntry, TreeId, Via,
    WorkspaceId,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::disk::{Disk, DiskConfig};
use crate::git::record_transition;
use crate::secrets::SecretFinding;
use crate::store::{Store, StoreError, Tx};
use cache::ObjectCache;
pub use tree::ChangedFile;
use tree::{diff_files, merge, node_at, Trie};

pub use diff::{DiffChange, FileDiff};
pub use disk::{MaterializeOptions, WorkspaceFile};
pub use edit::{Changed, EditOp, EditRequest};
pub use search::{
    Glob, SearchMatch, SearchOutcome, SearchRequest, WalkEntry, WalkOutcome, WalkRequest,
};

const TREE_CACHE_ENTRIES: usize = 16_384;
const SNAPSHOT_CACHE_ENTRIES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Exec,
    Symlink,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Exec => "exec",
            Self::Symlink => "symlink",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "exec" => Some(Self::Exec),
            "symlink" => Some(Self::Symlink),
            _ => None,
        }
    }
}

/// A file's kind and contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileEntry {
    pub kind: FileKind,
    pub blob: BlobId,
}

impl FileEntry {
    pub fn of_node(node: &Node) -> Option<Self> {
        let (kind, blob) = match *node {
            Node::File(blob) => (FileKind::File, blob),
            Node::Exec(blob) => (FileKind::Exec, blob),
            Node::Symlink(blob) => (FileKind::Symlink, blob),
            Node::Tree(_) => return None,
        };
        Some(Self { kind, blob })
    }

    pub fn node(&self) -> Node {
        match self.kind {
            FileKind::File => Node::File(self.blob),
            FileKind::Exec => Node::Exec(self.blob),
            FileKind::Symlink => Node::Symlink(self.blob),
        }
    }
}

/// A path and its file entry, or `None` for a delete.
pub type FileChange = (Path, Option<FileEntry>);

/// One overlay row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Put(FileEntry),
    Delete,
}

impl Change {
    fn file(&self) -> Option<FileEntry> {
        match self {
            Self::Put(file) => Some(*file),
            Self::Delete => None,
        }
    }
}

/// Derives a snapshot's environment from its tree. `Services` uses
/// [`crate::env::TreeEnv`]; tests may use stubs.
pub trait EnvResolver: Send + Sync {
    /// Whether changing `path` can change the environment of a tree whose
    /// base environment is `base`. Snapshots that change no such path keep
    /// the base environment without calling [`EnvResolver::resolve`].
    ///
    /// The root `package-lock.json` always counts: without a `lockfile` key
    /// it is picked up by default (§1.4.1), so adding it to a lockfile-less
    /// base changes the environment just as an import of that tree would.
    /// So does the root `package.json`, which the default check is read from,
    /// every layer's lockfile, and every file generic mode reads to find
    /// packages ([`crate::presets::may_change_detection`]).
    fn is_input(&self, path: &Path, base: &Environment) -> bool {
        matches!(
            path.as_str(),
            "aver.toml" | "package-lock.json" | "package.json"
        ) || base.lockfile.as_ref().is_some_and(|l| &l.path == path)
            || base.layers.get(path.as_str()).is_some()
            || crate::presets::may_change_detection(path.as_str())
    }

    /// The environment for the tree `root`, which is already in `store`.
    fn resolve(
        &self,
        store: &Store,
        root: &TreeId,
        base: &Environment,
    ) -> Result<Environment, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceState {
    Open,
    Committed,
    Discarded,
}

impl WorkspaceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Committed => "committed",
            Self::Discarded => "discarded",
        }
    }
}

/// Who writes a workspace's overlay (§1.8): edit batches (`api`) or its
/// materialized directory (`disk`). A workspace becomes `disk` only at
/// version 0 and never changes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMode {
    Api,
    Disk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub base: SnapshotId,
    pub owner: AgentId,
    pub attempt: Option<AttemptId>,
    pub state: WorkspaceState,
    pub mode: WorkspaceMode,
    pub version: u64,
    pub head: Option<SnapshotId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A file read from a workspace or snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: Path,
    pub file: FileEntry,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditOutcome {
    pub version: u64,
    pub changed: Vec<Changed>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    pub message: String,
    #[serde(default)]
    pub if_version: Option<u64>,
    /// Commit only if the directory's changes still make this snapshot
    /// (`aver accept` passes the verified one), else `SNAPSHOT_MOVED`.
    #[serde(default)]
    pub if_snapshot: Option<SnapshotId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitOutcome {
    pub workspace: Workspace,
    pub snapshot: SnapshotId,
    pub transition: TransitionId,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("{0}")]
    NotFound(String),
    #[error("workspace {workspace} belongs to {owner}")]
    NotOwner {
        workspace: WorkspaceId,
        owner: AgentId,
    },
    #[error("workspace {workspace} is {}", state.as_str())]
    Closed {
        workspace: WorkspaceId,
        state: WorkspaceState,
    },
    #[error("workspace is at version {actual}, not {expected}")]
    StaleVersion { expected: u64, actual: u64 },
    #[error("{0}")]
    PathConflict(String),
    #[error("{0} is a directory")]
    NotAFile(Path),
    #[error("{0} is not a directory")]
    NotADirectory(Path),
    #[error("{0}")]
    Secret(SecretFinding),
    #[error("{0}")]
    Invalid(String),
    #[error("file of {0} bytes exceeds the {max}-byte limit", max = aver_core::MAX_BLOB_BYTES)]
    BlobTooLarge(usize),
    #[error("workspace has no changes to commit")]
    NoChanges,
    #[error(
        "the workspace's changes make {actual}, not the expected {expected}: it changed since"
    )]
    SnapshotMoved {
        expected: SnapshotId,
        actual: SnapshotId,
    },
    #[error("environment: {0}")]
    Env(String),
    #[error("this workspace is materialized: edit the files in {dir} instead")]
    Materialized { dir: String },
    #[error("only a workspace at version 0 can be materialized (it is at version {0})")]
    NotFresh(u64),
    /// A directory holds changes no snapshot keeps, so it stays.
    #[error("{0}")]
    DirChanged(String),
    #[error(transparent)]
    Disk(crate::disk::DiskError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for WorkspaceError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e.into())
    }
}

impl WorkspaceError {
    /// The wire error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "NOT_FOUND",
            Self::NotOwner { .. } => "FORBIDDEN",
            Self::Closed { .. } => "WORKSPACE_CLOSED",
            Self::StaleVersion { .. } => "STALE_VERSION",
            Self::PathConflict(_) => "PATH_CONFLICT",
            Self::NotAFile(_) => "NOT_A_FILE",
            Self::NotADirectory(_) => "NOT_A_DIRECTORY",
            Self::Secret(_) => "SECRET_DETECTED",
            Self::Invalid(_) => "INVALID",
            Self::BlobTooLarge(_) => "TOO_LARGE",
            Self::NoChanges => "NO_CHANGES",
            Self::SnapshotMoved { .. } => "SNAPSHOT_MOVED",
            Self::Env(_) => "ENV_UNRESOLVED",
            Self::Materialized { .. } => "MATERIALIZED",
            Self::NotFresh(_) => "NOT_FRESH",
            Self::DirChanged(_) => "DIR_CHANGED",
            Self::Disk(e) => e.code(),
            Self::Store(StoreError::BlobTooLarge(_)) => "TOO_LARGE",
            Self::Store(StoreError::NotFound(_)) => "NOT_FOUND",
            Self::Store(_) => "INTERNAL",
        }
    }
}

pub type Result<T, E = WorkspaceError> = std::result::Result<T, E>;

/// A commit computed by [`Workspaces::prepare_commit`], not yet written.
pub(crate) struct PendingCommit {
    ws: Workspace,
    prepared: Prepared,
    transition: Transition,
}

impl PendingCommit {
    /// The snapshot the commit will write.
    pub(crate) fn head(&self) -> SnapshotId {
        self.transition.body.to
    }
}

/// The prepared result of applying a workspace's overlay.
struct Prepared {
    snapshot: Snapshot,
    /// A new environment object to store with the snapshot.
    env: Option<Environment>,
}

pub struct Workspaces {
    store: Arc<Store>,
    server_key: Arc<SigningKey>,
    env: Arc<dyn EnvResolver>,
    trees: ObjectCache<Tree>,
    snapshots: ObjectCache<Snapshot>,
    disk: Arc<Disk>,
}

impl Workspaces {
    /// The service over `store`, with disk-mode workspaces under the
    /// store's directory (`$AVER_HOME`).
    pub fn new(store: Arc<Store>, server_key: Arc<SigningKey>, env: Arc<dyn EnvResolver>) -> Self {
        let root = store.path().parent().map_or_else(
            || std::path::PathBuf::from("."),
            std::path::Path::to_path_buf,
        );
        let disk = Arc::new(Disk::new(store.clone(), DiskConfig::under(root)));
        Self {
            store,
            server_key,
            env,
            trees: ObjectCache::new(TREE_CACHE_ENTRIES),
            snapshots: ObjectCache::new(SNAPSHOT_CACHE_ENTRIES),
            disk,
        }
    }

    /// Replaces the disk configuration (tests force clone strategies, case
    /// folding and size limits).
    pub fn with_disk_config(mut self, config: DiskConfig) -> Self {
        self.disk = Arc::new(Disk::new(self.store.clone(), config));
        self
    }

    /// The host side of disk-mode workspaces.
    pub fn disk(&self) -> &Arc<Disk> {
        &self.disk
    }

    /// Opens a workspace on `base` owned by `owner`: one row, no copying.
    pub fn open(&self, owner: &AgentId, base: &SnapshotId) -> Result<Workspace> {
        self.store.tx(|tx| self.open_in(tx, owner, base, None))
    }

    /// Opens a workspace inside `tx`, optionally bound to the attempt that
    /// owns it (an attempt and its workspace are created together).
    pub(crate) fn open_in(
        &self,
        tx: &Tx<'_>,
        owner: &AgentId,
        base: &SnapshotId,
        attempt: Option<AttemptId>,
    ) -> Result<Workspace> {
        if !tx.has(base)? {
            return Err(WorkspaceError::NotFound(format!(
                "snapshot {base} not found"
            )));
        }
        let now = Timestamp::now();
        let ws = Workspace {
            id: WorkspaceId::generate(),
            base: *base,
            owner: *owner,
            attempt,
            state: WorkspaceState::Open,
            mode: WorkspaceMode::Api,
            version: 0,
            head: None,
            created_at: now,
            updated_at: now,
        };
        tx.sql.execute(
            "INSERT INTO workspaces (id, base, owner, attempt, state, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'open', 0, ?5, ?5)",
            params![
                ws.id.to_string(),
                ws.base.to_string(),
                ws.owner.to_string(),
                attempt.map(|a| a.to_string()),
                now.to_string()
            ],
        )?;
        Ok(ws)
    }

    pub fn get(&self, caller: &AgentId, id: &WorkspaceId) -> Result<Workspace> {
        let ws = self
            .store
            .read(|conn| load_row(conn, id))?
            .ok_or_else(|| not_found(id))?;
        check_owner(&ws, caller)?;
        Ok(ws)
    }

    /// Every file at or under `req.path` in the workspace's current state,
    /// filtered by `req.glob`, with kinds and sizes, in path order.
    pub fn walk(
        &self,
        caller: &AgentId,
        id: &WorkspaceId,
        req: &WalkRequest,
    ) -> Result<WalkOutcome> {
        let max = req
            .max_entries
            .unwrap_or(search::DEFAULT_MAX_ENTRIES)
            .clamp(1, search::MAX_ENTRIES);
        let glob = req.glob.as_deref().map(Glob::new).transpose()?;
        let (ws, files) = self.current_files(caller, id, req.path.as_ref())?;
        let (entries, total) = search::walk(&self.store, files, glob.as_ref(), max)?;
        Ok(WalkOutcome {
            workspace: ws.id,
            version: ws.version,
            path: req.path.clone(),
            truncated: total > entries.len(),
            entries,
            total,
        })
    }

    /// Lines matching `req.pattern` in the text files of the workspace's
    /// current state, at or under `req.path` and matching `req.glob`.
    pub fn search(
        &self,
        caller: &AgentId,
        id: &WorkspaceId,
        req: &SearchRequest,
    ) -> Result<SearchOutcome> {
        let max = req
            .max_results
            .unwrap_or(search::DEFAULT_MAX_RESULTS)
            .clamp(1, search::MAX_RESULTS);
        let regex = search::compile(req)?;
        let glob = req.glob.as_deref().map(Glob::new).transpose()?;
        let (ws, files) = self.current_files(caller, id, req.path.as_ref())?;
        let files: Vec<(String, FileEntry)> = files
            .into_iter()
            .filter(|(path, _)| glob.as_ref().is_none_or(|g| g.matches(path)))
            .collect();
        let (matches, truncated, files_searched) =
            search::search(&self.store, &files, &regex, max as usize)?;
        Ok(SearchOutcome {
            workspace: ws.id,
            version: ws.version,
            matches,
            truncated,
            files_searched,
        })
    }

    fn current_files(
        &self,
        caller: &AgentId,
        id: &WorkspaceId,
        under: Option<&Path>,
    ) -> Result<(Workspace, std::collections::BTreeMap<String, FileEntry>)> {
        self.sync_for(caller, id)?;
        let (ws, overlay) = self.state(id)?;
        check_owner(&ws, caller)?;
        check_readable(&ws)?;
        let root = self.base_root(&ws.base)?;
        let files = search::current_files(&self.loader(), &root, &overlay, under)?;
        Ok((ws, files))
    }

    pub fn edit(
        &self,
        caller: &AgentId,
        id: &WorkspaceId,
        req: EditRequest,
    ) -> Result<EditOutcome> {
        if req.ops.is_empty() {
            return Err(WorkspaceError::Invalid("ops must not be empty".into()));
        }
        let loader = self.loader();
        self.store.tx(|tx| {
            let ws = load_row(&tx.sql, id)?.ok_or_else(|| not_found(id))?;
            check_owner(&ws, caller)?;
            check_open(&ws)?;
            if ws.mode == WorkspaceMode::Disk {
                let dir = crate::disk::load_materialization(&tx.sql, id)?
                    .map_or_else(String::new, |m| m.dir.display().to_string());
                return Err(WorkspaceError::Materialized { dir });
            }
            if ws.version != req.if_version {
                return Err(WorkspaceError::StaleVersion {
                    expected: req.if_version,
                    actual: ws.version,
                });
            }
            let root = self.base_root(&ws.base)?;
            let mut batch = edit::Batch::new(tx, *id, root, &loader);
            for op in req.ops {
                batch.apply(op)?;
            }
            let changed = batch.flush()?;
            let version = ws.version + 1;
            tx.sql.execute(
                "UPDATE workspaces SET version = ?2, updated_at = ?3 WHERE id = ?1",
                params![id.to_string(), version as i64, Timestamp::now().to_string()],
            )?;
            Ok(EditOutcome { version, changed })
        })
    }

    /// A unified diff per changed file, in path order, and the workspace
    /// version it describes.
    pub fn diff(&self, caller: &AgentId, id: &WorkspaceId) -> Result<(u64, Vec<FileDiff>)> {
        self.sync_for(caller, id)?;
        let (ws, overlay) = self.state(id)?;
        check_owner(&ws, caller)?;
        check_readable(&ws)?;
        let root = self.base_root(&ws.base)?;
        let loader = self.loader();
        let mut out = Vec::with_capacity(overlay.len());
        for (path, change) in overlay {
            let old = node_at(&loader, &root, path.as_str())?
                .as_ref()
                .and_then(FileEntry::of_node);
            let new = change.file();
            let old_bytes = old.map(|f| self.store.get_raw(&f.blob)).transpose()?;
            let new_bytes = new.map(|f| self.store.get_raw(&f.blob)).transpose()?;
            out.push(diff::file_diff(
                &path,
                old.zip(old_bytes.as_deref()),
                new.zip(new_bytes.as_deref()),
            ));
        }
        Ok((ws.version, out))
    }

    /// The snapshot of the workspace's current state. Creates only
    /// content-addressed objects; returns the base when nothing changed.
    pub fn snapshot(&self, caller: &AgentId, id: &WorkspaceId) -> Result<SnapshotId> {
        Ok(self.snapshot_at(caller, id)?.0)
    }

    /// [`Workspaces::snapshot`] and the workspace version it was taken at.
    pub fn snapshot_at(&self, caller: &AgentId, id: &WorkspaceId) -> Result<(SnapshotId, u64)> {
        self.sync_for(caller, id)?;
        let (ws, overlay) = self.state(id)?;
        check_owner(&ws, caller)?;
        check_readable(&ws)?;
        if let Some(head) = ws.head {
            return Ok((head, ws.version));
        }
        let prepared = self.prepare(&ws, &overlay)?;
        let snapshot_id = prepared.snapshot.id();
        if snapshot_id != ws.base {
            self.store.tx(|tx| {
                if let Some(env) = &prepared.env {
                    tx.put(env)?;
                }
                tx.put(&prepared.snapshot)
            })?;
        }
        Ok((snapshot_id, ws.version))
    }

    pub fn commit(
        &self,
        caller: &AgentId,
        id: &WorkspaceId,
        req: CommitRequest,
    ) -> Result<CommitOutcome> {
        let pending = self.prepare_commit(caller, id, req.message, req.if_version)?;
        if let Some(expected) = req.if_snapshot.filter(|s| *s != pending.head()) {
            return Err(WorkspaceError::SnapshotMoved {
                expected,
                actual: pending.head(),
            });
        }
        self.store.tx(|tx| self.apply_commit(tx, &pending))
    }

    /// Computes everything a commit writes, outside any transaction (the
    /// environment resolver may do I/O). [`Workspaces::apply_commit`] then
    /// writes it, so a caller can commit inside a larger transaction.
    pub(crate) fn prepare_commit(
        &self,
        caller: &AgentId,
        id: &WorkspaceId,
        message: String,
        if_version: Option<u64>,
    ) -> Result<PendingCommit> {
        self.sync_for(caller, id)?;
        let (ws, overlay) = self.state(id)?;
        check_owner(&ws, caller)?;
        check_open(&ws)?;
        if let Some(expected) = if_version.filter(|v| *v != ws.version) {
            return Err(WorkspaceError::StaleVersion {
                expected,
                actual: ws.version,
            });
        }
        let prepared = self.prepare(&ws, &overlay)?;
        let head = prepared.snapshot.id();
        if head == ws.base {
            return Err(WorkspaceError::NoChanges);
        }
        let transition = Transition::sign(
            TransitionBody {
                schema: Schema::new(),
                from: vec![ws.base],
                to: head,
                via: Via::Workspace {
                    workspace: ws.id,
                    attempt: ws.attempt,
                },
                agent: *caller,
                message,
                at: Timestamp::now(),
            },
            &self.server_key,
        );
        Ok(PendingCommit {
            ws,
            prepared,
            transition,
        })
    }

    /// Writes a prepared commit: the snapshot, its transition and lineage
    /// row, and the committed state. Fails with `STALE_VERSION` if the
    /// workspace changed since it was prepared. Moves no ref.
    pub(crate) fn apply_commit(
        &self,
        tx: &Tx<'_>,
        pending: &PendingCommit,
    ) -> Result<CommitOutcome> {
        let PendingCommit {
            ws,
            prepared,
            transition,
        } = pending;
        let current = load_row(&tx.sql, &ws.id)?.ok_or_else(|| not_found(&ws.id))?;
        check_open(&current)?;
        if current.version != ws.version {
            return Err(WorkspaceError::StaleVersion {
                expected: ws.version,
                actual: current.version,
            });
        }
        if let Some(env) = &prepared.env {
            tx.put(env)?;
        }
        let head = tx.put(&prepared.snapshot)?;
        let transition_id = record_transition(tx, transition)?;
        let now = transition.body.at;
        tx.sql.execute(
            "UPDATE workspaces SET state = 'committed', head = ?2, updated_at = ?3 WHERE id = ?1",
            params![ws.id.to_string(), head.to_string(), now.to_string()],
        )?;
        Ok(CommitOutcome {
            workspace: Workspace {
                state: WorkspaceState::Committed,
                head: Some(head),
                updated_at: now,
                ..current
            },
            snapshot: head,
            transition: transition_id,
        })
    }

    /// Marks the workspace discarded and drops its overlay. Discarding a
    /// discarded workspace is a no-op.
    pub fn discard(&self, caller: &AgentId, id: &WorkspaceId) -> Result<Workspace> {
        let ws = self.discard_row(caller, id)?;
        if ws.mode == WorkspaceMode::Disk {
            if let Err(e) = self.disk.release(id) {
                tracing::warn!(workspace = %id, error = %e, "releasing the directory failed; aver gc retries");
            }
        }
        Ok(ws)
    }

    fn discard_row(&self, caller: &AgentId, id: &WorkspaceId) -> Result<Workspace> {
        self.store.tx(|tx| {
            let ws = load_row(&tx.sql, id)?.ok_or_else(|| not_found(id))?;
            check_owner(&ws, caller)?;
            match ws.state {
                WorkspaceState::Discarded => return Ok(ws),
                WorkspaceState::Committed => return Err(closed(&ws)),
                WorkspaceState::Open => {}
            }
            let now = Timestamp::now();
            tx.sql.execute(
                "UPDATE workspaces SET state = 'discarded', updated_at = ?2 WHERE id = ?1",
                params![id.to_string(), now.to_string()],
            )?;
            tx.sql
                .execute("DELETE FROM overlay WHERE workspace = ?1", [id.to_string()])?;
            Ok(Workspace {
                state: WorkspaceState::Discarded,
                updated_at: now,
                ..ws
            })
        })
    }

    pub fn read_snapshot(&self, snapshot: &SnapshotId, path: &Path) -> Result<FileRead> {
        let root = self.base_root(snapshot)?;
        match node_at(&self.loader(), &root, path.as_str())? {
            Some(Node::Tree(_)) => Err(WorkspaceError::NotAFile(path.clone())),
            Some(node) => self.file_read(path, FileEntry::of_node(&node).expect("not a tree")),
            None => Err(missing(path)),
        }
    }

    /// The file at `path` in `snapshot`, or `None` if there is no file there.
    pub fn file_in_snapshot(
        &self,
        snapshot: &SnapshotId,
        path: &Path,
    ) -> Result<Option<FileEntry>> {
        let root = self.base_root(snapshot)?;
        Ok(node_at(&self.loader(), &root, path.as_str())?.and_then(|n| FileEntry::of_node(&n)))
    }

    /// Every file whose entry (kind or contents) differs between two
    /// snapshots, in path order. Unchanged subtrees are skipped by id.
    pub fn changed_files(&self, base: &SnapshotId, head: &SnapshotId) -> Result<Vec<ChangedFile>> {
        let (base, head) = (self.base_root(base)?, self.base_root(head)?);
        let mut out = Vec::new();
        diff_files(&self.loader(), Some(&base), Some(&head), "", &mut out)?;
        Ok(out)
    }

    /// Every file of `snapshot`, in path order.
    pub fn snapshot_files(&self, snapshot: &SnapshotId) -> Result<Vec<(String, FileEntry)>> {
        let root = self.base_root(snapshot)?;
        let mut out = Vec::new();
        tree::walk_files(&self.loader(), &root, "", &mut |path, file| {
            out.push((path.trim_start_matches('/').to_owned(), file));
            Ok(false)
        })?;
        Ok(out)
    }

    /// Stores the snapshot whose tree is `root` with `changes` applied (a
    /// file entry put at the path, or `None` to delete it) and whose
    /// environment is `env`, and returns its id. Unchanged subtrees keep
    /// their ids; directories left empty are pruned. Verification builds the
    /// judged head and the transplant tree this way (§2.5.1).
    pub fn derive_snapshot(
        &self,
        root: &TreeId,
        changes: &[(Path, Option<FileEntry>)],
        env: aver_core::EnvId,
    ) -> Result<SnapshotId> {
        let loader = self.loader();
        let mut new_trees = Vec::new();
        let root = if changes.is_empty() {
            *root
        } else {
            let mut trie = Trie::default();
            for (path, file) in changes {
                let change = match file {
                    Some(file) => Change::Put(*file),
                    None => Change::Delete,
                };
                trie.insert(path.as_str(), change);
            }
            let base = loader(root)?;
            let merged = merge(&loader, Some(&base), &trie, &mut new_trees)?;
            let id = merged.id();
            new_trees.push(merged);
            id
        };
        let snapshot = Snapshot::new(root, env);
        Ok(self.store.tx(|tx| {
            for tree in &new_trees {
                tx.put(tree)?;
            }
            tx.put(&snapshot)
        })?)
    }

    pub fn list_snapshot(
        &self,
        snapshot: &SnapshotId,
        dir: Option<&Path>,
    ) -> Result<Vec<TreeEntry>> {
        let root = self.base_root(snapshot)?;
        let loader = self.loader();
        let tree = match dir {
            None => loader(&root)?,
            Some(dir) => match node_at(&loader, &root, dir.as_str())? {
                Some(Node::Tree(id)) => loader(&id)?,
                Some(_) => return Err(WorkspaceError::NotADirectory(dir.clone())),
                None => return Err(missing(dir)),
            },
        };
        Ok(tree.entries.as_slice().to_vec())
    }

    /// The parsed snapshot object, cached.
    pub fn get_snapshot(&self, id: &SnapshotId) -> Result<Arc<Snapshot>> {
        self.snapshots.get(&self.store, id).map_err(|e| match e {
            StoreError::NotFound(_) => WorkspaceError::NotFound(format!("snapshot {id} not found")),
            other => other.into(),
        })
    }

    fn base_root(&self, snapshot: &SnapshotId) -> Result<TreeId> {
        Ok(self.get_snapshot(snapshot)?.root)
    }

    fn loader(&self) -> impl Fn(&TreeId) -> Result<Arc<Tree>, StoreError> + '_ {
        move |id| self.trees.get(&self.store, id)
    }

    fn file_read(&self, path: &Path, file: FileEntry) -> Result<FileRead> {
        Ok(FileRead {
            path: path.clone(),
            file,
            bytes: self.store.get_raw(&file.blob)?,
        })
    }

    /// The workspace row and its overlay as file entries (`None` for a
    /// delete), without syncing a directory.
    pub(crate) fn overlay_files(&self, id: &WorkspaceId) -> Result<(Workspace, Vec<FileChange>)> {
        let (ws, overlay) = self.state(id)?;
        Ok((
            ws,
            overlay.into_iter().map(|(p, c)| (p, c.file())).collect(),
        ))
    }

    /// The workspace row and its overlay, read consistently.
    fn state(&self, id: &WorkspaceId) -> Result<(Workspace, Vec<(Path, Change)>)> {
        let (ws, overlay) = self.store.read(|conn| {
            let read = conn.unchecked_transaction()?;
            let ws = load_row(&read, id)?;
            let overlay = overlay_rows(&read, id, None)?;
            Ok((ws, overlay))
        })?;
        Ok((ws.ok_or_else(|| not_found(id))?, overlay))
    }

    /// Applies the overlay to the base tree, storing new trees, and resolves
    /// the environment if an input to it changed.
    fn prepare(&self, ws: &Workspace, overlay: &[(Path, Change)]) -> Result<Prepared> {
        let base = self.get_snapshot(&ws.base)?;
        let unchanged = || Prepared {
            snapshot: (*base).clone(),
            env: None,
        };
        if overlay.is_empty() {
            return Ok(unchanged());
        }
        let loader = self.loader();
        let mut trie = Trie::default();
        for (path, change) in overlay {
            trie.insert(path.as_str(), *change);
        }
        debug_assert!(!trie.is_empty());
        let base_root = loader(&base.root)?;
        let mut new_trees = Vec::new();
        let root = merge(&loader, Some(&base_root), &trie, &mut new_trees)?;
        let root_id = root.id();
        if root_id == base.root {
            return Ok(unchanged());
        }
        new_trees.push(root);
        self.store.tx(|tx| {
            for tree in &new_trees {
                tx.put(tree)?;
            }
            Ok::<_, StoreError>(())
        })?;
        let base_env_id = crate::store::pinned_env(&base)?;
        let base_env: Environment = self.store.get(&base_env_id)?;
        let env = if overlay
            .iter()
            .any(|(path, _)| self.env.is_input(path, &base_env))
        {
            Some(
                self.env
                    .resolve(&self.store, &root_id, &base_env)
                    .map_err(|e| WorkspaceError::Env(e.to_string()))?,
            )
        } else {
            None
        };
        let env_id = env.as_ref().map_or(base_env_id, |e| e.id());
        Ok(Prepared {
            snapshot: Snapshot::new(root_id, env_id),
            env: env.filter(|e| e.id() != base_env_id),
        })
    }
}

fn not_found(id: &WorkspaceId) -> WorkspaceError {
    WorkspaceError::NotFound(format!("workspace {id} not found"))
}

fn missing(path: &Path) -> WorkspaceError {
    WorkspaceError::NotFound(format!("{path} does not exist"))
}

fn closed(ws: &Workspace) -> WorkspaceError {
    WorkspaceError::Closed {
        workspace: ws.id,
        state: ws.state,
    }
}

fn check_owner(ws: &Workspace, caller: &AgentId) -> Result<()> {
    if ws.owner == *caller {
        Ok(())
    } else {
        Err(WorkspaceError::NotOwner {
            workspace: ws.id,
            owner: ws.owner,
        })
    }
}

fn check_open(ws: &Workspace) -> Result<()> {
    if ws.state == WorkspaceState::Open {
        Ok(())
    } else {
        Err(closed(ws))
    }
}

fn check_readable(ws: &Workspace) -> Result<()> {
    if ws.state == WorkspaceState::Discarded {
        Err(closed(ws))
    } else {
        Ok(())
    }
}

fn corrupt(id: &WorkspaceId, reason: impl ToString) -> StoreError {
    StoreError::Corrupt {
        id: id.to_string(),
        reason: reason.to_string(),
    }
}

fn load_row(conn: &Connection, id: &WorkspaceId) -> Result<Option<Workspace>, StoreError> {
    type Raw = (
        String,
        String,
        Option<String>,
        String,
        i64,
        Option<String>,
        String,
        String,
        String,
    );
    let row: Option<Raw> = conn
        .query_row(
            "SELECT base, owner, attempt, state, version, head, created_at, updated_at, mode
             FROM workspaces WHERE id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .optional()?;
    let Some((base, owner, attempt, state, version, head, created_at, updated_at, mode)) = row
    else {
        return Ok(None);
    };
    let bad = |e: aver_core::ParseError| corrupt(id, e);
    let state = match state.as_str() {
        "open" => WorkspaceState::Open,
        "committed" => WorkspaceState::Committed,
        "discarded" => WorkspaceState::Discarded,
        other => return Err(corrupt(id, format!("unknown state {other:?}"))),
    };
    let mode = match mode.as_str() {
        "api" => WorkspaceMode::Api,
        "disk" => WorkspaceMode::Disk,
        other => return Err(corrupt(id, format!("unknown mode {other:?}"))),
    };
    Ok(Some(Workspace {
        id: *id,
        base: base.parse().map_err(bad)?,
        owner: owner.parse().map_err(bad)?,
        attempt: attempt.map(|a| a.parse()).transpose().map_err(bad)?,
        state,
        mode,
        version: version as u64,
        head: head.map(|h| h.parse()).transpose().map_err(bad)?,
        created_at: created_at.parse().map_err(bad)?,
        updated_at: updated_at.parse().map_err(bad)?,
    }))
}

fn overlay_row(
    conn: &Connection,
    id: &WorkspaceId,
    path: &str,
) -> Result<Option<Change>, StoreError> {
    let row: Option<(String, Option<String>)> = conn
        .prepare_cached("SELECT kind, blob FROM overlay WHERE workspace = ?1 AND path = ?2")?
        .query_row(params![id.to_string(), path], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    row.map(|(kind, blob)| edit::parse_change(&kind, blob))
        .transpose()
}

/// Overlay rows in path order: all of them, or those strictly under `dir`.
fn overlay_rows(
    conn: &Connection,
    id: &WorkspaceId,
    dir: Option<&str>,
) -> Result<Vec<(Path, Change)>, StoreError> {
    let (lo, hi) = match dir {
        Some(dir) => (format!("{dir}/"), Some(format!("{dir}0"))),
        None => (String::new(), None),
    };
    let mut stmt = conn.prepare_cached(
        "SELECT path, kind, blob FROM overlay
         WHERE workspace = ?1 AND path >= ?2 AND (?3 IS NULL OR path < ?3) ORDER BY path",
    )?;
    let rows = stmt.query_map(params![id.to_string(), lo, hi], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (path, kind, blob) = row?;
        let path = path.parse().map_err(|e| corrupt(id, e))?;
        out.push((path, edit::parse_change(&kind, blob)?));
    }
    Ok(out)
}
