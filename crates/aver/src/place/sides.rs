//! Local slots (ARCHITECTURE.md §1.8.5): one directory per side of a
//! verification, cloned copy-on-write from a directory that already has
//! the repository's dependencies installed (the agent's workspace, the
//! user's checkout, a CI checkout), then swapped to the side's tree by
//! [`Workdir::become_tree`]. A slot is never the agent's own directory, so
//! a run never changes what the agent is editing.
//!
//! Layout under `$AVER_HOME/sides/<repo16>/`: `B-<tree16>` (a base),
//! `J-<ws>` (a workspace's judged head and its head-tests tree), `T-<ws>`
//! (its transplant) and `S-<snap16>` (a run outside a verification), each
//! with `<slot>.lock`, `<slot>.pid`, `<slot>.swap.json` (the
//! [`Workdir`] journal), `<slot>.source.json` (what it was cloned from) and
//! `<slot>.runs/` beside it.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use aver_core::{BlobId, DepsState, Path, SnapshotId, TreeId, WorkspaceId};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::workdir::{Changed, Keep, StaleCopy, Workdir, WorkdirError};
use crate::disk::clone::{self, CloneStrategy};
use crate::disk::{BaseFiles, FileLock};
use crate::store::Store;
use crate::workspace::{FileKind, Glob};

/// Which slot a run uses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SlotName {
    /// `B-<tree16>`: the base tree, shared by every workspace on it.
    Base(TreeId),
    /// `J-<ws>`: a workspace's judged head and head-tests tree.
    Judged(WorkspaceId),
    /// `T-<ws>`: a workspace's transplant, apart from `J-` so head-written
    /// tests never run where the base's build lives and the two can run at
    /// once.
    Transplant(WorkspaceId),
    /// `S-<snap16>`: a run outside any verification, so head code never
    /// writes a base slot.
    Snapshot(SnapshotId),
}

impl SlotName {
    pub fn dir(&self) -> String {
        let short = |s: String| s[s.len().saturating_sub(16)..].to_owned();
        match self {
            Self::Base(tree) => format!("B-{}", short(tree.to_string())),
            Self::Judged(ws) => format!("J-{ws}"),
            Self::Transplant(ws) => format!("T-{ws}"),
            Self::Snapshot(snap) => format!("S-{}", short(snap.to_string())),
        }
    }
}

/// What a slot is cloned from, and how its dependencies are laid out
/// there: `keep` (installed dependencies, shared read-only by intent),
/// `seed` (build caches, cloned once per slot) and `inputs` (the
/// lockfiles the kept directories were installed from), as patterns of
/// [`Keep`].
#[derive(Debug, Clone)]
pub struct SlotSource<'a> {
    pub dir: &'a FsPath,
    pub keep: Vec<String>,
    pub seed: Vec<String>,
    pub inputs: Vec<String>,
}

/// Where a repository's dependencies live, from its environment's
/// layers (the bridge until the dependency contract replaces
/// `Environment`): `node_modules` beside each npm lockfile and `.venv`
/// beside each uv or pip one are kept, `target` beside each `Cargo.lock`
/// is seeded, and every lockfile is an input. `node_modules` and `.venv`
/// are kept at any depth whatever installed them; without a layer, their
/// inputs are [`NODE_INPUTS`] and [`PYTHON_INPUTS`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deps {
    pub keep: Vec<String>,
    pub seed: Vec<String>,
    pub inputs: Vec<String>,
}

impl Deps {
    /// What a swap leaves alone: `keep` and `seed`.
    pub fn keep_all(&self) -> Result<Keep, PlaceError> {
        Keep::new(self.keep.iter().chain(&self.seed).map(String::as_str))
            .map_err(PlaceError::Pattern)
    }

    /// The dependency state of `dir` under this layout.
    pub fn state_of(&self, dir: &FsPath) -> Result<DepsState, PlaceError> {
        Ok(deps_state(dir, &globs(&self.inputs)?, &self.keep_all()?))
    }

    /// Both layouts' paths, once each (an in-place checkout keeps what
    /// either side installs).
    pub fn union(&self, other: &Deps) -> Deps {
        let merge = |a: &[String], b: &[String]| {
            let mut out = a.to_vec();
            out.extend(b.iter().filter(|p| !a.contains(p)).cloned());
            out
        };
        Deps {
            keep: merge(&self.keep, &other.keep),
            seed: merge(&self.seed, &other.seed),
            inputs: merge(&self.inputs, &other.inputs),
        }
    }

    /// This layout for clones from `dir`.
    pub fn source<'a>(&self, dir: &'a FsPath) -> SlotSource<'a> {
        SlotSource {
            dir,
            keep: self.keep.clone(),
            seed: self.seed.clone(),
            inputs: self.inputs.clone(),
        }
    }
}

/// [`Deps`] of `env`.
pub fn deps_of(env: &aver_core::Environment) -> Deps {
    let mut deps = Deps::default();
    for layer in env.all_layers() {
        let path = layer.lockfile.path.as_str();
        let beside = |name: &str| match path.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{name}"),
            None => name.to_owned(),
        };
        let place = match layer.kind {
            aver_core::LayerKind::Npm => Some((&mut deps.keep, "node_modules")),
            aver_core::LayerKind::Uv | aver_core::LayerKind::Pip => Some((&mut deps.keep, ".venv")),
            aver_core::LayerKind::Cargo => Some((&mut deps.seed, "target")),
            aver_core::LayerKind::Go => None,
        };
        if let Some((list, name)) = place {
            let at = beside(name);
            if !list.contains(&at) {
                list.push(at);
            }
        }
        if !deps.inputs.iter().any(|i| i == path) {
            deps.inputs.push(path.to_owned());
        }
    }
    let layers = env.all_layers();
    let has = |kinds: &[aver_core::LayerKind]| layers.iter().any(|l| kinds.contains(&l.kind));
    // Installed dependencies with no layer of their own: `node_modules` from
    // pnpm, yarn, bun or an npm install without a lockfile, `.venv` from a
    // project without uv.lock or requirements. Moved aside, they leave every
    // check failing on "command not found" on both sides.
    for (name, inputs, kinds) in [
        (
            "node_modules",
            NODE_INPUTS,
            &[aver_core::LayerKind::Npm][..],
        ),
        (
            ".venv",
            PYTHON_INPUTS,
            &[aver_core::LayerKind::Uv, aver_core::LayerKind::Pip][..],
        ),
    ] {
        if !deps.keep.iter().any(|k| k == name) {
            deps.keep.push(name.to_owned());
        }
        if !has(kinds) {
            deps.inputs.extend(inputs.iter().map(|i| (*i).to_owned()));
        }
    }
    // `python -m venv venv` is as common as `.venv`; a tracked directory of
    // that name is source, which `Workdir` swaps whatever `keep` says.
    deps.keep.push("venv".to_owned());
    deps
}

/// The files a Python build backend writes from version control when the
/// project is installed, as the root `pyproject.toml` (`text`) names them:
/// setuptools-scm's `version_file` or `write_to`, hatch-vcs's
/// `version-file`. Ignored by Git, they exist only where the install ran,
/// and an editable install imports them from the checkout: moved aside,
/// every test module fails to import on both sides. They hold the
/// version, not the code, so an in-place verification keeps them like
/// `.venv`.
pub fn version_files(text: &str) -> Vec<String> {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let at = |keys: &[&str]| {
        keys.iter()
            .try_fold(&doc, |t, k| t.get(*k)?.as_table())
            .map(|t| t as &toml::Table)
    };
    let mut out: Vec<String> = Vec::new();
    for (table, key) in [
        (&["tool", "setuptools_scm"][..], "version_file"),
        (&["tool", "setuptools_scm"][..], "write_to"),
        (
            &["tool", "hatch", "build", "hooks", "vcs"][..],
            "version-file",
        ),
    ] {
        let Some(file) = at(table).and_then(|t| t.get(key)).and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(path) = file.parse::<Path>() else {
            continue;
        };
        let path = path.as_str().to_owned();
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// What identifies a `node_modules` no npm lockfile describes: any other
/// package manager's lockfile, else the `package.json` files.
pub const NODE_INPUTS: &[&str] = &[
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "package.json",
];

/// What identifies a `.venv` no uv or pip lockfile describes.
pub const PYTHON_INPUTS: &[&str] = &["pyproject.toml", "setup.py", "setup.cfg"];

/// The slot `<slot>.source.json` describes: the directory it was cloned
/// from and that directory's dependency inputs then, with the patterns
/// that state was taken under, so `aver gc` can take it again (empty in a
/// record from before they were kept).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceRecord {
    source: PathBuf,
    deps: DepsState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keep: Vec<String>,
}

impl SourceRecord {
    /// Whether a slot with this record serves `wanted` without a reclone.
    fn serves(&self, wanted: &SourceRecord) -> bool {
        self.source == wanted.source && self.deps == wanted.deps
    }

    /// Whether the source's dependency inputs changed since the clone, so
    /// the next entry reclones: never for a record without its patterns,
    /// a source that is gone, or patterns that no longer parse.
    fn outdated(&self) -> bool {
        if self.inputs.is_empty() || !self.source.is_dir() {
            return false;
        }
        let (Ok(inputs), Ok(keep)) = (
            globs(&self.inputs),
            Keep::new(self.keep.iter().map(String::as_str)),
        ) else {
            return false;
        };
        deps_state(&self.source, &inputs, &keep) != self.deps
    }
}

/// A slot, entered: its directory at the tree asked for, locked until
/// dropped.
pub struct Slot {
    pub dir: PathBuf,
    pub work: Workdir,
    /// Where the slot's runs keep their output (`<slot>.runs`).
    pub runs: PathBuf,
    /// `<slot>`, whose `.pid` record names the run in progress.
    pub record: PathBuf,
    pub relocated: Relocated,
    /// The tree's dependency inputs differ from those the kept
    /// directories were installed from.
    pub deps_differ: bool,
    /// A note for the run's log when a run an earlier process left was
    /// killed on entry.
    pub leftover: Option<String>,
    /// Directories inside the slot a run user never gets
    /// ([`super::proc::Spawn::withheld`]).
    pub withheld: Vec<PathBuf>,
    _lock: FileLock,
}

/// What [`relocate`] changed: text files rewritten, and binary files that
/// hold the source path and could not be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relocated {
    pub rewritten: Vec<String>,
    pub binary: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlaceError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Workdir(#[from] WorkdirError),
    #[error("invalid dependency pattern: {0}")]
    Pattern(String),
}

pub struct Sides {
    root: PathBuf,
    store: Arc<Store>,
    strategy: OnceLock<CloneStrategy>,
}

impl Sides {
    /// The slots of `$AVER_HOME` `home` (`<home>/sides`).
    pub fn new(home: &FsPath, store: Arc<Store>) -> Self {
        Self {
            root: home.join("sides"),
            store,
            strategy: OnceLock::new(),
        }
    }

    /// Slots under `root` instead of a home's (`aver ci --run-as`, whose
    /// run user cannot read the home).
    pub fn at(root: &FsPath, store: Arc<Store>) -> Self {
        Self {
            root: root.to_path_buf(),
            store,
            strategy: OnceLock::new(),
        }
    }

    pub fn root(&self) -> &FsPath {
        &self.root
    }

    /// The dependency state a slot cloned from `source` would record, or
    /// `None` when its patterns do not parse.
    pub fn deps_state_of(&self, source: &SlotSource<'_>) -> Option<DepsState> {
        let keep = keep_of(source).ok()?;
        let inputs = globs(&source.inputs).ok()?;
        Some(deps_state(source.dir, &inputs, &keep))
    }

    /// Forces a clone strategy instead of probing for one.
    pub fn with_strategy(self, strategy: CloneStrategy) -> Self {
        let _ = self.strategy.set(strategy);
        self
    }

    fn strategy(&self) -> CloneStrategy {
        *self.strategy.get_or_init(|| {
            if let Some(s) = std::env::var("AVER_CLONE")
                .ok()
                .and_then(|s| CloneStrategy::parse(&s))
            {
                return s;
            }
            clone::probe(&self.root.join(".probe"), &self.root).unwrap_or(CloneStrategy::Copy)
        })
    }

    /// The directory of the slots of repository directory `repo`.
    pub fn repo_dir(&self, repo: &FsPath) -> PathBuf {
        self.root.join(aver_core::host::local_name(
            repo.as_os_str().as_encoded_bytes(),
        ))
    }

    /// Takes slot `name` of repository `repo` (exclusive until the
    /// [`Slot`] drops), clones it from `source` when it is missing or was
    /// cloned from another directory or other dependency inputs, and
    /// swaps it to `tree`.
    pub fn enter(
        &self,
        repo: &FsPath,
        name: &SlotName,
        source: &SlotSource<'_>,
        tree: TreeId,
    ) -> Result<Slot, PlaceError> {
        self.enter_seeded(repo, name, source, None, tree)
    }

    /// [`Self::enter`], cloning a missing slot from slot `seed` of the same
    /// repository instead of from `source` when [`Self::clone_seed`] can:
    /// the record still names `source`, which the slot then serves.
    pub fn enter_seeded(
        &self,
        repo: &FsPath,
        name: &SlotName,
        source: &SlotSource<'_>,
        seed: Option<&SlotName>,
        tree: TreeId,
    ) -> Result<Slot, PlaceError> {
        let base = self.repo_dir(repo);
        fs::create_dir_all(&base)?;
        let dir = base.join(name.dir());
        let lock = super::proc::lock_in_place(&dir.with_extension("lock"))?;
        let leftover = super::proc::kill_leftover(&dir);
        let keep = keep_of(source)?;
        let inputs = globs(&source.inputs)?;
        let wanted = SourceRecord {
            source: source.dir.to_path_buf(),
            deps: deps_state(source.dir, &inputs, &keep),
            inputs: source.inputs.clone(),
            keep: source.keep.iter().chain(&source.seed).cloned().collect(),
        };
        let record_path = dir.with_extension("source.json");
        let recorded: Option<SourceRecord> = fs::read(&record_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let journal = dir.with_extension("swap.json");
        let mut relocated = Relocated::default();
        if !dir.is_dir() || !recorded.as_ref().is_some_and(|r| r.serves(&wanted)) {
            let _ = fs::remove_file(&record_path);
            let _ = fs::remove_file(&journal);
            if fs::symlink_metadata(&dir).is_ok() {
                super::proc::remove_all(&dir)?;
            }
            let tmp = base.join(format!("{}.tmp-{}", name.dir(), ulid::Ulid::generate()));
            let seeded = match seed {
                Some(seed) => self.clone_seed(&base.join(seed.dir()), &wanted, &tmp)?,
                None => None,
            };
            if seeded.is_none() {
                self.clone_source(source, &tmp)?;
            }
            fs::rename(&tmp, &dir)?;
            crate::fault::point("slot.cloned")?;
            let from = seeded.as_deref().unwrap_or(source.dir);
            relocated = relocate(&dir, from, &keep)?;
            crate::atomic::write(
                &record_path,
                &serde_json::to_vec(&wanted).map_err(io::Error::other)?,
                crate::atomic::Mode::Keep,
            )?;
        }
        // `aver gc` takes a slot for idle by its modification time.
        if let Ok(f) = fs::File::open(&dir) {
            let _ = f.set_modified(SystemTime::now());
        }
        let mut work = Workdir::open(&dir, &journal, keep_of(source)?)?;
        work.become_tree(&self.store, tree)?;
        let deps_differ = deps_state(&dir, &inputs, &keep_of(source)?) != wanted.deps;
        Ok(Slot {
            runs: dir.with_extension("runs"),
            record: dir.clone(),
            dir,
            work,
            relocated,
            deps_differ,
            leftover,
            withheld: Vec::new(),
            _lock: lock,
        })
    }

    /// The checkout at `dir` that [`Workdir::adopt`] took over, as a slot
    /// (`aver ci` in place): locked through `.git/aver-swap.lock` until
    /// dropped, and swapped to `tree`. `deps` is what the adopt kept, and
    /// `state` the checkout's dependency state then. Its runs and pid record
    /// live under this root's `<repo16>/in-place`.
    pub fn enter_adopted(
        &self,
        dir: &FsPath,
        deps: &Deps,
        state: &DepsState,
        tree: TreeId,
        scrub_seed: bool,
    ) -> Result<Slot, PlaceError> {
        let repo = self.repo_dir(dir);
        fs::create_dir_all(&repo)?;
        let record = repo.join("in-place");
        let lock = super::proc::lock_in_place(&record.with_extension("lock"))?;
        let leftover = super::proc::kill_leftover(&record);
        let mut work = Workdir::open_adopted(dir, deps.keep_all()?)?;
        work.become_tree(&self.store, tree)?;
        if scrub_seed {
            // Only the build caches go: under `--run-as` the run user
            // cannot write the kept directories (`withheld` below), so head
            // code cannot have changed them. Without `--run-as` it could,
            // and they stay: the head then runs as the user whose CI job
            // installed them, the trust that job already gives it.
            for seed in &deps.seed {
                let rel = seed.trim_end_matches('/');
                if !work.tracks(rel) {
                    super::proc::remove_all(&dir.join(rel))?;
                }
            }
        }
        let deps_differ = deps_state(dir, &globs(&deps.inputs)?, &deps.keep_all()?) != *state;
        // Installed dependencies stay the verifier's, read-only to a run
        // user, as they are when a slot links them.
        let mut withheld = vec![dir.join(".git")];
        withheld.extend(
            deps.keep
                .iter()
                .map(|k| dir.join(k.trim_end_matches('/')))
                .filter(|p| p.is_dir()),
        );
        Ok(Slot {
            runs: record.with_extension("runs"),
            record,
            dir: dir.to_path_buf(),
            work,
            relocated: Relocated::default(),
            deps_differ,
            leftover,
            withheld,
            _lock: lock,
        })
    }

    /// Clones slot `seed` to `to` when that saves disk and changes nothing
    /// a run sees after the swap: the clone strategy shares blocks, no run
    /// holds the slot or left a process behind in it, and its record says
    /// it was cloned with the dependency state, inputs and kept patterns
    /// `wanted` asks for. `seed` when it did, `None` (nothing written)
    /// otherwise.
    fn clone_seed(
        &self,
        seed: &FsPath,
        wanted: &SourceRecord,
        to: &FsPath,
    ) -> Result<Option<PathBuf>, PlaceError> {
        let strategy = self.strategy();
        if strategy == CloneStrategy::Copy || !seed.is_dir() {
            return Ok(None);
        }
        let Ok(Some(_lock)) = FileLock::try_exclusive(&seed.with_extension("lock")) else {
            return Ok(None);
        };
        if fs::symlink_metadata(seed.with_extension("pid")).is_ok() {
            return Ok(None);
        }
        let recorded: Option<SourceRecord> = fs::read(seed.with_extension("source.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let same = recorded.is_some_and(|r| {
            r.deps == wanted.deps && r.inputs == wanted.inputs && r.keep == wanted.keep
        });
        if !same {
            return Ok(None);
        }
        if clone::clone_tree(strategy, seed, to).is_err() {
            super::proc::remove_all(to)?;
            return Ok(None);
        }
        Ok(Some(seed.to_path_buf()))
    }

    /// Clones `source.dir` to `to`: everything but `.git`; under a plain
    /// copy, kept directories are linked rather than copied and build
    /// caches start empty, since copying `node_modules` and `target` for
    /// every slot costs more than a cold build.
    fn clone_source(&self, source: &SlotSource<'_>, to: &FsPath) -> Result<(), PlaceError> {
        let strategy = self.strategy();
        if strategy != CloneStrategy::Copy {
            clone::clone_tree(strategy, source.dir, to)?;
            let git = to.join(".git");
            if fs::symlink_metadata(&git).is_ok() {
                super::proc::remove_all(&git)?;
            }
            return Ok(());
        }
        let keep = globs(&source.keep)?;
        let seed = globs(&source.seed)?;
        fs::create_dir_all(to)?;
        copy_walk(source.dir, to, "", &keep, &seed)?;
        Ok(())
    }

    /// Removes the `J-` and `T-` slots of workspace `ws` in every
    /// repository (its release): how many.
    pub fn remove_for(&self, ws: &WorkspaceId) -> io::Result<u32> {
        let names = [SlotName::Judged(*ws).dir(), SlotName::Transplant(*ws).dir()];
        let mut removed = 0;
        for repo in fs::read_dir(&self.root).into_iter().flatten().flatten() {
            for name in &names {
                if remove_slot(&repo.path().join(name))? {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    /// What `aver gc` may delete: base slots of no tree in `live_bases`
    /// untouched for `idle`, base slots whose source's dependency inputs
    /// changed since the clone (the next entry reclones them anyway), slots
    /// of workspaces not in `live_ws`, and clones a crash left (`*.tmp-*`).
    /// Slots a run holds are left out.
    pub fn stale(
        &self,
        live_bases: &BTreeSet<TreeId>,
        live_ws: &BTreeSet<WorkspaceId>,
        idle: Duration,
    ) -> Vec<PathBuf> {
        let live_b: BTreeSet<String> = live_bases
            .iter()
            .map(|t| SlotName::Base(*t).dir())
            .collect();
        let live_w: BTreeSet<String> = live_ws
            .iter()
            .flat_map(|w| [SlotName::Judged(*w).dir(), SlotName::Transplant(*w).dir()])
            .collect();
        let mut out = Vec::new();
        for repo in fs::read_dir(&self.root).into_iter().flatten().flatten() {
            for e in fs::read_dir(repo.path()).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let path = e.path();
                if name.contains(".tmp-") {
                    out.push(path);
                    continue;
                }
                if !e.file_type().is_ok_and(|t| t.is_dir()) || name.ends_with(".runs") {
                    continue;
                }
                let stale = if name.starts_with("B-") {
                    let idle_out = !live_b.contains(&name)
                        && fs::metadata(&path)
                            .and_then(|m| m.modified())
                            .is_ok_and(|t| t.elapsed().unwrap_or_default() > idle);
                    idle_out
                        || fs::read(path.with_extension("source.json"))
                            .ok()
                            .and_then(|b| serde_json::from_slice::<SourceRecord>(&b).ok())
                            .is_some_and(|r| r.outdated())
                } else if name.starts_with("J-") || name.starts_with("T-") {
                    !live_w.contains(&name)
                } else {
                    fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t.elapsed().unwrap_or_default() > idle)
                };
                if stale
                    && FileLock::try_exclusive(&path.with_extension("lock"))
                        .is_ok_and(|l| l.is_some())
                {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// One of the host's run slots (`sides/run-slots/<n>.lock`, half the
    /// cores): the first free one, else the one this process's id picks,
    /// waited for. Bounds how many runs go at once across processes.
    pub fn run_slot(&self) -> io::Result<RunSlot> {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        let k = (cores / 2).max(1);
        let dir = self.root.join("run-slots");
        fs::create_dir_all(&dir)?;
        for i in 0..k {
            let path = dir.join(format!("{i}.lock"));
            if !path.exists() {
                fs::write(&path, b"")?;
            }
            if let Some(lock) = FileLock::try_exclusive(&path)? {
                return Ok(RunSlot(lock));
            }
        }
        let i = std::process::id() as usize % k;
        Ok(RunSlot(FileLock::exclusive(
            &dir.join(format!("{i}.lock")),
        )?))
    }
}

/// A host run slot, held until dropped.
pub struct RunSlot(#[allow(dead_code)] FileLock);

/// Removes slot `dir` with what lies beside it (journal, source record,
/// pid record, runs), unless a run holds its lock: whether the slot was
/// there and went. A crashed clone (`*.tmp-*`) has no lock and just goes.
pub fn remove_slot(dir: &FsPath) -> io::Result<bool> {
    if dir
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains(".tmp-"))
    {
        super::proc::remove_all(dir)?;
        return Ok(true);
    }
    let Ok(Some(_lock)) = FileLock::try_exclusive(&dir.with_extension("lock")) else {
        return Ok(false);
    };
    let there = fs::symlink_metadata(dir).is_ok();
    if there {
        super::proc::remove_all(dir)?;
    }
    for ext in ["swap.json", "source.json", "pid", "runs"] {
        let side = dir.with_extension(ext);
        if fs::symlink_metadata(&side).is_ok() {
            super::proc::remove_all(&side)?;
        }
    }
    Ok(there)
}

/// Copies of the change's files in the kept directories of `source` (a
/// non-editable install's `site-packages`): every slot cloned from it
/// shares them, so its runs would test the copy, not the tree
/// (`StaleCopy`).
pub fn stale_copies(
    store: &Store,
    source: &SlotSource<'_>,
    changed: &[Changed],
) -> Result<Vec<StaleCopy>, PlaceError> {
    if source.keep.is_empty() || changed.is_empty() || !source.dir.is_dir() {
        return Ok(Vec::new());
    }
    // Build caches (`seed`) hold no copies of sources and can be large.
    let keep = Keep::new(source.keep.iter().map(String::as_str)).map_err(PlaceError::Pattern)?;
    // Only read: never swapped, so it has no journal.
    let work = Workdir::open(source.dir, FsPath::new(""), keep)?;
    Ok(work.probe_copies(store, changed)?)
}

fn globs(patterns: &[String]) -> Result<Vec<Glob>, PlaceError> {
    patterns
        .iter()
        .map(|p| Glob::new(p.trim_end_matches('/')).map_err(|e| PlaceError::Pattern(e.to_string())))
        .collect()
}

fn keep_of(source: &SlotSource<'_>) -> Result<Keep, PlaceError> {
    Keep::new(source.keep.iter().chain(&source.seed).map(String::as_str))
        .map_err(PlaceError::Pattern)
}

/// The dependency state of `dir`: its files matching `inputs` (outside
/// kept directories and `.git`) and their content.
pub fn deps_state(dir: &FsPath, inputs: &[Glob], keep: &Keep) -> DepsState {
    let mut rows: Vec<(Path, BlobId)> = Vec::new();
    if !inputs.is_empty() {
        walk_inputs(dir, "", inputs, keep, &mut rows);
    }
    DepsState::of(rows.iter().map(|(p, b)| (p, b)))
}

fn walk_inputs(
    dir: &FsPath,
    rel: &str,
    inputs: &[Glob],
    keep: &Keep,
    out: &mut Vec<(Path, BlobId)>,
) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if name == ".git" || keep.names(&path) {
            continue;
        }
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            walk_inputs(&e.path(), &path, inputs, keep, out);
        } else if t.is_file() && inputs.iter().any(|g| g.matches(&path)) {
            if let (Ok(bytes), Ok(p)) = (fs::read(e.path()), path.parse::<Path>()) {
                out.push((p, BlobId::of_bytes(&bytes)));
            }
        }
    }
}

fn copy_walk(
    from: &FsPath,
    to: &FsPath,
    rel: &str,
    keep: &[Glob],
    seed: &[Glob],
) -> io::Result<()> {
    for e in fs::read_dir(from)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if path == ".git" || seed.iter().any(|g| g.matches(&path)) {
            continue;
        }
        let dst = to.join(&name);
        if keep.iter().any(|g| g.matches(&path)) {
            std::os::unix::fs::symlink(e.path(), &dst)?;
            continue;
        }
        let meta = fs::symlink_metadata(e.path())?;
        if meta.is_dir() {
            fs::create_dir(&dst)?;
            copy_walk(&e.path(), &dst, &path, keep, seed)?;
        } else {
            clone::clone_entry(CloneStrategy::Copy, &e.path(), &dst)?;
        }
    }
    Ok(())
}

/// Clones the dependencies of directory `from` (the user's checkout, or a
/// workspace directory for its `aver check` copies) into `dir`, a fresh
/// checkout of the tree `files`: each `deps.keep`, then each `deps.seed`
/// path that `from` has, `files` does not track and `dir` lacks, provided
/// the `deps.inputs` beside it hold the same bytes in `from` as in `files`.
/// Kept directories are then relocated to `dir`. Returns the paths cloned.
///
/// After a seed path, every tracked file of `dir` takes the mtime of
/// `from`'s file when the bytes are equal, and the current time when they
/// differ: a build cache's unit built from `from`'s content then reads
/// stale against a different file of `dir`, however old the checkout's
/// copy of it is (the round 12 hazard).
pub fn seed_workspace(
    strategy: CloneStrategy,
    from: &FsPath,
    dir: &FsPath,
    deps: &Deps,
    files: &BaseFiles,
) -> io::Result<Vec<String>> {
    let same_input = |input: &str| {
        let ours = fs::read(from.join(input))
            .ok()
            .map(|b| BlobId::of_bytes(&b));
        ours == files.files.get(input).map(|f| f.blob)
    };
    fn parent(p: &str) -> &str {
        p.rsplit_once('/').map_or("", |(d, _)| d)
    }
    let mut cloned = Vec::new();
    let mut seeded = false;
    for (rel, seed) in deps
        .keep
        .iter()
        .map(|p| (p, false))
        .chain(deps.seed.iter().map(|p| (p, true)))
    {
        let rel = rel.trim_end_matches('/');
        let (src, dst) = (from.join(rel), dir.join(rel));
        if files.files.contains_key(rel)
            || files.dirs.contains(rel)
            || fs::symlink_metadata(&src).is_err()
            || fs::symlink_metadata(&dst).is_ok()
            || !deps
                .inputs
                .iter()
                .filter(|i| parent(i) == parent(rel))
                .all(|i| same_input(i))
        {
            continue;
        }
        if let Some(up) = dst.parent() {
            fs::create_dir_all(up)?;
        }
        clone::clone_entry(strategy, &src, &dst)?;
        seeded |= seed;
        cloned.push(rel.to_owned());
    }
    let kept: Vec<&str> = cloned
        .iter()
        .filter(|c| {
            deps.keep
                .iter()
                .any(|k| k.trim_end_matches('/') == c.as_str())
        })
        .map(String::as_str)
        .collect();
    if !kept.is_empty() {
        let keep = Keep::new(kept).map_err(io::Error::other)?;
        relocate(dir, from, &keep)?;
    }
    if seeded {
        let now = SystemTime::now();
        files.files.par_iter().try_for_each(|(path, entry)| {
            if entry.kind == FileKind::Symlink {
                return Ok(());
            }
            let Ok(file) = fs::File::open(dir.join(path)) else {
                return Ok(());
            };
            let theirs = from.join(path);
            let when = match fs::read(&theirs) {
                Ok(bytes) if BlobId::of_bytes(&bytes) == entry.blob => {
                    fs::metadata(&theirs)?.modified()?
                }
                _ => now,
            };
            file.set_modified(when)
        })?;
    }
    Ok(cloned)
}

/// Text files inside kept directories that name the source directory by
/// its absolute path, and are read at run time to find code. A `.py`
/// module at the top of `site-packages` is an editable install's finder
/// (setuptools' `__editable___*_finder.py`, scikit-build-core's and
/// meson-python's loaders), whose mapping names the checkout.
fn relocatable(rel: &str) -> bool {
    let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    name.ends_with(".pth")
        || (name.ends_with(".py") && dir.ends_with("site-packages"))
        || name.ends_with(".egg-link")
        || name == "direct_url.json"
        || name == ".modules.yaml"
        || rel.contains(".venv/bin/")
        || rel.starts_with("bin/")
        || rel.contains("/bin/")
}

/// Points what was cloned from `from` into `slot` at the slot: in kept
/// directories, text files that record `from` (`*.pth`, `direct_url.json`,
/// `.modules.yaml`, `*.egg-link`, scripts under `bin/` whose `#!` names
/// it) get the slot path, and absolute symlinks into `from` are
/// re-pointed. Only the slot's copy changes. A binary in such a place
/// holding the path is listed, not changed.
pub fn relocate(slot: &FsPath, from: &FsPath, keep: &Keep) -> io::Result<Relocated> {
    let mut out = Relocated::default();
    let needle = from.as_os_str().as_encoded_bytes().to_vec();
    let with = slot.as_os_str().as_encoded_bytes().to_vec();
    if needle.is_empty() {
        return Ok(out);
    }
    relocate_walk(slot, "", false, &needle, &with, keep, &mut out)?;
    out.rewritten.sort();
    out.binary.sort();
    Ok(out)
}

fn relocate_walk(
    dir: &FsPath,
    rel: &str,
    inside: bool,
    needle: &[u8],
    with: &[u8],
    keep: &Keep,
    out: &mut Relocated,
) -> io::Result<()> {
    for e in fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if name == ".git" {
            continue;
        }
        let here = inside || keep.names(&path);
        let meta = fs::symlink_metadata(e.path())?;
        if meta.file_type().is_symlink() {
            if !here {
                continue;
            }
            let target = fs::read_link(e.path())?;
            let bytes = target.as_os_str().as_encoded_bytes();
            if target.is_absolute() && bytes.starts_with(needle) {
                let rest = &bytes[needle.len()..];
                let mut new = with.to_vec();
                new.extend_from_slice(rest);
                fs::remove_file(e.path())?;
                std::os::unix::fs::symlink(
                    <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&new),
                    e.path(),
                )?;
                out.rewritten.push(path);
            }
        } else if meta.is_dir() {
            relocate_walk(&e.path(), &path, here, needle, with, keep, out)?;
        } else if here && meta.is_file() && meta.len() <= 8 << 20 {
            let candidate = relocatable(&path);
            let native = [".so", ".dylib", ".node"].iter().any(|x| name.ends_with(x));
            if !candidate && !native {
                continue;
            }
            let bytes = fs::read(e.path())?;
            if find(&bytes, needle).is_none() {
                continue;
            }
            let text = !bytes.contains(&0);
            // Under `bin/`, only scripts: their `#!` line runs the source's
            // interpreter otherwise.
            let in_bin = path.starts_with("bin/") || path.contains("/bin/");
            let script = !in_bin || bytes.starts_with(b"#!");
            if text && candidate && script {
                let replaced = replace_all(&bytes, needle, with);
                let mode = meta.mode();
                // A new file, so the source's shared blocks are not written.
                fs::remove_file(e.path())?;
                fs::write(e.path(), &replaced)?;
                fs::set_permissions(e.path(), std::os::unix::fs::PermissionsExt::from_mode(mode))?;
                out.rewritten.push(path);
            } else {
                out.binary.push(path);
            }
        }
    }
    Ok(())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn replace_all(hay: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(needle) {
            out.extend_from_slice(with);
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::version_files;

    #[test]
    fn version_files_come_from_setuptools_scm_and_hatch_vcs() {
        let text = "[tool.setuptools_scm]\nwrite_to = \"src/pkg/_version.py\"\nversion_file = \"src/pkg/_version.py\"\n\n[tool.hatch.build.hooks.vcs]\nversion-file = \"src/_black_version.py\"\n";
        assert_eq!(
            version_files(text),
            ["src/pkg/_version.py", "src/_black_version.py"]
        );
        assert!(version_files("[tool.setuptools_scm]\nwrite_to = \"../outside.py\"\n").is_empty());
        assert!(version_files("[tool.hatch.version]\nsource = \"vcs\"\n").is_empty());
        assert!(version_files("not toml [").is_empty());
    }
}
