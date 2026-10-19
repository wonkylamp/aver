//! Disk-mode workspaces (§2.3.1): materialize, sync-on-read, restore and
//! release, over the filesystem mechanics in [`crate::disk`].

use std::collections::BTreeMap;
use std::fs;
use std::os::fd::{AsFd as _, AsRawFd as _};
use std::os::unix::fs::{symlink, MetadataExt as _, PermissionsExt};
use std::path::{Path as FsPath, PathBuf};

use aver_core::{Environment, Path, Timestamp, WorkspaceId};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use super::{
    check_open, check_owner, load_row, not_found, FileEntry, FileKind, Result, Workspace,
    WorkspaceError, WorkspaceMode, WorkspaceState, Workspaces,
};
use crate::disk::clone::clone_tree;
use crate::disk::scan::{self, ScanInput, StatCache};
use crate::disk::{
    self, BaseFiles, DiskError, FileLock, MatState, Materialization, SyncReport, WorkspaceConfig,
};
use crate::place::sides::{deps_of, seed_workspace, Deps};

/// What [`Workspaces::materialize`] needs beyond the workspace.
#[derive(Debug, Clone)]
pub struct MaterializeOptions {
    /// The Git repository the base was imported from: names the directory,
    /// scopes ignore rules, and is the source of cloned dependencies and of
    /// `seed` and `warm` paths.
    pub repo: Option<PathBuf>,
    /// The directory's name under `work/<repo>/` (default `ws-<id suffix>`).
    pub label: Option<String>,
    /// Clone the dependencies `repo` has installed (`node_modules`,
    /// `.venv`, a Cargo `target`) when their lockfiles match the base.
    pub deps: bool,
    /// The key file name agents in the directory sign with, recorded in
    /// `.aver/workspace.toml`.
    pub key: Option<String>,
    /// Write the agent-facing Git view (§2.3.1). Needs `repo` and a base
    /// imported from it; otherwise the directory gets none.
    pub git_view: bool,
    /// Reserve this many consecutive ports for the workspace's servers.
    pub ports: Option<u16>,
    /// Untracked paths Aver writes into the directory for agent tools (for
    /// example `.claude/settings.local.json`): never synced and hidden from
    /// the Git view. A path the base tracks is refused.
    pub reserve: Vec<String>,
}

impl Default for MaterializeOptions {
    fn default() -> Self {
        Self {
            repo: None,
            label: None,
            deps: true,
            key: None,
            git_view: false,
            ports: None,
            reserve: Vec::new(),
        }
    }
}

/// The first port `aver run` hands out, and the last one it may use.
const PORTS_FROM: u16 = 42000;
const PORTS_TO: u16 = 60000;

/// `.aver/workspace.toml`: how commands run inside the directory find
/// their workspace.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct WorkspaceFile {
    pub workspace: WorkspaceId,
    pub home: PathBuf,
    pub base: aver_core::SnapshotId,
    pub owner: aver_core::AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<aver_core::AttemptId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<aver_core::TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl WorkspaceFile {
    pub const PATH: &'static str = ".aver/workspace.toml";

    /// Finds the workspace file in `dir` or its nearest ancestor that has one.
    pub fn find(dir: &FsPath) -> std::io::Result<Option<(PathBuf, Self)>> {
        for candidate in dir.ancestors() {
            let path = candidate.join(Self::PATH);
            match fs::read_to_string(&path) {
                Ok(text) => {
                    let file = toml::from_str(&text).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("{}: {e}", path.display()),
                        )
                    })?;
                    return Ok(Some((candidate.to_owned(), file)));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    /// `root/.aver` made a real directory, replacing a symlink or file
    /// there: for a directory Aver has just created, whose `.aver` nothing
    /// else owns. `.aver/` holds Aver's own files (§2.3.1); the checkout
    /// never writes a tracked `.aver` path there (§1.8.2).
    pub fn ensure_dir(root: &FsPath) -> std::io::Result<PathBuf> {
        let dir = root.join(META_DIR);
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => return Ok(dir),
            Ok(_) => fs::remove_file(&dir)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        fs::create_dir(&dir)?;
        Ok(dir)
    }

    /// Writes `bytes` to `rel` (`.aver/<name>`) in `root`, replacing it
    /// atomically: a kill or a full disk leaves the old file or the new one,
    /// never a truncated one. A symlink at `.aver` is an error and one at
    /// the file is replaced, never followed, so neither the repository nor
    /// a link made later steers the write.
    pub fn write_meta(root: &FsPath, rel: &str, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
        Self::write_metas(root, &[(rel, bytes.as_ref())])
    }

    /// [`Self::write_meta`] for several files at once, made durable
    /// together (one drive flush, `atomic::write_all_in_at`).
    pub fn write_metas(root: &FsPath, files: &[(&str, &[u8])]) -> std::io::Result<()> {
        let mut names = Vec::with_capacity(files.len());
        for (rel, bytes) in files {
            let path = meta_file(root, rel)?;
            crate::atomic::sweep(&path);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("meta_file names a UTF-8 file")
                .to_owned();
            names.push((name, *bytes));
        }
        let dir = crate::atomic::open_dir(&root.join(META_DIR))?;
        let files: Vec<(&str, &[u8])> = names.iter().map(|(n, b)| (n.as_str(), *b)).collect();
        crate::atomic::write_all_in_at(Some("meta.write"), dir.as_fd(), &files)
    }

    /// Creates `rel` (`.aver/<name>`) in `root` if nothing is there, as
    /// [`Self::write_meta`] would; true when this call created it.
    pub fn create_meta(root: &FsPath, rel: &str) -> bool {
        meta_file(root, rel)
            .and_then(|path| {
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
            })
            .is_ok()
    }
}

const META_DIR: &str = ".aver";

/// The file `rel` (`.aver/<name>`) in `root`, once `.aver` is known to be a
/// real directory.
fn meta_file(root: &FsPath, rel: &str) -> std::io::Result<PathBuf> {
    let name = rel
        .strip_prefix(".aver/")
        .filter(|n| !n.is_empty() && !n.contains('/'))
        .unwrap_or_else(|| panic!("{rel} is not a file directly under .aver/"));
    let dir = root.join(META_DIR);
    if !fs::symlink_metadata(&dir)?.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a directory; Aver keeps its files there",
            dir.display()
        )));
    }
    Ok(dir.join(name))
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 64
        && !label.starts_with('.')
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl From<DiskError> for WorkspaceError {
    fn from(e: DiskError) -> Self {
        match e {
            DiskError::Store(e) => Self::Store(e),
            other => Self::Disk(other),
        }
    }
}

impl From<std::io::Error> for WorkspaceError {
    fn from(e: std::io::Error) -> Self {
        Self::Disk(DiskError::Io(e))
    }
}

impl Workspaces {
    pub(super) fn base_files_of(
        &self,
        ws: &Workspace,
    ) -> Result<(aver_core::TreeId, std::sync::Arc<BaseFiles>)> {
        let root = self.base_root(&ws.base)?;
        let base = self.disk.base_files(&root, || {
            self.snapshot_files(&ws.base)
                .map_err(|e| DiskError::Corrupt(e.to_string()))
        })?;
        Ok((root, base))
    }

    /// Writes a directory for workspace `id`, which must be open and at
    /// version 0, and switches it to disk mode: from then on the directory
    /// is the only writer of its overlay. Idempotent: a materialized
    /// workspace returns its existing materialization.
    pub fn materialize(
        &self,
        id: &WorkspaceId,
        opts: &MaterializeOptions,
    ) -> Result<Materialization> {
        let ws = self
            .store
            .read(|conn| load_row(conn, id))?
            .ok_or_else(|| not_found(id))?;
        if ws.mode == WorkspaceMode::Disk {
            if let Some(m) = self.disk.materialization(id)? {
                return Ok(m);
            }
        }
        check_open(&ws)?;
        if ws.version != 0 {
            return Err(WorkspaceError::NotFresh(ws.version));
        }
        let label = match &opts.label {
            Some(label) if !valid_label(label) => {
                return Err(WorkspaceError::Invalid(format!(
                    "label {label:?} must be 1-64 of [A-Za-z0-9._-], not starting with '.'"
                )))
            }
            Some(label) => label.clone(),
            None => {
                let ulid = ws.id.ulid().to_string().to_lowercase();
                format!("ws-{}", &ulid[ulid.len() - 8..])
            }
        };
        let repo = opts.repo.as_deref().map(fs::canonicalize).transpose()?;
        let snapshot = self.get_snapshot(&ws.base)?;
        let (root, base) = self.base_files_of(&ws)?;
        // Until the row is in, the checkout and the directory belong to no
        // live materialization; gc waits instead of taking them.
        let work = self.disk.work_lock()?;
        let checkout = self.disk.ensure_checkout(&self.store, &root, || {
            let mut files: Vec<(String, FileEntry)> =
                base.files.iter().map(|(p, f)| (p.clone(), *f)).collect();
            files.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(files)
        })?;
        let config = match base.files.get("aver.toml") {
            Some(file) => WorkspaceConfig::from_aver_toml(&String::from_utf8_lossy(
                &self.store.get_raw(&file.blob)?,
            ))?,
            None => WorkspaceConfig::default(),
        };
        let env: Environment = self.store.get(&crate::store::pinned_env(&snapshot)?)?;

        let repo_name = repo
            .as_deref()
            .and_then(FsPath::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| valid_label(n))
            .unwrap_or_else(|| "snapshots".into());
        let parent = self.disk.work_dir().join(repo_name);
        fs::create_dir_all(&parent)?;
        let parent = fs::canonicalize(&parent)?;
        let mut dir = parent.join(&label);
        let mut n = 2;
        while fs::symlink_metadata(&dir).is_ok() {
            dir = parent.join(format!("{label}-{n}"));
            n += 1;
        }
        let strategy = self.disk.strategy()?;
        clone_tree(strategy, &checkout.dir, &dir)?;
        let built = crate::fault::point("materialize.cloned")
            .map_err(WorkspaceError::from)
            .and_then(|()| {
                self.finish_materialize(
                    &ws, opts, &dir, repo, &checkout, &config, &env, &base, strategy,
                )
            });
        if built.is_err() {
            if let Err(e) = self.disk.trash(&dir) {
                tracing::warn!(error = %e, dir = %dir.display(), "could not trash a failed workspace directory");
            }
            let _ = fs::remove_dir_all(self.disk.mat_dir(id));
        }
        drop(work);
        let m = built?;
        crate::fault::point("materialize.recorded")?;
        Ok(m)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_materialize(
        &self,
        ws: &Workspace,
        opts: &MaterializeOptions,
        dir: &FsPath,
        repo: Option<PathBuf>,
        checkout: &disk::checkout::Checkout,
        config: &WorkspaceConfig,
        env: &Environment,
        base: &BaseFiles,
        strategy: disk::CloneStrategy,
    ) -> Result<Materialization> {
        let mut seeded = Vec::new();
        if let Some(repo) = &repo {
            let mut chosen = Deps::default();
            for (rels, list) in [
                (&config.seed, &mut chosen.keep),
                (&config.warm, &mut chosen.seed),
            ] {
                for rel in rels {
                    let path: Path = rel.parse().map_err(|e| {
                        WorkspaceError::Invalid(format!("aver.toml [workspace] path {rel:?}: {e}"))
                    })?;
                    list.push(path.as_str().to_owned());
                }
            }
            // The paths `aver.toml` names are cloned whatever the lockfiles
            // say; they need not be dependencies at all.
            let found = if opts.deps {
                deps_of(env)
            } else {
                Deps::default()
            };
            for deps in [found, chosen] {
                for rel in seed_workspace(strategy, repo, dir, &deps, base)? {
                    if !seeded.contains(&rel) {
                        seeded.push(rel);
                    }
                }
            }
        }
        for rel in &opts.reserve {
            let path: Path = rel
                .parse()
                .map_err(|e| WorkspaceError::Invalid(format!("reserved path {rel:?}: {e}")))?;
            let rel = path.as_str();
            if base.files.contains_key(rel) || base.dirs.contains(rel) {
                return Err(WorkspaceError::Invalid(format!(
                    "{rel} is tracked by the base, so Aver cannot reserve it"
                )));
            }
            if !seeded.iter().any(|s| s == rel) {
                seeded.push(rel.to_owned());
            }
        }
        let git_view = match (&repo, opts.git_view) {
            (Some(repo), true) => self.write_git_view(ws, dir, repo, base, &seeded)?,
            _ => false,
        };
        crate::fault::point("materialize.view")?;

        let task: Option<aver_core::TaskId> = match ws.attempt {
            Some(att) => self
                .store
                .read(|conn| {
                    Ok(conn
                        .query_row(
                            "SELECT task FROM attempts WHERE id = ?1",
                            [att.to_string()],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()?)
                })?
                .and_then(|t| t.parse().ok()),
            None => None,
        };
        let file = WorkspaceFile {
            workspace: ws.id,
            home: fs::canonicalize(self.disk.root())?,
            base: ws.base,
            owner: ws.owner,
            attempt: ws.attempt,
            task,
            key: opts.key.clone(),
        };
        WorkspaceFile::ensure_dir(dir)?;
        let mut text = String::from(
            "# Written by Aver for commands run in this directory (aver verify, diff, restore).\n",
        );
        text.push_str(&toml::to_string(&file).expect("workspace files serialize"));
        WorkspaceFile::write_metas(
            dir,
            &[
                (".aver/.gitignore", b"*\n"),
                (WorkspaceFile::PATH, text.as_bytes()),
            ],
        )?;

        let mat_dir = self.disk.mat_dir(&ws.id);
        fs::create_dir_all(&mat_dir)?;
        let manifest = self.disk.manifest(&checkout.tree)?;
        let cache = scan::seed(dir, &mat_dir, &manifest)?;
        cache.save(&mat_dir.join("stat"))?;

        let m = Materialization {
            workspace: ws.id,
            dir: dir.to_owned(),
            repo,
            checkout: checkout.tree,
            clone: strategy,
            seeded,
            absent: checkout.absent.clone(),
            git_view,
            ports: None,
            state: MatState::Live,
            created_at: Timestamp::now(),
            released_at: None,
        };
        self.store.tx(|tx| {
            // The switch comes first: a concurrent materialize of the same
            // workspace then fails as NOT_FRESH, not on the row's key.
            let switched = tx.sql.execute(
                "UPDATE workspaces SET mode = 'disk', updated_at = ?2
                 WHERE id = ?1 AND version = 0 AND state = 'open' AND mode = 'api'",
                params![ws.id.to_string(), Timestamp::now().to_string()],
            )?;
            if switched == 0 {
                return Err(WorkspaceError::NotFresh(ws.version));
            }
            // Allocated inside the write transaction, so two concurrent
            // materializations never share a block.
            let mut m = m;
            if let Some(count) = opts.ports.filter(|c| *c > 0) {
                m.ports = Some(free_ports(&tx.sql, count)?);
            }
            disk::insert_materialization(&tx.sql, &m)?;
            Ok(m)
        })
    }

    /// Writes the agent-facing Git view when the base came from `repo`'s
    /// history and import recorded every blob's SHA-1; returns whether it
    /// wrote one.
    fn write_git_view(
        &self,
        ws: &Workspace,
        dir: &FsPath,
        repo: &FsPath,
        base: &BaseFiles,
        seeded: &[String],
    ) -> Result<bool> {
        let Some(head) = crate::git::imported_commit(&self.store, &ws.base)? else {
            return Ok(false);
        };
        let shas = crate::git::blob_shas(&self.store, base.files.values().map(|f| &f.blob))?;
        let mut files = Vec::with_capacity(base.files.len());
        for (path, file) in &base.files {
            let Some(sha) = shas.get(&file.blob) else {
                return Ok(false);
            };
            files.push(disk::gitview::IndexFile {
                path,
                kind: file.kind,
                sha,
            });
        }
        let mut exclude = vec![".aver".to_owned()];
        exclude.extend(seeded.iter().cloned());
        let label = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        disk::gitview::write(&disk::gitview::ViewSpec {
            gitdir: &self.disk.git_dir(&ws.id),
            dir,
            repo,
            head: &head,
            branch: &disk::gitview::branch_for(&label),
            files,
            exclude,
        })?;
        Ok(true)
    }

    /// Scans a disk-mode workspace's directory into its overlay and returns
    /// what changed, or `None` for an API-mode workspace. Every read of a
    /// disk-mode workspace does this first, so callers rarely need it.
    pub fn sync(
        &self,
        caller: &aver_core::AgentId,
        id: &WorkspaceId,
    ) -> Result<Option<SyncReport>> {
        let ws = self.get(caller, id)?;
        self.sync_row(&ws)
    }

    /// Syncs `id` for `caller` before a read: refuses non-owners first, so
    /// only the owner can make Aver scan the directory.
    pub(super) fn sync_for(&self, caller: &aver_core::AgentId, id: &WorkspaceId) -> Result<()> {
        let ws = self
            .store
            .read(|conn| load_row(conn, id))?
            .ok_or_else(|| not_found(id))?;
        check_owner(&ws, caller)?;
        self.sync_row(&ws)?;
        Ok(())
    }

    pub(super) fn sync_row(&self, ws: &Workspace) -> Result<Option<SyncReport>> {
        if ws.mode != WorkspaceMode::Disk || ws.state != WorkspaceState::Open {
            return Ok(None);
        }
        let started = std::time::Instant::now();
        let m = self.disk.materialization(&ws.id)?.ok_or_else(|| {
            WorkspaceError::Store(super::corrupt(
                &ws.id,
                "disk mode without a materialization",
            ))
        })?;
        if m.state != MatState::Live {
            return Err(DiskError::DirMissing(m.dir).into());
        }
        let mat_dir = self.disk.mat_dir(&ws.id);
        let _lock = FileLock::exclusive(&mat_dir.join("lock"))?;
        let (ws, current) = self.state(&ws.id)?;
        let current: BTreeMap<String, Option<FileEntry>> = current
            .into_iter()
            .map(|(p, c)| (p.as_str().to_owned(), c.file()))
            .collect();
        let (_, base) = self.base_files_of(&ws)?;
        let cache = StatCache::load(&mat_dir.join("stat"));
        let exclusions = disk::exclusions(&m);
        let extra = self.disk.extra_ignores(m.repo.as_deref());
        let input = ScanInput {
            dir: &m.dir,
            clock_dir: &mat_dir,
            base: &base,
            current: &current,
            cache: &cache,
            exclusions: &exclusions,
            extra_ignores: &extra,
            max_file_bytes: self.disk.config().max_file_bytes,
        };
        let state = match scan::scan(&input) {
            Ok(state) => state,
            Err(DiskError::DirMissing(dir)) => {
                self.disk.set_state(&ws.id, MatState::Lost)?;
                return Err(DiskError::DirMissing(dir).into());
            }
            Err(e) => return Err(e.into()),
        };
        let changed: Vec<(&String, Option<FileEntry>)> = state
            .desired
            .iter()
            .filter(|(p, v)| current.get(*p) != Some(v))
            .map(|(p, v)| (p, *v))
            .chain(
                current
                    .keys()
                    .filter(|p| !state.desired.contains_key(*p))
                    .map(|p| (p, None)),
            )
            .collect();
        let removed: Vec<&String> = current
            .keys()
            .filter(|p| !state.desired.contains_key(*p))
            .collect();
        crate::fault::point("sync.blobs")?;
        let version = if changed.is_empty() {
            ws.version
        } else {
            self.store.tx(|tx| {
                let row = load_row(&tx.sql, &ws.id)?.ok_or_else(|| not_found(&ws.id))?;
                check_open(&row)?;
                for file in changed.iter().filter_map(|(_, file)| *file) {
                    if let Some(bytes) = state.new_blobs.get(&file.blob) {
                        tx.put_blob(bytes)?;
                    }
                }
                for path in &removed {
                    tx.sql.execute(
                        "DELETE FROM overlay WHERE workspace = ?1 AND path = ?2",
                        params![ws.id.to_string(), path],
                    )?;
                }
                for (path, row) in &state.desired {
                    if current.get(path) == Some(row) {
                        continue;
                    }
                    let (kind, blob) = match row {
                        Some(file) => (file.kind.as_str(), Some(file.blob.to_string())),
                        None => ("delete", None),
                    };
                    tx.sql.execute(
                        "INSERT INTO overlay (workspace, path, kind, blob) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (workspace, path) DO UPDATE SET kind = excluded.kind, blob = excluded.blob",
                        params![ws.id.to_string(), path, kind, blob],
                    )?;
                }
                let version = row.version + 1;
                tx.sql.execute(
                    "UPDATE workspaces SET version = ?2, updated_at = ?3 WHERE id = ?1",
                    params![ws.id.to_string(), version as i64, Timestamp::now().to_string()],
                )?;
                Ok::<_, WorkspaceError>(version)
            })?
        };
        crate::fault::point("sync.committed")?;
        // Only after the commit: every blob the cache names is now stored.
        state.stat.save(&mat_dir.join("stat"))?;
        crate::coord::renew(&self.store, &ws.id)?;
        let (added, modified, deleted) = disk::counts(&base, &state.desired);
        Ok(Some(SyncReport {
            version,
            added,
            modified,
            deleted,
            updated: changed.len() as u32,
            changed: state
                .desired
                .keys()
                .take(SyncReport::LISTED)
                .cloned()
                .collect(),
            skipped: state.skipped,
            stats: disk::ScanStats {
                took_ms: started.elapsed().as_millis() as u64,
                ..state.stats
            },
        }))
    }

    /// The live directory of open disk workspace `id`, for its owner.
    pub(super) fn live_dir(
        &self,
        caller: &aver_core::AgentId,
        id: &WorkspaceId,
    ) -> Result<(Workspace, Materialization)> {
        let ws = self.get(caller, id)?;
        check_open(&ws)?;
        if ws.mode != WorkspaceMode::Disk {
            return Err(WorkspaceError::Invalid(format!(
                "workspace {id} has no directory"
            )));
        }
        let m = self
            .disk
            .materialization(id)?
            .filter(|m| m.state == MatState::Live)
            .ok_or_else(|| {
                WorkspaceError::Invalid(format!("workspace {id} has no live directory"))
            })?;
        Ok((ws, m))
    }

    /// Writes each path's base version into the directory, or removes it
    /// where the base has none. A base directory restores every file under
    /// it. The next sync records the change.
    ///
    /// It holds the directory's sync lock, so no verify judges a
    /// half-restored tree, and first stores what it replaces and logs it to
    /// `mat/<ws>/restored.jsonl`, so [`Self::restore_undo`] can put it back.
    pub fn restore(
        &self,
        caller: &aver_core::AgentId,
        id: &WorkspaceId,
        paths: &[Path],
    ) -> Result<Vec<String>> {
        let (ws, m) = self.live_dir(caller, id)?;
        let mat_dir = self.disk.mat_dir(id);
        let _lock = FileLock::exclusive(&mat_dir.join("lock"))?;
        let exclusions = disk::exclusions(&m);
        let (_, base) = self.base_files_of(&ws)?;
        // Each path and what it becomes: a base file, or nothing.
        let mut steps: Vec<(String, Option<FileEntry>)> = Vec::new();
        for path in paths {
            let rel = path.as_str();
            let protected = rel
                .match_indices('/')
                .map(|(i, _)| &rel[..i])
                .chain([rel])
                .any(|p| exclusions.silent.contains(p));
            if protected {
                return Err(WorkspaceError::Invalid(format!(
                    "{rel} is managed by Aver (.aver, or a seeded dependency or path)"
                )));
            }
            let mut files: Vec<(&String, &FileEntry)> = match base.files.get_key_value(rel) {
                Some(hit) => vec![hit],
                None if base.dirs.contains(rel) => {
                    let prefix = format!("{rel}/");
                    base.files
                        .iter()
                        .filter(|(p, _)| p.starts_with(&prefix))
                        .collect()
                }
                None => Vec::new(),
            };
            files.sort_by(|a, b| a.0.cmp(b.0));
            if files.is_empty() {
                // Behind a symlink or a file, the path is not in the
                // directory, and deleting through it would reach outside.
                if real_ancestors(&m.dir, rel, false)?
                    && fs::symlink_metadata(m.dir.join(rel)).is_ok()
                {
                    steps.push((rel.to_owned(), None));
                }
                continue;
            }
            steps.extend(files.into_iter().map(|(p, f)| (p.clone(), Some(*f))));
        }
        self.apply_restore(&m, &steps)
    }

    /// Writes each step into the directory of `m` (a file, or nothing where
    /// it is `None`), first storing what it replaces and logging it to
    /// `mat/<ws>/restored.jsonl` for [`Self::restore_undo`]. The caller
    /// holds the directory's sync lock.
    pub(super) fn apply_restore(
        &self,
        m: &Materialization,
        steps: &[(String, Option<FileEntry>)],
    ) -> Result<Vec<String>> {
        let mat_dir = self.disk.mat_dir(&m.workspace);
        if steps.is_empty() {
            return Ok(Vec::new());
        }
        let mut replaced = Vec::new();
        let mut blobs = Vec::new();
        for (rel, _) in steps {
            capture(&m.dir, rel, &mut replaced, &mut blobs)?;
        }
        let kept: usize = blobs.iter().map(Vec::len).sum();
        if kept > MAX_RESTORE_KEPT {
            return Err(WorkspaceError::Invalid(format!(
                "restoring would replace {} MiB, more than aver restore --undo keeps ({} MiB); remove what you do not need yourself first",
                kept >> 20,
                MAX_RESTORE_KEPT >> 20
            )));
        }
        self.store.tx(|tx| {
            for bytes in &blobs {
                tx.put_blob(bytes)?;
            }
            Ok::<_, WorkspaceError>(())
        })?;
        append_restored(
            &mat_dir.join(RESTORE_LOG),
            &RestoreRecord {
                at: Timestamp::now(),
                files: replaced,
            },
        )?;
        let tmp = restore_tmp(&m.dir)?;
        let mut restored = Vec::new();
        for (rel, to) in steps {
            match to {
                Some(file) => {
                    let bytes = self.store.get_raw(&file.blob)?;
                    put_file(&m.dir, &tmp, rel, file.kind, &bytes)?;
                }
                None => super::rebase::remove_in(&m.dir, rel)?,
            }
            restored.push(rel.clone());
            crate::fault::point("restore.partial")?;
        }
        Ok(restored)
    }

    /// Puts back what the last [`Self::restore`] in the directory replaced
    /// (also one cut short), and forgets it, so a second undo goes one
    /// restore further back. Returns the paths written or removed.
    pub fn restore_undo(
        &self,
        caller: &aver_core::AgentId,
        id: &WorkspaceId,
    ) -> Result<Vec<String>> {
        let (_, m) = self.live_dir(caller, id)?;
        let mat_dir = self.disk.mat_dir(id);
        let _lock = FileLock::exclusive(&mat_dir.join("lock"))?;
        let log = mat_dir.join(RESTORE_LOG);
        let text = match fs::read_to_string(&log) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let mut lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        // A line torn while it was appended came before every write of its
        // restore, so there is nothing of it to undo.
        while lines
            .last()
            .is_some_and(|l| serde_json::from_str::<RestoreRecord>(l).is_err())
        {
            lines.pop();
        }
        let Some(last) = lines.pop() else {
            return Err(WorkspaceError::Invalid(
                "nothing to undo: aver restore has replaced nothing in this directory".into(),
            ));
        };
        let record: RestoreRecord =
            serde_json::from_str(last).expect("the last line was just parsed");
        let tmp = restore_tmp(&m.dir)?;
        let mut undone = Vec::new();
        // Removals first: a path that was absent may sit where a replaced
        // directory's files go back.
        for r in record.files.iter().filter(|r| r.was.is_none()) {
            super::rebase::remove_in(&m.dir, &r.path)?;
            undone.push(r.path.clone());
        }
        for r in &record.files {
            if let Some(file) = r.was {
                let bytes = self.store.get_raw(&file.blob)?;
                put_file(&m.dir, &tmp, &r.path, file.kind, &bytes)?;
                undone.push(r.path.clone());
            }
        }
        let rest: String = lines.iter().map(|l| format!("{l}\n")).collect();
        crate::atomic::write(&log, rest.as_bytes(), crate::atomic::Mode::Keep)?;
        undone.sort();
        undone.dedup();
        Ok(undone)
    }

    pub(super) fn write_base_file(&self, dir: &FsPath, rel: &str, file: &FileEntry) -> Result<()> {
        let bytes = self.store.get_raw(&file.blob)?;
        put_file(dir, &restore_tmp(dir)?, rel, file.kind, &bytes)
    }

    /// The paths where the directory of committed workspace `id` differs
    /// from its commit, by a scan that records nothing; empty for a
    /// workspace that is not committed or has no live directory.
    pub fn changed_since_commit(&self, id: &WorkspaceId) -> Result<Vec<String>> {
        let ws = self
            .store
            .read(|conn| load_row(conn, id))?
            .ok_or_else(|| not_found(id))?;
        let (WorkspaceState::Committed, Some(head)) = (ws.state, ws.head) else {
            return Ok(Vec::new());
        };
        self.changed_since(id, &head)
    }

    /// The paths where the live directory of workspace `id` differs from
    /// `snapshot`, by a scan that records nothing; empty when it has no
    /// live directory.
    pub fn changed_since(
        &self,
        id: &WorkspaceId,
        snapshot: &aver_core::SnapshotId,
    ) -> Result<Vec<String>> {
        let Some(m) = self
            .disk
            .materialization(id)?
            .filter(|m| m.state == MatState::Live)
        else {
            return Ok(Vec::new());
        };
        let files = BaseFiles::new(self.snapshot_files(snapshot)?);
        let mat_dir = self.disk.mat_dir(id);
        fs::create_dir_all(&mat_dir)?;
        let cache = StatCache::load(&mat_dir.join("stat"));
        let current = BTreeMap::new();
        let exclusions = disk::exclusions(&m);
        let extra = self.disk.extra_ignores(m.repo.as_deref());
        let state = scan::scan(&ScanInput {
            dir: &m.dir,
            clock_dir: &mat_dir,
            base: &files,
            current: &current,
            cache: &cache,
            exclusions: &exclusions,
            extra_ignores: &extra,
            max_file_bytes: self.disk.config().max_file_bytes,
        })?;
        Ok(state.desired.into_keys().collect())
    }

    /// Why the directory of workspace `m` must stay: it changed since its
    /// commit, or cannot be compared with it.
    fn changed_reason(&self, m: &Materialization) -> Option<String> {
        match self.changed_since_commit(&m.workspace) {
            Ok(paths) if paths.is_empty() => None,
            Ok(paths) => Some(format!(
                "its directory changed since the commit ({}); `aver workspace rm --force {}` removes it",
                listed(&paths),
                m.dir.display()
            )),
            Err(e) => Some(format!(
                "its directory could not be compared with its commit ({e})"
            )),
        }
    }

    /// [`Disk::gc`], also keeping the directory of a committed workspace
    /// that changed since its commit.
    pub fn gc(&self, policy: &disk::GcPolicy) -> Result<disk::GcReport> {
        Ok(self.disk.gc_keeping(policy, &|m| self.changed_reason(m))?)
    }

    /// Moves the workspace's directory to the trash. The workspace keeps its
    /// last synced overlay; reads that need the directory then fail with
    /// `DIR_MISSING`. A committed workspace's directory that changed since
    /// the commit stays (`DIR_CHANGED`) unless `force`: nothing else holds
    /// those changes.
    pub fn release(&self, id: &WorkspaceId, force: bool) -> Result<()> {
        self.store
            .read(|conn| load_row(conn, id))?
            .ok_or_else(|| not_found(id))?;
        if !force {
            if let Some(m) = self.disk.materialization(id)? {
                if let Some(why) = self.changed_reason(&m) {
                    return Err(WorkspaceError::DirChanged(why));
                }
            }
        }
        Ok(self.disk.release(id)?)
    }
}

/// The lowest block of `count` ports from [`PORTS_FROM`], aligned to ten,
/// that no live materialization holds.
fn free_ports(conn: &rusqlite::Connection, count: u16) -> Result<disk::Ports> {
    let rows = || -> rusqlite::Result<Vec<(u16, u16)>> {
        conn.prepare(
            "SELECT port_base, port_count FROM materializations
             WHERE state = 'live' AND port_base IS NOT NULL",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
    };
    let taken = rows().map_err(crate::store::StoreError::from)?;
    let step = count.div_ceil(10) * 10;
    let mut base = PORTS_FROM;
    while base.saturating_add(count) <= PORTS_TO {
        let end = base + count;
        if taken.iter().all(|(b, c)| end <= *b || b + c <= base) {
            return Ok(disk::Ports { base, count });
        }
        base += step;
    }
    Err(WorkspaceError::Invalid(format!(
        "no free block of {count} ports between {PORTS_FROM} and {PORTS_TO}"
    )))
}

/// Whether every ancestor of `rel` under `root` is a real directory, checked
/// from the root down without following a symlink, so a path there stays
/// inside `root`. With `make`, a missing ancestor is created and a symlink
/// or file in the way is replaced by a directory, as the base has one there.
pub(super) fn real_ancestors(root: &FsPath, rel: &str, make: bool) -> std::io::Result<bool> {
    let mut at = root.to_path_buf();
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop();
    for part in parts {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.is_dir() => continue,
            Ok(_) if make => fs::remove_file(&at)?,
            Err(e) if make && e.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
        fs::create_dir(&at)?;
    }
    Ok(true)
}

/// The most a restore stores of what it replaces, for `--undo`.
const MAX_RESTORE_KEPT: usize = 256 << 20;

/// What [`Workspaces::restore`] replaced, one line per restore, in the
/// workspace's `mat/` directory (outside the agent's reach).
const RESTORE_LOG: &str = "restored.jsonl";

#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreRecord {
    at: Timestamp,
    files: Vec<Replaced>,
}

/// A path a restore wrote or removed, and what was there, stored.
#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Replaced {
    path: String,
    /// `None`: nothing was there.
    was: Option<FileEntry>,
}

/// Appends `record` to the log at `path` and makes it durable before the
/// restore touches a file.
fn append_restored(path: &FsPath, record: &RestoreRecord) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut line = serde_json::to_vec(record).expect("restore records serialize");
    line.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(&line)?;
    file.sync_all()
}

/// Adds to `out` (and its contents to `blobs`) what is at `rel` in `dir`
/// now: a file or symlink, every file under a directory, or nothing. A
/// file or symlink standing where `rel` needs a directory is what a write
/// at `rel` replaces, so it is captured instead.
fn capture(
    dir: &FsPath,
    rel: &str,
    out: &mut Vec<Replaced>,
    blobs: &mut Vec<Vec<u8>>,
) -> Result<()> {
    let mut at = String::new();
    for part in rel.split('/').take(rel.split('/').count() - 1) {
        if !at.is_empty() {
            at.push('/');
        }
        at.push_str(part);
        match fs::symlink_metadata(dir.join(&at)) {
            Ok(meta) if meta.is_dir() => continue,
            Ok(_) => {
                capture(dir, &at, out, blobs)?;
                out.push(Replaced {
                    path: rel.to_owned(),
                    was: None,
                });
                return Ok(());
            }
            Err(_) => break,
        }
    }
    let target = dir.join(rel);
    let meta = match fs::symlink_metadata(&target) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            out.push(Replaced {
                path: rel.to_owned(),
                was: None,
            });
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let (kind, bytes) = if meta.is_dir() {
        out.push(Replaced {
            path: rel.to_owned(),
            was: None,
        });
        let mut names: Vec<String> = fs::read_dir(&target)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<_>>()?;
        names.sort();
        for name in names {
            capture(dir, &format!("{rel}/{name}"), out, blobs)?;
        }
        return Ok(());
    } else if meta.file_type().is_symlink() {
        use std::os::unix::ffi::OsStrExt;
        let to = fs::read_link(&target)?;
        (FileKind::Symlink, to.as_os_str().as_bytes().to_vec())
    } else if meta.is_file() {
        let kind = if meta.mode() & 0o111 != 0 {
            FileKind::Exec
        } else {
            FileKind::File
        };
        (kind, fs::read(&target)?)
    } else {
        out.push(Replaced {
            path: rel.to_owned(),
            was: None,
        });
        return Ok(());
    };
    out.push(Replaced {
        path: rel.to_owned(),
        was: Some(FileEntry {
            kind,
            blob: aver_core::BlobId::of_bytes(&bytes),
        }),
    });
    blobs.push(bytes);
    Ok(())
}

/// `.aver/tmp` in workspace directory `dir`, emptied of what dead
/// processes left there: restore's temporary files sit on the directory's
/// volume, so a rename moves them in, and under `.aver/`, which no scan
/// records, so a killed restore leaves nothing a sync picks up.
fn restore_tmp(dir: &FsPath) -> Result<PathBuf> {
    let meta = dir.join(META_DIR);
    if !fs::symlink_metadata(&meta)?.is_dir() {
        return Err(WorkspaceError::Invalid(format!(
            "{} is not a directory; Aver keeps its files there",
            meta.display()
        )));
    }
    let tmp = meta.join("tmp");
    match fs::symlink_metadata(&tmp) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            fs::remove_file(&tmp)?;
            fs::create_dir(&tmp)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&tmp)?,
        Err(e) => return Err(e.into()),
    }
    for entry in fs::read_dir(&tmp)?.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|n| n.strip_prefix("restore-"))
            .and_then(|r| r.split('-').next())
            .and_then(|p| p.parse::<u32>().ok());
        if pid.is_none_or(|p| !crate::run::state::process_alive(p)) {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(tmp)
}

/// Writes `bytes` as a `kind` file at `rel` in `dir`: staged in `tmp`
/// ([`restore_tmp`]), then renamed into the parent, opened once its
/// ancestors are checked to be real directories inside `dir`.
fn put_file(dir: &FsPath, tmp: &FsPath, rel: &str, kind: FileKind, bytes: &[u8]) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    real_ancestors(dir, rel, true)?;
    let target = dir.join(rel);
    if let Ok(meta) = fs::symlink_metadata(&target) {
        if meta.is_dir() {
            fs::remove_dir_all(&target)?;
        }
    }
    let parent = crate::atomic::open_dir(
        target
            .parent()
            .expect("a file under the directory has a parent"),
    )?;
    let staged = tmp.join(format!(
        "restore-{}-{}",
        std::process::id(),
        ulid::Ulid::generate()
    ));
    match kind {
        FileKind::Symlink => symlink(std::ffi::OsStr::from_bytes(bytes), &staged)?,
        kind => {
            fs::write(&staged, bytes)?;
            let mode = if kind == FileKind::Exec { 0o755 } else { 0o644 };
            fs::set_permissions(&staged, fs::Permissions::from_mode(mode))?;
        }
    }
    let moved = crate::fault::point("restore.tmp").and_then(|()| {
        let cstr = |b: &[u8]| {
            std::ffi::CString::new(b).map_err(|_| std::io::Error::other("a NUL in a path"))
        };
        let from = cstr(staged.as_os_str().as_bytes())?;
        let name = cstr(
            target
                .file_name()
                .expect("a file under the directory has a name")
                .as_bytes(),
        )?;
        // SAFETY: valid C strings and an open directory descriptor; the
        // result is checked.
        let r = unsafe {
            libc::renameat(
                libc::AT_FDCWD,
                from.as_ptr(),
                parent.as_raw_fd(),
                name.as_ptr(),
            )
        };
        if r == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    });
    if moved.is_err() {
        let _ = fs::remove_file(&staged);
    }
    Ok(moved?)
}

/// Up to five of `paths`, and how many more.
fn listed(paths: &[String]) -> String {
    let shown: Vec<&str> = paths.iter().take(5).map(String::as_str).collect();
    match paths.len().saturating_sub(shown.len()) {
        0 => shown.join(", "),
        more => format!("{} and {more} more", shown.join(", ")),
    }
}
