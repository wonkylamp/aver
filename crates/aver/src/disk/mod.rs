//! On-disk workspaces (ARCHITECTURE.md §1.8.1-§1.8.3, §2.3.1): the host
//! side of a disk-mode workspace.
//!
//! A materialized workspace is a real directory that agent CLIs edit with
//! their own tools. It is a copy-on-write clone of an immutable checkout of
//! the base tree, plus clones of the dependencies the user's checkout has
//! installed (`place::sides::seed_workspace`). The directory is the only
//! writer of the workspace's overlay: the workspace service scans it
//! before every read (sync-on-read), so no caller observes a stale
//! overlay. This module owns the filesystem mechanics (checkouts, clones,
//! scans, stat caches, trash and gc); [`crate::workspace::Workspaces`] owns
//! the rows and calls into it.
//!
//! `$AVER_HOME` layout: `work/<repo>/<label>` (workspace directories),
//! `checkouts/<tree>` (+ `.manifest`), `mat/<ws>/` (stat cache and lock),
//! `trash/` (renamed, then deleted).

pub mod checkout;
pub mod clone;
pub mod gitview;
pub(crate) mod scan;
pub mod usage;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aver_core::{Timestamp, TreeId, WorkspaceId, MAX_BLOB_BYTES};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub use clone::CloneStrategy;

use crate::place::sides::{remove_slot, Sides};
use crate::store::{Store, StoreError};
use crate::workspace::FileEntry;

/// Environment variable that forces a clone strategy: `clonefile`,
/// `reflink` or `copy`.
pub const CLONE_ENV: &str = "AVER_CLONE";

#[derive(Debug, thiserror::Error)]
pub enum DiskError {
    #[error("workspace directory {} is missing", .0.display())]
    DirMissing(PathBuf),
    #[error("clone strategy {0} is not supported on this host")]
    Unsupported(&'static str),
    #[error("{0}")]
    Invalid(String),
    #[error("corrupt disk state: {0}")]
    Corrupt(String),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for DiskError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e.into())
    }
}

impl DiskError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DirMissing(_) => "DIR_MISSING",
            Self::Unsupported(_) | Self::Invalid(_) => "INVALID",
            Self::Corrupt(_) | Self::Io(_) | Self::Store(_) => "INTERNAL",
        }
    }
}

/// Where the host side lives and how it behaves. Tests force strategies
/// and limits that the host would not pick by itself.
#[derive(Debug, Clone)]
pub struct DiskConfig {
    /// `$AVER_HOME`.
    pub root: PathBuf,
    /// A forced clone strategy; otherwise [`CLONE_ENV`], otherwise a probe.
    pub clone: Option<CloneStrategy>,
    /// Files at least this large are skipped (`too_large`).
    pub max_file_bytes: u64,
    /// Treat the filesystem as case-insensitive (fold paths when writing
    /// checkouts) regardless of what a probe finds.
    pub case_insensitive: Option<bool>,
    /// Apply the user's global Git excludes file to untracked paths.
    pub global_excludes: bool,
}

impl DiskConfig {
    pub fn under(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            clone: std::env::var(CLONE_ENV)
                .ok()
                .and_then(|s| CloneStrategy::parse(&s)),
            max_file_bytes: MAX_BLOB_BYTES as u64,
            case_insensitive: None,
            global_excludes: true,
        }
    }
}

/// Every file of a base tree, and every directory holding one.
#[derive(Debug, Default)]
pub struct BaseFiles {
    pub files: HashMap<String, FileEntry>,
    pub dirs: HashSet<String>,
}

impl BaseFiles {
    pub fn new(files: impl IntoIterator<Item = (String, FileEntry)>) -> Self {
        let files: HashMap<String, FileEntry> = files.into_iter().collect();
        let mut dirs = HashSet::new();
        for path in files.keys() {
            let mut prefix = path.as_str();
            while let Some(i) = prefix.rfind('/') {
                prefix = &prefix[..i];
                if !dirs.insert(prefix.to_owned()) {
                    break;
                }
            }
        }
        Self { files, dirs }
    }
}

/// Why a path on disk was not synced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// The bytes match a secret pattern; they never leave the directory.
    Secret { rule: String },
    /// At least the blob size limit.
    TooLarge { bytes: u64 },
    /// A symlink whose target is absolute or leaves the directory.
    SymlinkEscapes,
    /// A name a tree cannot hold (not UTF-8, `.git`, …).
    InvalidPath,
    /// A FIFO, socket or device.
    Special,
    /// The file kept changing while it was read.
    Unstable,
    /// The file or directory could not be read.
    Unreadable { error: String },
}

impl SkipReason {
    /// A short explanation for agents.
    pub fn describe(&self) -> String {
        match self {
            Self::Secret { rule } => {
                format!("looks like it contains a secret ({rule}); it never leaves this directory")
            }
            Self::TooLarge { bytes } => format!("{bytes} bytes, over the size limit"),
            Self::SymlinkEscapes => "a symlink that points outside the workspace".into(),
            Self::InvalidPath => "a name Aver cannot store".into(),
            Self::Special => "not a regular file, directory or symlink".into(),
            Self::Unstable => "kept changing while being read; sync again".into(),
            Self::Unreadable { error } => format!("could not be read: {error}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    #[serde(flatten)]
    pub reason: SkipReason,
    /// The base tracks the path, so verification sees its last synced
    /// version while the directory holds something else.
    #[serde(default)]
    pub tracked: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanStats {
    /// Entries looked at.
    pub scanned: u64,
    /// Files read and hashed (stat cache misses).
    pub hashed: u64,
    pub took_ms: u64,
}

/// What a sync found, returned with every verify of a disk workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    /// The workspace version after the sync.
    pub version: u64,
    /// Files the workspace adds, modifies and deletes against its base.
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
    /// Overlay rows this sync changed (0 when the directory is unchanged).
    pub updated: u32,
    /// The paths that differ from the base, sorted, at most
    /// [`SyncReport::LISTED`] of them.
    #[serde(default)]
    pub changed: Vec<String>,
    pub skipped: Vec<Skipped>,
    #[serde(flatten)]
    pub stats: ScanStats,
}

impl SyncReport {
    /// How many changed paths a report lists.
    pub const LISTED: usize = 50;

    /// Files that differ from the base.
    pub fn differ(&self) -> u32 {
        self.added + self.modified + self.deleted
    }

    /// Whether the directory changed something and every changed file is
    /// documentation, configuration or data rather than source code
    /// ([`crate::coverage::language`]), as far as the listed paths show.
    pub fn non_code_only(&self) -> bool {
        self.differ() > 0
            && self.changed.len() == self.differ() as usize
            && self
                .changed
                .iter()
                .all(|p| crate::coverage::language(p).is_none())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatState {
    Live,
    Released,
    Lost,
}

impl MatState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Released => "released",
            Self::Lost => "lost",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Live, Self::Released, Self::Lost]
            .into_iter()
            .find(|m| m.as_str() == s)
    }
}

/// A block of ports reserved for a workspace's servers (`aver run` allocates them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ports {
    pub base: u16,
    pub count: u16,
}

/// The `materializations` row (§1.8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Materialization {
    pub workspace: WorkspaceId,
    pub dir: PathBuf,
    /// The repository the base was imported from, when known.
    pub repo: Option<PathBuf>,
    pub checkout: TreeId,
    pub clone: CloneStrategy,
    /// Untracked paths cloned from the repo (its dependencies, `seed` and
    /// `warm`) or reserved for Aver's tools; never synced.
    pub seeded: Vec<String>,
    /// Base paths the checkout could not write; never treated as deleted.
    pub absent: Vec<String>,
    pub git_view: bool,
    pub ports: Option<Ports>,
    pub state: MatState,
    pub created_at: Timestamp,
    pub released_at: Option<Timestamp>,
}

/// The `[workspace]` table of `aver.toml`, read from the base (§1.4.1). It
/// is ignored by environment parsing, so it changes no environment id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// Read and ignored: kept so an `aver.toml` from before Aver 0.4 parses.
    #[serde(default)]
    pub node_modules: Ignored,
    #[serde(default)]
    pub clone_packages: Ignored,
    #[serde(default)]
    pub seed: Vec<String>,
    #[serde(default)]
    pub warm: Vec<String>,
    #[serde(default)]
    pub ports: Option<u16>,
}

impl WorkspaceConfig {
    /// Reads `[workspace]` from `aver.toml` text; a missing table is the default.
    pub fn from_aver_toml(text: &str) -> Result<Self, DiskError> {
        let table: toml::Table = text
            .parse()
            .map_err(|e| DiskError::Invalid(format!("aver.toml: {e}")))?;
        match table.get("workspace") {
            None => Ok(Self::default()),
            Some(value) => value
                .clone()
                .try_into()
                .map_err(|e| DiskError::Invalid(format!("aver.toml [workspace]: {e}"))),
        }
    }
}

/// A value of any shape, accepted and dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ignored;

impl<'de> Deserialize<'de> for Ignored {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(d).map(|_| Self)
    }
}

/// Bounds for [`Disk::gc`].
#[derive(Debug, Clone)]
pub struct GcPolicy {
    pub checkout_idle: Duration,
    pub checkout_cap_bytes: u64,
    /// A live workspace whose directory has not synced or built for this
    /// long loses its build artifacts: its Cargo `target` directories and
    /// its `aver check` copies.
    pub build_idle: Duration,
    /// Also release the live directories no one can work in any more
    /// (`aver gc --accepted`): those of closed workspaces (accepted,
    /// committed or discarded) and of workspaces whose owner's key is
    /// revoked (the other attempts of an accepted or discarded run). One
    /// whose Git view holds commits the user's repository lacks, or cannot
    /// be checked, is kept and listed.
    pub accepted: bool,
}

impl Default for GcPolicy {
    fn default() -> Self {
        Self {
            checkout_idle: Duration::from_secs(24 * 3600),
            checkout_cap_bytes: 1 << 30,
            build_idle: Duration::from_secs(24 * 3600),
            accepted: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcReport {
    /// Workspace directories moved to the trash (released or orphaned).
    pub trashed: u32,
    /// Live materializations whose directory vanished.
    pub lost: u32,
    pub checkouts_removed: u32,
    /// Build artifact directories of idle live workspaces moved to the
    /// trash: Cargo `target` directories and `aver check` copies.
    #[serde(default)]
    pub builds_reclaimed: u32,
    /// Their apparent size ([`apparent_bytes`]).
    #[serde(default)]
    pub build_bytes: u64,
    /// Entries deleted from the trash.
    pub trash_emptied: u32,
    /// Directories of finished workspaces released ([`GcPolicy::accepted`]).
    #[serde(default)]
    pub finished_released: u32,
    /// Directories of finished workspaces kept, each with why.
    #[serde(default)]
    pub finished_kept: Vec<String>,
    /// Slots (ARCHITECTURE.md §1.8.5) removed: see
    /// [`crate::place::sides::Sides::stale`].
    #[serde(default)]
    pub slots_removed: u32,
    /// Their apparent size.
    #[serde(default)]
    pub slot_bytes: u64,
}

/// An exclusive `flock` held until drop. Two opens of the same file
/// conflict even within one process, so it serializes threads and
/// processes alike.
pub(crate) struct FileLock(fs::File);

impl FileLock {
    pub(crate) fn exclusive(path: &FsPath) -> io::Result<Self> {
        Self::lock(path, libc::LOCK_EX)
    }

    /// A shared `flock`: any number of holders, none while an exclusive
    /// one is held.
    pub(crate) fn shared(path: &FsPath) -> io::Result<Self> {
        Self::lock(path, libc::LOCK_SH)
    }

    fn lock(path: &FsPath, how: libc::c_int) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        loop {
            // SAFETY: the descriptor is open for the duration of the call.
            if unsafe { libc::flock(file.as_raw_fd(), how) } == 0 {
                return Ok(Self(file));
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }

    /// The lock at `path` if nobody holds it now; `None` if somebody does
    /// or there is no such file.
    pub(crate) fn try_exclusive(path: &FsPath) -> io::Result<Option<Self>> {
        let file = match fs::OpenOptions::new().write(true).open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        loop {
            // SAFETY: the descriptor is open for the duration of the call.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Some(Self(file)));
            }
            let err = io::Error::last_os_error();
            match err.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::WouldBlock => return Ok(None),
                _ => return Err(err),
            }
        }
    }

    /// The locked file's metadata.
    pub(crate) fn metadata(&self) -> io::Result<fs::Metadata> {
        self.0.metadata()
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // SAFETY: the descriptor is still open.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// The host side of disk-mode workspaces under one `$AVER_HOME`.
pub struct Disk {
    store: Arc<Store>,
    config: DiskConfig,
    strategy: OnceLock<CloneStrategy>,
    folds: OnceLock<bool>,
    bases: Mutex<HashMap<TreeId, Arc<BaseFiles>>>,
}

/// Base file sets kept in memory, at most.
const BASES_CACHED: usize = 16;

impl Disk {
    pub fn new(store: Arc<Store>, config: DiskConfig) -> Self {
        Self {
            store,
            config,
            strategy: OnceLock::new(),
            folds: OnceLock::new(),
            bases: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &DiskConfig {
        &self.config
    }

    pub fn root(&self) -> &FsPath {
        &self.config.root
    }

    /// `$AVER_HOME/work.lock`: held shared by a materialize from its
    /// checkout to its row, exclusive by [`Self::gc`], so gc never takes a
    /// directory still being made for an orphan.
    pub(crate) fn work_lock(&self) -> io::Result<FileLock> {
        FileLock::shared(&self.work_lock_path())
    }

    fn work_lock_path(&self) -> PathBuf {
        self.root().join("work.lock")
    }

    pub(crate) fn work_dir(&self) -> PathBuf {
        self.root().join("work")
    }

    pub(crate) fn mat_dir(&self, ws: &WorkspaceId) -> PathBuf {
        self.root().join("mat").join(ws.to_string())
    }

    /// Where `aver check` ran the transplant for `ws` before WP3; a
    /// release still removes what an older Aver left there.
    pub fn check_dir(&self, ws: &WorkspaceId) -> PathBuf {
        self.root().join("check").join(ws.to_string())
    }

    /// Like [`Self::check_dir`], for the judged head.
    pub fn judged_dir(&self, ws: &WorkspaceId) -> PathBuf {
        self.root().join("judged").join(ws.to_string())
    }

    /// Like [`Self::check_dir`], for `aver check --combined`'s trial merge.
    pub fn combined_dir(&self, ws: &WorkspaceId) -> PathBuf {
        self.root().join("combined").join(ws.to_string())
    }

    /// The Git view's repository for `ws` (§2.3.1).
    pub fn git_dir(&self, ws: &WorkspaceId) -> PathBuf {
        gitview::view_dir(self.root(), ws)
    }

    fn trash_dir(&self) -> PathBuf {
        self.root().join("trash")
    }

    /// The clone strategy for this home: forced, else probed once.
    pub fn strategy(&self) -> Result<CloneStrategy, DiskError> {
        if let Some(s) = self.strategy.get() {
            return Ok(*s);
        }
        let picked = match self.config.clone {
            Some(forced) if !forced.supported() => {
                return Err(DiskError::Unsupported(forced.as_str()))
            }
            Some(forced) => forced,
            None => clone::probe(&self.root().join("checkouts"), &self.work_dir())?,
        };
        Ok(*self.strategy.get_or_init(|| picked))
    }

    /// Whether checkouts must fold case: forced, else probed on the volume.
    pub(crate) fn folds_case(&self) -> Result<bool, DiskError> {
        if let Some(f) = self.config.case_insensitive {
            return Ok(f);
        }
        if let Some(f) = self.folds.get() {
            return Ok(*f);
        }
        let dir = self.root().join("checkouts");
        fs::create_dir_all(&dir)?;
        let probe = dir.join(format!("case-{}-a", ulid::Ulid::generate()));
        fs::write(&probe, b"")?;
        let upper = dir.join(
            probe
                .file_name()
                .expect("probe has a name")
                .to_string_lossy()
                .to_uppercase(),
        );
        let folds = upper.exists();
        let _ = fs::remove_file(&probe);
        Ok(*self.folds.get_or_init(|| folds))
    }

    /// The files of base tree `tree`, from memory or `load`.
    pub(crate) fn base_files(
        &self,
        tree: &TreeId,
        load: impl FnOnce() -> Result<Vec<(String, FileEntry)>, DiskError>,
    ) -> Result<Arc<BaseFiles>, DiskError> {
        if let Some(hit) = self.bases.lock().get(tree) {
            return Ok(hit.clone());
        }
        let base = Arc::new(BaseFiles::new(load()?));
        let mut bases = self.bases.lock();
        if bases.len() >= BASES_CACHED {
            bases.clear();
        }
        bases.insert(*tree, base.clone());
        Ok(base)
    }

    /// Ignore files outside the workspace that apply to its untracked paths.
    pub(crate) fn extra_ignores(&self, repo: Option<&FsPath>) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Some(repo) = repo {
            out.push(repo.join(".git").join("info").join("exclude"));
        }
        if self.config.global_excludes {
            if let Some(global) = ignore::gitignore::gitconfig_excludes_path() {
                out.push(global);
            }
        }
        out
    }

    /// Moves `path` into the trash, which is instant on one volume.
    pub(crate) fn trash(&self, path: &FsPath) -> io::Result<PathBuf> {
        fs::create_dir_all(self.trash_dir())?;
        let dest = self.trash_dir().join(ulid::Ulid::generate().to_string());
        fs::rename(path, &dest)?;
        Ok(dest)
    }

    /// Deletes everything in the trash and returns how many entries went.
    pub fn empty_trash(&self) -> io::Result<u32> {
        let Ok(entries) = fs::read_dir(self.trash_dir()) else {
            return Ok(0);
        };
        let mut removed = 0;
        for entry in entries {
            let path = entry?.path();
            // A background emptier may delete the same entry concurrently.
            let gone = |r: io::Result<()>| match r {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            };
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => gone(remove_tree(&path))?,
                Ok(_) => gone(fs::remove_file(&path))?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            }
            removed += 1;
        }
        Ok(removed)
    }

    /// Deletes the trash on a background thread.
    pub(crate) fn empty_trash_later(self: &Arc<Self>) {
        let disk = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = disk.empty_trash() {
                tracing::warn!(error = %e, "emptying the trash failed");
            }
        });
    }

    /// The materialization of `ws`, if it has one.
    pub fn materialization(&self, ws: &WorkspaceId) -> Result<Option<Materialization>, DiskError> {
        Ok(self.store.read(|conn| load_materialization(conn, ws))?)
    }

    /// Every materialization, newest first; only live ones unless `all`.
    pub fn materializations(&self, all: bool) -> Result<Vec<Materialization>, DiskError> {
        Ok(self.store.read(|conn| {
            let ids: Vec<String> = conn
                .prepare(
                    "SELECT workspace FROM materializations
                     WHERE ?1 OR state = 'live' ORDER BY created_at DESC, workspace DESC",
                )?
                .query_map([all], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                let ws = id.parse().map_err(|e| StoreError::Corrupt {
                    id: id.clone(),
                    reason: format!("{e}"),
                })?;
                if let Some(m) = load_materialization(conn, &ws)? {
                    out.push(m);
                }
            }
            Ok(out)
        })?)
    }

    pub(crate) fn set_state(&self, ws: &WorkspaceId, state: MatState) -> Result<(), DiskError> {
        self.store.tx(|tx| {
            tx.sql.execute(
                "UPDATE materializations SET state = ?2,
                   released_at = CASE WHEN ?2 = 'released' THEN ?3 ELSE released_at END
                 WHERE workspace = ?1",
                params![ws.to_string(), state.as_str(), Timestamp::now().to_string()],
            )?;
            Ok::<_, DiskError>(())
        })
    }

    /// Moves a live directory to the trash and marks it released. Releasing
    /// a released workspace is a no-op.
    pub(crate) fn release(self: &Arc<Self>, ws: &WorkspaceId) -> Result<(), DiskError> {
        let Some(m) = self.materialization(ws)? else {
            return Ok(());
        };
        if m.state == MatState::Released {
            return Ok(());
        }
        // Marked first: a crash between the two leaves a released row
        // whose directory gc trashes, never a live row without its
        // directory.
        self.set_state(ws, MatState::Released)?;
        self.release_dirs(&m)?;
        crate::fault::point("release.trashed")?;
        self.empty_trash_later();
        if let Err(e) = Sides::new(self.root(), self.store.clone()).remove_for(ws) {
            tracing::warn!(error = %e, "removing a released workspace's slots failed");
        }
        Ok(())
    }

    /// Moves a materialization's directory, Git view and `aver check`
    /// copies to the trash.
    fn release_dirs(&self, m: &Materialization) -> Result<(), DiskError> {
        let ws = &m.workspace;
        if m.dir.exists() {
            self.trash(&m.dir)?;
        }
        let _ = fs::remove_dir_all(self.mat_dir(ws));
        let git = self.git_dir(ws);
        if git.exists() {
            self.trash(&git)?;
        }
        for check in [
            self.check_dir(ws),
            self.judged_dir(ws),
            self.combined_dir(ws),
        ] {
            if check.exists() {
                self.trash(&check)?;
            }
        }
        Ok(())
    }

    /// Releases the live directories of finished workspaces for
    /// [`GcPolicy::accepted`], keeping those whose Git view has commits
    /// the user's repository lacks, committed ones whose commit no branch
    /// holds, and those `keep` names a reason for.
    fn release_finished(
        &self,
        report: &mut GcReport,
        keep: &dyn Fn(&Materialization) -> Option<String>,
    ) -> Result<(), DiskError> {
        // A committed workspace is exported when a transition recording its
        // head maps to a Git commit (`git_map`).
        let finished: Vec<(String, bool)> = self.store.read(|conn| {
            Ok(conn
                .prepare(
                    "SELECT m.workspace, w.state = 'committed' AND NOT EXISTS (
                         SELECT 1 FROM transitions_by_snapshot t
                         JOIN git_map x ON x.aver_id = t.transition AND x.kind = 'commit'
                         WHERE t.snapshot = w.head)
                     FROM materializations m
                     JOIN workspaces w ON w.id = m.workspace
                     LEFT JOIN agents g ON g.id = w.owner
                     WHERE m.state = 'live' AND (w.state != 'open' OR g.revoked_at IS NOT NULL)
                     ORDER BY m.created_at",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?)
        })?;
        for (id, unexported) in finished {
            let Ok(ws) = id.parse::<WorkspaceId>() else {
                continue;
            };
            let Some(m) = self.materialization(&ws)? else {
                continue;
            };
            if unexported {
                report.finished_kept.push(format!(
                    "{}: it was committed but never exported to a branch; `aver accept` in it exports it",
                    m.dir.display()
                ));
                continue;
            }
            if let Some(why) = keep(&m) {
                report
                    .finished_kept
                    .push(format!("{}: {why}", m.dir.display()));
                continue;
            }
            let view = self.git_dir(&ws);
            if view.is_dir() {
                let why = match gitview::unfetched(&view) {
                    Ok(lost) if lost.is_empty() => None,
                    Ok(lost) => Some(format!(
                        "its Git view has {} commit(s) the repository lacks; `aver workspace rm {}` says how to fetch them",
                        lost.len(),
                        m.dir.display()
                    )),
                    Err(e) => Some(format!("its Git view could not be checked ({e})")),
                };
                if let Some(why) = why {
                    report
                        .finished_kept
                        .push(format!("{}: {why}", m.dir.display()));
                    continue;
                }
            }
            self.set_state(&ws, MatState::Released)?;
            self.release_dirs(&m)?;
            report.finished_released += 1;
            crate::fault::point("gc.released")?;
        }
        Ok(())
    }

    /// Moves the build artifacts of the live workspace `m` to the trash
    /// when it has been idle for `policy.build_idle`: no sync (its stat
    /// cache) and no build (its `target` directories) since. The artifacts
    /// are its Cargo `target` directories (warm paths, and a `target` at
    /// the root) and its `aver check` copies; a later build or check makes
    /// them again.
    fn reclaim_builds(
        &self,
        m: &Materialization,
        policy: &GcPolicy,
        report: &mut GcReport,
    ) -> Result<(), DiskError> {
        let mtime = |p: &FsPath| fs::metadata(p).and_then(|m| m.modified()).ok();
        let ws = m.workspace.to_string();
        let mut targets: Vec<PathBuf> = m
            .seeded
            .iter()
            .chain(std::iter::once(&"target".to_owned()))
            .map(|rel| m.dir.join(rel))
            .collect();
        targets.sort();
        targets.dedup();
        targets.retain(|t| is_target(t));
        let copies: Vec<PathBuf> = ["check", "judged", "combined"]
            .iter()
            .map(|kind| self.root().join(kind).join(&ws))
            .filter(|dir| dir.exists())
            .collect();
        if targets.is_empty() && copies.is_empty() {
            return Ok(());
        }
        let last = targets
            .iter()
            .filter_map(|t| last_built(t))
            .chain(mtime(&self.mat_dir(&m.workspace).join("stat")))
            .chain(copies.iter().filter_map(|c| mtime(c)))
            .chain(copies.iter().filter_map(|c| last_built(&c.join("target"))))
            .max();
        let idle = last
            .and_then(|t| t.elapsed().ok())
            .is_none_or(|age| age >= policy.build_idle);
        if !idle {
            return Ok(());
        }
        for dir in targets.iter().chain(&copies) {
            report.build_bytes += apparent_bytes(dir);
            self.trash(dir)?;
            report.builds_reclaimed += 1;
        }
        Ok(())
    }

    /// The repository of the newest workspace directory made on `base`.
    pub fn repo_of_base(&self, base: &aver_core::SnapshotId) -> Option<PathBuf> {
        self.store
            .read(|conn| {
                use rusqlite::OptionalExtension as _;
                Ok(conn
                    .query_row(
                        "SELECT m.repo FROM materializations m JOIN workspaces w ON w.id = m.workspace
                         WHERE w.base = ?1 AND m.repo IS NOT NULL
                         ORDER BY m.created_at DESC LIMIT 1",
                        [base.to_string()],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?)
            })
            .ok()
            .flatten()
            .map(PathBuf::from)
    }

    /// The repository of a workspace made here that holds commit `sha`,
    /// the newest workspace's repository asked first, each only once: the
    /// repository whose base pool a warm-up of a base imported from that
    /// commit belongs in, before any workspace on it exists.
    pub fn repo_with_commit(&self, sha: &aver_core::GitSha) -> Option<PathBuf> {
        let rows = self.materializations(true).ok()?;
        let mut asked = HashSet::new();
        let object = format!("{}^{{commit}}", sha.as_str());
        rows.into_iter().filter_map(|m| m.repo).find(|repo| {
            if !asked.insert(repo.clone()) || !repo.is_dir() {
                return false;
            }
            let mut git = std::process::Command::new("git");
            for var in [
                "GIT_DIR",
                "GIT_WORK_TREE",
                "GIT_COMMON_DIR",
                "GIT_INDEX_FILE",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            ] {
                git.env_remove(var);
            }
            git.arg("-C")
                .arg(repo)
                .args(["cat-file", "-e", &object])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    }

    /// Collects garbage (§2.3.1): released and orphaned workspace
    /// directories go to the trash, live rows whose directory vanished
    /// become `lost`, idle checkouts beyond their cap and stale slots go,
    /// and the trash is emptied. Nothing a live materialization references is
    /// ever removed.
    ///
    /// Without a scan of the directories, this keeps every committed
    /// workspace's directory only by its export; `Workspaces::gc` also
    /// keeps one that changed since its commit.
    pub fn gc(&self, policy: &GcPolicy) -> Result<GcReport, DiskError> {
        self.gc_keeping(policy, &|_| None)
    }

    /// [`Self::gc`], also keeping the finished directories `keep` names a
    /// reason for.
    pub fn gc_keeping(
        &self,
        policy: &GcPolicy,
        keep: &dyn Fn(&Materialization) -> Option<String>,
    ) -> Result<GcReport, DiskError> {
        let mut report = GcReport::default();
        // Exclusive until the stores are pruned: a materialize between its
        // checkout and its row (shared) is neither an orphan nor unused.
        let work = FileLock::exclusive(&self.work_lock_path())?;
        if policy.accepted {
            self.release_finished(&mut report, keep)?;
        }
        let rows = self.materializations(true)?;
        let live: Vec<&Materialization> =
            rows.iter().filter(|m| m.state == MatState::Live).collect();
        for m in &rows {
            match m.state {
                MatState::Live if !m.dir.exists() => {
                    self.set_state(&m.workspace, MatState::Lost)?;
                    report.lost += 1;
                }
                MatState::Released | MatState::Lost if m.dir.exists() => {
                    self.trash(&m.dir)?;
                    report.trashed += 1;
                    crate::fault::point("gc.trashed")?;
                }
                _ => {}
            }
        }
        let live_dirs: HashSet<PathBuf> = live.iter().map(|m| m.dir.clone()).collect();
        if let Ok(repos) = fs::read_dir(self.work_dir()) {
            for repo in repos {
                let repo = repo?.path();
                if !repo.is_dir() {
                    continue;
                }
                for dir in fs::read_dir(&repo)? {
                    let dir = dir?.path();
                    if !live_dirs.contains(&dir) {
                        self.trash(&dir)?;
                        report.trashed += 1;
                        crate::fault::point("gc.trashed")?;
                    }
                }
                let _ = fs::remove_dir(&repo);
            }
        }
        for m in &live {
            self.reclaim_builds(m, policy, &mut report)?;
        }
        let live_ids: HashSet<String> = live.iter().map(|m| m.workspace.to_string()).collect();
        for kind in ["git", "check", "judged", "combined"] {
            let Ok(dirs) = fs::read_dir(self.root().join(kind)) else {
                continue;
            };
            for dir in dirs {
                let dir = dir?;
                if !live_ids.contains(&*dir.file_name().to_string_lossy()) {
                    self.trash(&dir.path())?;
                }
            }
        }

        let now = Timestamp::now().unix_millis();
        let in_use: HashSet<String> = live.iter().map(|m| m.checkout.to_string()).collect();
        let checkouts: Vec<(String, i64, String)> = self.store.read(|conn| {
            Ok(conn
                .prepare(
                    "SELECT tree, bytes, last_used_at FROM checkouts ORDER BY last_used_at DESC",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?)
        })?;
        let mut kept_bytes = 0u64;
        for (tree, bytes, used) in checkouts {
            let idle = now - used.parse::<Timestamp>().map_or(0, |t| t.unix_millis());
            let over = kept_bytes + bytes as u64 > policy.checkout_cap_bytes;
            if !in_use.contains(&tree) && (idle > policy.checkout_idle.as_millis() as i64 || over) {
                let dir = self.root().join("checkouts").join(&tree);
                if dir.exists() {
                    self.trash(&dir)?;
                }
                let _ = fs::remove_file(
                    self.root()
                        .join("checkouts")
                        .join(format!("{tree}.manifest")),
                );
                self.store.tx(|tx| {
                    tx.sql
                        .execute("DELETE FROM checkouts WHERE tree = ?1", [&tree])?;
                    Ok::<_, DiskError>(())
                })?;
                report.checkouts_removed += 1;
            } else {
                kept_bytes += bytes as u64;
            }
        }

        drop(work);
        // Base slots are the seed of every judged side, so they stay a week.
        let sides = Sides::new(self.root(), self.store.clone());
        let on_disk = || live.iter().filter(|m| m.dir.exists());
        let live_ws: BTreeSet<WorkspaceId> = on_disk().map(|m| m.workspace).collect();
        let live_bases: BTreeSet<TreeId> = on_disk().map(|m| m.checkout).collect();
        let idle = Duration::from_secs(7 * 24 * 3600).max(policy.build_idle);
        for dir in sides.stale(&live_bases, &live_ws, idle) {
            let bytes = apparent_bytes(&dir);
            if remove_slot(&dir)? {
                report.slots_removed += 1;
                report.slot_bytes += bytes;
            }
        }
        report.trash_emptied = self.empty_trash()?;
        Ok(report)
    }
}

/// Inserts a materialization row inside the caller's transaction. A
/// released or lost row naming the same directory is deleted first: the
/// caller chose the directory because nothing is there, so that row only
/// records a directory already trashed, and keeping it would block the
/// label (`dir` is unique).
pub(crate) fn insert_materialization(
    conn: &Connection,
    m: &Materialization,
) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM materializations WHERE dir = ?1 AND state != 'live'",
        params![m.dir.to_string_lossy()],
    )?;
    conn.execute(
        "INSERT INTO materializations (workspace, dir, repo, checkout, clone, farms, seeded, absent,
           git_view, port_base, port_count, state, created_at, released_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            m.workspace.to_string(),
            m.dir.to_string_lossy(),
            m.repo.as_ref().map(|r| r.to_string_lossy().into_owned()),
            m.checkout.to_string(),
            m.clone.as_str(),
            "[]",
            serde_json::to_string(&m.seeded).expect("paths serialize"),
            serde_json::to_string(&m.absent).expect("paths serialize"),
            m.git_view,
            m.ports.map(|p| p.base),
            m.ports.map(|p| p.count),
            m.state.as_str(),
            m.created_at.to_string(),
            m.released_at.map(|t| t.to_string()),
        ],
    )?;
    Ok(())
}

pub(crate) fn load_materialization(
    conn: &Connection,
    ws: &WorkspaceId,
) -> Result<Option<Materialization>, StoreError> {
    type Raw = (
        String,
        Option<String>,
        String,
        String,
        String,
        String,
        String,
        bool,
        Option<u16>,
        Option<u16>,
        String,
        String,
        Option<String>,
    );
    let row: Option<Raw> = conn
        .query_row(
            "SELECT dir, repo, checkout, clone, farms, seeded, absent, git_view, port_base,
               port_count, state, created_at, released_at
             FROM materializations WHERE workspace = ?1",
            [ws.to_string()],
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
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                ))
            },
        )
        .optional()?;
    let Some((
        dir,
        repo,
        checkout,
        clone,
        farms,
        seeded,
        absent,
        git_view,
        pb,
        pc,
        state,
        created,
        released,
    )) = row
    else {
        return Ok(None);
    };
    let bad = |reason: String| StoreError::Corrupt {
        id: ws.to_string(),
        reason,
    };
    Ok(Some(Materialization {
        workspace: *ws,
        dir: PathBuf::from(dir),
        repo: repo.map(PathBuf::from),
        checkout: checkout.parse().map_err(|e| bad(format!("{e}")))?,
        clone: CloneStrategy::parse(&clone).ok_or_else(|| bad(format!("clone {clone:?}")))?,
        seeded: with_farms(&seeded, &farms).map_err(|e| bad(e.to_string()))?,
        absent: serde_json::from_str(&absent).map_err(|e| bad(e.to_string()))?,
        git_view,
        ports: pb.zip(pc).map(|(base, count)| Ports { base, count }),
        state: MatState::parse(&state).ok_or_else(|| bad(format!("state {state:?}")))?,
        created_at: created.parse().map_err(|e| bad(format!("{e}")))?,
        released_at: released
            .map(|t| t.parse())
            .transpose()
            .map_err(|e| bad(format!("{e}")))?,
    }))
}

/// The paths a scan must never treat as content for `m`.
pub(crate) fn exclusions(m: &Materialization) -> scan::Exclusions {
    let mut silent: HashSet<String> = HashSet::from([".aver".to_owned()]);
    silent.extend(m.seeded.iter().cloned());
    scan::Exclusions {
        silent,
        absent: m.absent.iter().cloned().collect(),
    }
}

/// Counts what a desired overlay changes against its base.
pub(crate) fn counts(
    base: &BaseFiles,
    desired: &BTreeMap<String, Option<FileEntry>>,
) -> (u32, u32, u32) {
    let (mut added, mut modified, mut deleted) = (0, 0, 0);
    for (path, row) in desired {
        match (base.files.contains_key(path), row) {
            (_, None) => deleted += 1,
            (true, Some(_)) => modified += 1,
            (false, Some(_)) => added += 1,
        }
    }
    (added, modified, deleted)
}

/// The `seeded` paths of a row, with the `dir` of each farm a row from
/// before Aver 0.4 recorded: those directories are still in the workspace
/// and must stay out of its scans.
fn with_farms(seeded: &str, farms: &str) -> serde_json::Result<Vec<String>> {
    #[derive(Deserialize)]
    struct OldFarm {
        dir: String,
    }
    let mut paths: Vec<String> = serde_json::from_str(seeded)?;
    for farm in serde_json::from_str::<Vec<OldFarm>>(farms)? {
        if !paths.contains(&farm.dir) {
            paths.push(farm.dir);
        }
    }
    Ok(paths)
}

/// Whether `dir` is a Cargo `target` directory: it carries Cargo's
/// `CACHEDIR.TAG` and a profile directory with a `.fingerprint`.
pub fn is_target(dir: &FsPath) -> bool {
    dir.join("CACHEDIR.TAG").is_file()
        && profile_dirs(dir).is_ok_and(|profiles| !profiles.is_empty())
}

/// The profile directories of a `target` directory, relative to it:
/// `debug`, `release` and the like, and `<triple>/<profile>` for a
/// cross-compiled target.
fn profile_dirs(target: &FsPath) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(target)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = PathBuf::from(entry.file_name());
        if entry.path().join(".fingerprint").is_dir() {
            found.push(name);
            continue;
        }
        let Ok(inner) = fs::read_dir(entry.path()) else {
            continue;
        };
        for sub in inner.flatten() {
            if sub.path().join(".fingerprint").is_dir() {
                found.push(name.join(sub.file_name()));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The newest modification time of a `target` directory and its profile
/// directories: Cargo replaces uplifted outputs in a profile directory on
/// every build that changes something, so this is about when it last
/// built.
pub fn last_built(target: &FsPath) -> Option<std::time::SystemTime> {
    let mut newest = fs::metadata(target).ok()?.modified().ok()?;
    for profile in profile_dirs(target).unwrap_or_default() {
        if let Ok(time) = fs::metadata(target.join(profile)).and_then(|m| m.modified()) {
            newest = newest.max(time);
        }
    }
    Some(newest)
}

/// The sum of the lengths of the files under `path`, not following
/// symlinks, each inode once. Copy-on-write clones share blocks, so this
/// is an upper bound on what removing `path` frees.
pub fn apparent_bytes(path: &FsPath) -> u64 {
    fn walk(path: &FsPath, seen: &mut HashSet<(u64, u64)>) -> u64 {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return 0;
        };
        if meta.is_dir() {
            return fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| walk(&e.path(), seen))
                .sum();
        }
        if meta.is_file() && seen.insert((meta.dev(), meta.ino())) {
            meta.len()
        } else {
            0
        }
    }
    walk(path, &mut HashSet::new())
}

/// Gives the owner write access to every directory and file of a tree.
pub(crate) fn make_writable(dir: &FsPath) -> io::Result<()> {
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(meta.mode() | 0o200))?;
    if meta.is_dir() {
        for entry in fs::read_dir(dir)? {
            make_writable(&entry?.path())?;
        }
    }
    Ok(())
}

/// Removes a tree, restoring write access first if a read-only one
/// refuses the removal.
pub(crate) fn remove_tree(dir: &FsPath) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            make_writable(dir)?;
            fs::remove_dir_all(dir)
        }
    }
}
