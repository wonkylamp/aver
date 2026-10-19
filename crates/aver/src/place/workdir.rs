//! One directory swapped between trees: a slot's
//! checkout, or a CI job's own checkout adopted for the length of a
//! verification and restored after.
//!
//! [`Workdir::become_tree`] makes the directory exactly a tree's tracked
//! files plus what `keep` names (installed dependencies, build caches),
//! rewriting only what differs. A file kept from before keeps its mtime;
//! every file written gets a fresh one, so a build tool that compares
//! mtimes (Cargo, Python's `.pyc`) never takes an output built from other
//! content as fresh (invariant I1). A journal beside the directory says
//! what each tracked path holds and which swap was in progress, so a swap
//! cut short is finished by the next one and never trusted.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path as FsPath, PathBuf};

use aver_core::{BlobId, Node, Tree, TreeId};
use serde::{Deserialize, Serialize};

use aver_core::GitSha;

use crate::git::GitError;
use crate::store::{Store, StoreError};
use crate::workspace::{FileEntry, FileKind, Glob};

/// Every file of tree `root`, by path, in path order.
pub fn tree_files(store: &Store, root: &TreeId) -> Result<Vec<(String, FileEntry)>, StoreError> {
    let mut out = Vec::new();
    let mut stack = vec![(String::new(), *root)];
    while let Some((prefix, id)) = stack.pop() {
        let tree: Tree = store.get(&id)?;
        for entry in &tree.entries {
            let path = format!("{prefix}{}", entry.name);
            match entry.node {
                Node::Tree(sub) => stack.push((format!("{path}/"), sub)),
                node => {
                    if let Some(file) = FileEntry::of_node(&node) {
                        out.push((path, file));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum WorkdirError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Git(#[from] GitError),
    /// The checkout's tracked files are not the commit's: adopting it
    /// would verify something else, and restoring would lose the edits.
    #[error("the checkout has changes to tracked files: {}", .0.join(", "))]
    Dirty(Vec<String>),
    #[error("no adopted checkout to restore at {0}")]
    NotAdopted(PathBuf),
    /// An earlier adopt was never restored: restore it first.
    #[error("{0} is still adopted by an earlier run; restore it first")]
    Adopted(PathBuf),
}

/// What [`Workdir::become_tree`] leaves alone besides the tree: paths a
/// glob names, and everything under them.
#[derive(Default)]
pub struct Keep {
    globs: Vec<Glob>,
    patterns: Vec<String>,
}

impl Keep {
    /// `.aver` and every pattern in `patterns` (`node_modules`, `target`,
    /// `crates/*/target`): a pattern without `/` names a file or directory
    /// at any depth.
    pub fn new<'a>(patterns: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let patterns: Vec<String> = patterns.into_iter().map(str::to_owned).collect();
        let globs = std::iter::once(".aver")
            .chain(patterns.iter().map(String::as_str))
            .map(|p| Glob::new(p.trim_end_matches('/')).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        Ok(Self { globs, patterns })
    }

    /// The patterns given to [`Keep::new`].
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Whether the path itself is kept (its ancestors are not checked).
    pub fn names(&self, path: &str) -> bool {
        self.globs.iter().any(|g| g.matches(path))
    }
}

/// What the journal records per tracked path Aver wrote or checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Written {
    blob: BlobId,
    kind: String,
    size: u64,
    mtime_ns: i128,
    ino: u64,
}

impl Written {
    fn of(entry: &FileEntry, meta: &fs::Metadata) -> Self {
        Self {
            blob: entry.blob,
            kind: entry.kind.as_str().to_owned(),
            size: meta.len(),
            mtime_ns: mtime_ns(meta),
            ino: meta.ino(),
        }
    }

    /// Whether the file on disk is still what was recorded.
    fn holds(&self, entry: &FileEntry, meta: &fs::Metadata) -> bool {
        self.blob == entry.blob
            && self.kind == entry.kind.as_str()
            && meta.len() == self.size
            && mtime_ns(meta) == self.mtime_ns
            && meta.ino() == self.ino
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Journal {
    /// The adopted checkout's commit and tree, which `restore` puts back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adopted: Option<Adopted>,
    /// The tree the directory holds; `None`: not known, so the next swap
    /// checks every tracked path's content.
    at: Option<TreeId>,
    /// Set before a swap's first write and cleared after its last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    swapping_to: Option<TreeId>,
    files: BTreeMap<String, Written>,
    /// Every path a swap wrote or deleted since the adopt, recorded before
    /// the swap's first write.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    touched: BTreeSet<String>,
    /// An adopted checkout's `keep` patterns, which `restore` leaves alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keep: Vec<String>,
    /// The untracked paths the adopt moved under [`STASH`], each put back
    /// by `restore`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    stashed: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Adopted {
    commit: GitSha,
    tree: TreeId,
}

/// One swap's work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Swap {
    pub written: u32,
    pub removed: u32,
}

/// A copy of a changed tracked file left in a kept directory (a
/// non-editable install's `site-packages`): the run would test it, not the
/// tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleCopy {
    pub keep: String,
    pub tracked: String,
}

/// A tracked path a change touches, with its content on each side.
#[derive(Debug, Clone)]
pub struct Changed {
    pub path: String,
    pub base: Option<BlobId>,
    pub head: Option<BlobId>,
}

/// What [`Workdir::restore`] put back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Restored {
    pub written: u32,
    pub removed: u32,
    /// Untracked paths the adopt had moved aside, put back.
    pub returned: u32,
}

pub struct Workdir {
    root: PathBuf,
    keep: Keep,
    journal_path: PathBuf,
    state: Journal,
    /// Paths left alone besides `keep` (a restore's stashed paths).
    also_kept: BTreeSet<String>,
}

/// Where an adopted checkout's untracked files wait for the restore, under
/// `.git` so no swap or run sees them.
const STASH: &str = "aver-stash";

/// The journal of an adopted checkout.
pub fn ci_journal(root: &FsPath) -> PathBuf {
    root.join(".git").join("aver-swap.json")
}

fn stash_dir(root: &FsPath) -> PathBuf {
    root.join(".git").join(STASH)
}

impl Workdir {
    /// The directory `root` whose journal is `journal` (a slot): created if
    /// missing. A journal left mid-swap is trusted for no path the swap
    /// could have touched.
    pub fn open(root: &FsPath, journal: &FsPath, keep: Keep) -> Result<Self, WorkdirError> {
        fs::create_dir_all(root)?;
        let mut state: Journal = fs::read(journal)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if state.swapping_to.take().is_some() {
            // Which paths the cut swap rewrote is not known; checking every
            // path's content once is.
            state.at = None;
            state.files.clear();
        }
        Ok(Self {
            root: root.to_path_buf(),
            keep,
            journal_path: journal.to_path_buf(),
            state,
            also_kept: BTreeSet::new(),
        })
    }

    /// The checkout at `root` a [`Workdir::adopt`] took over, as its
    /// journal left it: for each run of an in-place verification.
    pub fn open_adopted(root: &FsPath, keep: Keep) -> Result<Self, WorkdirError> {
        let journal = ci_journal(root);
        if !journal.is_file() {
            return Err(WorkdirError::NotAdopted(root.to_path_buf()));
        }
        Self::open(root, &journal, keep)
    }

    /// Takes over the CI checkout at `root` for verification, at its
    /// commit `head` (and the tree Aver imported it as): refuses a checkout
    /// whose tracked files differ from the commit, then moves untracked
    /// files outside `keep` and `.git` aside under `.git`, where
    /// [`Workdir::restore`] finds them and puts them back with the commit.
    pub fn adopt(
        root: &FsPath,
        store: &Store,
        head: (GitSha, TreeId),
        keep: Keep,
    ) -> Result<Self, WorkdirError> {
        if ci_journal(root).exists() {
            return Err(WorkdirError::Adopted(root.to_path_buf()));
        }
        let files = tree_files(store, &head.1)?;
        let mut dirty = Vec::new();
        let mut checked = BTreeMap::new();
        for (path, entry) in &files {
            let at = root.join(path);
            match on_disk(&at, entry)? {
                Some(meta) => {
                    checked.insert(path.clone(), Written::of(entry, &meta));
                }
                None => dirty.push(path.clone()),
            }
        }
        if !dirty.is_empty() {
            return Err(WorkdirError::Dirty(dirty));
        }
        // With no journal, nothing under the stash is waiting to go back.
        if fs::symlink_metadata(stash_dir(root)).is_ok() {
            super::proc::remove_all(&stash_dir(root))?;
        }
        let mut work = Self {
            root: root.to_path_buf(),
            journal_path: ci_journal(root),
            state: Journal {
                adopted: Some(Adopted {
                    commit: head.0,
                    tree: head.1,
                }),
                at: Some(head.1),
                swapping_to: None,
                files: checked,
                touched: BTreeSet::new(),
                keep: keep.patterns().to_vec(),
                stashed: BTreeSet::new(),
            },
            keep,
            also_kept: BTreeSet::new(),
        };
        let wanted: HashSet<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        let mut extras = Vec::new();
        work.extras(
            &work.root,
            "",
            &wanted,
            &parents(files.iter().map(|(p, _)| p.as_str())),
            &mut extras,
        )?;
        // Journaled before the first move, so a restore after a cut finds
        // every path moved.
        work.state.stashed = extras.iter().map(|(p, _)| p.clone()).collect();
        work.save()?;
        let stash = stash_dir(root);
        for (rel, _) in &extras {
            let to = stash.join(rel);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(root.join(rel), to)?;
        }
        Ok(work)
    }

    /// Whether the tree the checkout holds tracks `rel` or a path under it.
    pub fn tracks(&self, rel: &str) -> bool {
        let prefix = format!("{rel}/");
        self.state
            .files
            .keys()
            .any(|p| p == rel || p.starts_with(&prefix))
    }

    pub fn root(&self) -> &FsPath {
        &self.root
    }

    /// The tree the directory holds, if known.
    pub fn at(&self) -> Option<TreeId> {
        self.state.at
    }

    /// Makes the directory exactly `tree`'s tracked files plus what `keep`
    /// names: a file the journal shows unchanged (or, with no trusted
    /// journal, whose content already matches) keeps its mtime; every
    /// other is written afresh; everything else is deleted.
    pub fn become_tree(&mut self, store: &Store, tree: TreeId) -> Result<Swap, WorkdirError> {
        let files = tree_files(store, &tree)?;
        let wanted: HashSet<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        let mut extras = Vec::new();
        self.extras(
            &self.root,
            "",
            &wanted,
            &parents(files.iter().map(|(p, _)| p.as_str())),
            &mut extras,
        )?;
        let trusted = self.state.at.is_some();
        let mut written = BTreeMap::new();
        let mut writes = Vec::new();
        for (path, entry) in &files {
            let target = self.root.join(path);
            let meta = fs::symlink_metadata(&target).ok();
            let kept = match (&meta, self.state.files.get(path)) {
                (Some(meta), Some(w)) if trusted => w.holds(entry, meta).then(|| w.clone()),
                (Some(_), _) if !trusted => {
                    on_disk(&target, entry)?.map(|m| Written::of(entry, &m))
                }
                _ => None,
            };
            match kept {
                Some(w) => {
                    written.insert(path.clone(), w);
                }
                None => writes.push((path, entry, target, meta)),
            }
        }
        // Every tracked path the swap will write or delete is journaled
        // before the first changes, so a `restore` after a cut puts each back.
        for (rel, _) in &extras {
            let prefix = format!("{rel}/");
            self.state.touched.extend(
                self.state
                    .files
                    .keys()
                    .filter(|p| *p == rel || p.starts_with(&prefix))
                    .cloned(),
            );
        }
        self.state
            .touched
            .extend(writes.iter().map(|w| w.0.clone()));
        self.state.swapping_to = Some(tree);
        self.save()?;
        crate::fault::point("swap.journaled")?;
        remove(&self.root, &extras)?;
        let mut swap = Swap {
            written: 0,
            removed: extras.len() as u32,
        };
        for (path, entry, target, meta) in writes {
            if let Some(meta) = meta {
                if meta.is_dir() {
                    super::proc::remove_all(&target)?;
                } else {
                    fs::remove_file(&target)?;
                }
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let bytes = store.get_raw(&entry.blob)?;
            write_entry(&target, entry.kind, &bytes)?;
            let meta = fs::symlink_metadata(&target)?;
            written.insert(path.clone(), Written::of(entry, &meta));
            swap.written += 1;
            crate::fault::point("swap.partial")?;
        }
        self.state.at = Some(tree);
        self.state.swapping_to = None;
        self.state.files = written;
        self.save()?;
        crate::fault::point("swap.done")?;
        Ok(swap)
    }

    /// The files a run left in the directory that the tree lacks, outside
    /// `keep`, matching `globs` (built test files), each as the hash of its
    /// content, in path order. Nothing is stored: a side only compares.
    pub fn derived(&self, globs: &[Glob]) -> Result<Vec<(String, BlobId)>, WorkdirError> {
        let mut out = Vec::new();
        if globs.is_empty() {
            return Ok(out);
        }
        self.walk_untracked(&self.root, "", &mut |rel, path| {
            if globs.iter().any(|g| g.matches(rel)) {
                out.push((rel.to_owned(), BlobId::of_bytes(&fs::read(path)?)));
            }
            Ok(())
        })?;
        out.sort();
        Ok(out)
    }

    /// Copies of changed tracked files inside kept directories, where an
    /// install puts the repository's own package: `site-packages/<p>` for a
    /// changed path ending in `<p>` (two or more components), and
    /// `node_modules/<name>/<p>` for a changed path `<dir>/<p>` whose
    /// `<dir>/package.json` is named `<name>`, unless the install record
    /// says it came from a registry (a published version some dependency
    /// pulled in, which nothing in the repository imports). The bytes must
    /// be the path's base or head content. Files over 1 MiB and tool
    /// caches (`node_modules/.cache`) are skipped.
    pub fn probe_copies(
        &self,
        store: &Store,
        changed: &[Changed],
    ) -> Result<Vec<StaleCopy>, WorkdirError> {
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        self.walk_kept(&self.root, "", false, &mut |rel| {
            if let Some(name) = rel.rsplit('/').next() {
                by_name
                    .entry(name.to_owned())
                    .or_default()
                    .push(rel.to_owned());
            }
        })?;
        let mut out = Vec::new();
        for c in changed {
            let parts: Vec<&str> = c.path.split('/').collect();
            if parts.len() < 2 {
                continue;
            }
            let tail = parts[parts.len() - 2..].join("/");
            let Some(candidates) = by_name.get(*parts.last().expect("two parts")) else {
                continue;
            };
            let blobs: Vec<Vec<u8>> = [c.base, c.head]
                .into_iter()
                .flatten()
                .map(|b| store.get_raw(&b))
                .collect::<Result<_, _>>()?;
            let own = own_install_paths(&self.root, &c.path);
            for kept in candidates {
                if !(kept.ends_with(&format!("/{tail}")) || *kept == tail) {
                    continue;
                }
                let python = kept
                    .split_once("site-packages/")
                    .is_some_and(|(_, p)| c.path.ends_with(&format!("/{p}")));
                let node = own.iter().any(|p| {
                    kept.strip_suffix(p.as_str())
                        .is_some_and(|at| !registry_install(&self.root, at, p))
                });
                if !python && !node {
                    continue;
                }
                let at = self.root.join(kept);
                let Ok(meta) = fs::metadata(&at) else {
                    continue;
                };
                if meta.len() > 1 << 20 || !blobs.iter().any(|b| b.len() as u64 == meta.len()) {
                    continue;
                }
                let bytes = fs::read(&at)?;
                if blobs.contains(&bytes) {
                    out.push(StaleCopy {
                        keep: kept.clone(),
                        tracked: c.path.clone(),
                    });
                }
            }
        }
        out.sort_by(|a, b| (&a.tracked, &a.keep).cmp(&(&b.tracked, &b.keep)));
        Ok(out)
    }

    /// Puts the adopted checkout at `root` back as it was adopted, from
    /// Git and the journal alone (the Aver home may be gone): every
    /// tracked path a swap touched is rewritten from the commit, or deleted
    /// where the commit lacks it; what runs left outside `keep` goes; the
    /// untracked files the adopt moved aside come back. Then the journal
    /// goes. Run again after a cut, it finishes the job.
    pub fn restore(root: &FsPath) -> Result<Restored, WorkdirError> {
        let path = ci_journal(root);
        let state: Journal = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(|| WorkdirError::NotAdopted(root.to_path_buf()))?;
        let adopted = state
            .adopted
            .clone()
            .ok_or_else(|| WorkdirError::NotAdopted(root.to_path_buf()))?;
        let repo = crate::git::Repo::new(root);
        let listing = crate::git::tracked_files(&repo, &adopted.commit)?;
        let mut done = Restored::default();
        let tracked: HashSet<&str> = listing.iter().map(|f| f.path.as_str()).collect();
        let wanted: Vec<_> = listing
            .iter()
            .filter(|f| state.touched.contains(&f.path))
            .collect();
        let shas: Vec<GitSha> = wanted.iter().map(|f| f.sha.clone()).collect();
        let blobs = crate::git::read_blobs_whole(&repo, &shas)?;
        let mut restored = BTreeSet::new();
        for (file, bytes) in wanted.iter().zip(blobs) {
            let target = root.join(&file.path);
            if let Ok(meta) = fs::symlink_metadata(&target) {
                if meta.is_dir() {
                    super::proc::remove_all(&target)?;
                } else {
                    fs::remove_file(&target)?;
                }
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            write_entry(&target, file.kind, &bytes)?;
            restored.insert(file.path.clone());
            done.written += 1;
            crate::fault::point("workdir.restore.partial")?;
        }
        for path in state.touched.difference(&restored) {
            let target = root.join(path);
            if fs::symlink_metadata(&target).is_ok() {
                fs::remove_file(&target)?;
                done.removed += 1;
                prune_empty_parents(root, &target);
                crate::fault::point("workdir.restore.partial")?;
            }
        }
        let keep = Keep::new(state.keep.iter().map(String::as_str))
            .map_err(|e| WorkdirError::Io(io::Error::other(e)))?;
        // Stashed paths are skipped: a cut restore may have put some back.
        let work = Self {
            root: root.to_path_buf(),
            keep,
            journal_path: path.clone(),
            also_kept: state.stashed.clone(),
            state,
        };
        let mut extras = Vec::new();
        work.extras(
            root,
            "",
            &tracked,
            &parents(tracked.iter().copied()),
            &mut extras,
        )?;
        done.removed += extras.len() as u32;
        remove(root, &extras)?;
        let stash = stash_dir(root);
        for rel in &work.state.stashed {
            let from = stash.join(rel);
            if fs::symlink_metadata(&from).is_err() {
                continue;
            }
            let to = root.join(rel);
            if let Ok(meta) = fs::symlink_metadata(&to) {
                if meta.is_dir() {
                    super::proc::remove_all(&to)?;
                } else {
                    fs::remove_file(&to)?;
                }
            }
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&from, &to)?;
            done.returned += 1;
        }
        crate::fault::point("workdir.restore.done")?;
        if fs::symlink_metadata(&stash).is_ok() {
            super::proc::remove_all(&stash)?;
        }
        fs::remove_file(&path)?;
        Ok(done)
    }

    fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.journal_path.parent() {
            fs::create_dir_all(dir)?;
        }
        crate::atomic::write(
            &self.journal_path,
            &serde_json::to_vec(&self.state).map_err(io::Error::other)?,
            crate::atomic::Mode::Keep,
        )
    }

    /// Whether `rel` stays whatever the tree says: kept, or (in an adopted
    /// checkout) Git's own directory.
    fn kept(&self, rel: &str) -> bool {
        self.keep.names(rel)
            || (self.state.adopted.is_some() && rel == ".git")
            || self.also_kept.contains(rel)
    }

    /// What the tree lacks under `dir` (`rel` in the directory), as paths
    /// in the directory with whether each is a directory to delete whole,
    /// walking the directory itself so anything a run wrote is found too.
    fn extras(
        &self,
        dir: &FsPath,
        rel: &str,
        wanted: &HashSet<&str>,
        dirs: &BTreeSet<String>,
        out: &mut Vec<(String, bool)>,
    ) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = join(rel, &name);
            // A kept name stands for installed dependencies, never for
            // source: a directory either tree tracks (a package named
            // `venv`) swaps like any other.
            let tracked =
                dirs.contains(&path) || wanted.contains(path.as_str()) || self.held_tracks(&path);
            if self.kept(&path) && !tracked {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                if dirs.contains(&path) || self.keeps_below(&path) {
                    self.extras(&entry.path(), &path, wanted, dirs, out)?;
                } else {
                    out.push((path, true));
                }
            } else if !wanted.contains(path.as_str()) {
                out.push((path, false));
            }
        }
        Ok(())
    }

    /// Whether the tree the directory holds tracks `rel` or a file below it.
    fn held_tracks(&self, rel: &str) -> bool {
        let below = format!("{rel}/");
        self.state.files.contains_key(rel)
            || self
                .state
                .files
                .range(below.clone()..)
                .next()
                .is_some_and(|(p, _)| p.starts_with(&below))
    }

    /// Whether a pattern of `keep` may name something below `dir`: then
    /// the directory is walked rather than removed whole.
    fn keeps_below(&self, dir: &str) -> bool {
        self.any_kept_under(&self.root.join(dir), dir)
    }

    fn any_kept_under(&self, at: &FsPath, rel: &str) -> bool {
        fs::read_dir(at).into_iter().flatten().flatten().any(|e| {
            let path = join(rel, &e.file_name().to_string_lossy());
            self.kept(&path)
                || (e.file_type().is_ok_and(|t| t.is_dir())
                    && self.any_kept_under(&e.path(), &path))
        })
    }

    /// Calls `f` with each regular file under `dir` that the current tree
    /// lacks and `keep` does not name.
    fn walk_untracked(
        &self,
        dir: &FsPath,
        rel: &str,
        f: &mut dyn FnMut(&str, &FsPath) -> Result<(), WorkdirError>,
    ) -> Result<(), WorkdirError> {
        for entry in fs::read_dir(dir)?.flatten() {
            let path = join(rel, &entry.file_name().to_string_lossy());
            if self.kept(&path) || path == ".git" {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                self.walk_untracked(&entry.path(), &path, f)?;
            } else if meta.is_file() && !self.state.files.contains_key(&path) {
                f(&path, &entry.path())?;
            }
        }
        Ok(())
    }

    /// Calls `f` with each regular file inside a kept directory.
    fn walk_kept(
        &self,
        dir: &FsPath,
        rel: &str,
        inside: bool,
        f: &mut dyn FnMut(&str),
    ) -> io::Result<()> {
        for entry in fs::read_dir(dir)?.flatten() {
            let path = join(rel, &entry.file_name().to_string_lossy());
            if path == ".git" || path == ".aver" || path.ends_with("node_modules/.cache") {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let here = inside || self.keep.names(&path);
            if meta.is_dir() {
                self.walk_kept(&entry.path(), &path, here, f)?;
            } else if meta.is_file() && here {
                f(&path);
            }
        }
        Ok(())
    }
}

/// `node_modules/<name>/<p>` for each directory above `path` (`<dir>/<p>`)
/// whose `package.json` in `root` is named `<name>`.
fn own_install_paths(root: &FsPath, path: &str) -> Vec<String> {
    let parents = std::iter::once(0).chain(path.match_indices('/').map(|(i, _)| i));
    parents
        .filter_map(|end| {
            let (dir, rel) = match end {
                0 => ("", path),
                i => (&path[..i], &path[i + 1..]),
            };
            let bytes = fs::read(root.join(dir).join("package.json")).ok()?;
            let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            let name = manifest.get("name")?.as_str()?;
            Some(format!("node_modules/{name}/{rel}"))
        })
        .collect()
}

/// Whether the package whose file is at `<at><own>` (`own` is
/// `node_modules/<name>/<p>`) was installed from a registry: it sits in
/// pnpm's store as `.pnpm/<name>@<version>` rather than a `file:` or
/// `link:` copy, or npm's `node_modules/.package-lock.json` at the top
/// records an `http` `resolved` for it.
fn registry_install(root: &FsPath, at: &str, own: &str) -> bool {
    if let Some((_, store)) = at.split_once(".pnpm/") {
        let entry = store.split('/').next().unwrap_or_default();
        return !(entry.contains("@file+") || entry.contains("@link+"));
    }
    let rest = &own["node_modules/".len()..];
    let name = match rest.split_once('/') {
        Some((scope, more)) if scope.starts_with('@') => {
            format!("{scope}/{}", more.split('/').next().unwrap_or_default())
        }
        Some((name, _)) => name.to_owned(),
        None => return false,
    };
    let package = format!("{at}node_modules/{name}");
    let top = package.find("node_modules/").expect("ends in node_modules");
    let record = root
        .join(&package[..top])
        .join("node_modules/.package-lock.json");
    fs::read(record)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| {
            let resolved = v.get("packages")?.get(&package[top..])?.get("resolved")?;
            let resolved = resolved.as_str()?;
            Some(resolved.starts_with("https://") || resolved.starts_with("http://"))
        })
        .unwrap_or(false)
}

fn remove(root: &FsPath, extras: &[(String, bool)]) -> io::Result<()> {
    for (rel, dir) in extras {
        if *dir {
            super::proc::remove_all(&root.join(rel))?;
        } else {
            fs::remove_file(root.join(rel))?;
        }
    }
    Ok(())
}

/// The metadata of the file at `at` if it holds `entry` exactly (content
/// and kind), else `None`.
fn on_disk(at: &FsPath, entry: &FileEntry) -> io::Result<Option<fs::Metadata>> {
    let Ok(meta) = fs::symlink_metadata(at) else {
        return Ok(None);
    };
    let bytes = match entry.kind {
        FileKind::Symlink if meta.file_type().is_symlink() => {
            use std::os::unix::ffi::OsStrExt as _;
            fs::read_link(at)?.as_os_str().as_bytes().to_vec()
        }
        FileKind::File | FileKind::Exec if meta.is_file() => {
            let exec = meta.permissions().mode() & 0o111 != 0;
            if exec != (entry.kind == FileKind::Exec) {
                return Ok(None);
            }
            fs::read(at)?
        }
        _ => return Ok(None),
    };
    Ok((BlobId::of_bytes(&bytes) == entry.blob).then_some(meta))
}

fn write_entry(target: &FsPath, kind: FileKind, bytes: &[u8]) -> io::Result<()> {
    match kind {
        FileKind::Symlink => {
            use std::os::unix::ffi::OsStrExt as _;
            std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(bytes), target)
        }
        kind => {
            fs::write(target, bytes)?;
            let mode = if kind == FileKind::Exec { 0o755 } else { 0o644 };
            fs::set_permissions(target, fs::Permissions::from_mode(mode))
        }
    }
}

/// Every directory holding a file of `paths`.
fn parents<'a>(paths: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    let mut dirs = BTreeSet::new();
    for path in paths {
        let mut prefix = path;
        while let Some(i) = prefix.rfind('/') {
            prefix = &prefix[..i];
            if !dirs.insert(prefix.to_owned()) {
                break;
            }
        }
    }
    dirs
}

fn prune_empty_parents(root: &FsPath, file: &FsPath) {
    let mut at = file.parent();
    while let Some(dir) = at.filter(|d| *d != root && d.starts_with(root)) {
        if fs::remove_dir(dir).is_err() {
            break;
        }
        at = dir.parent();
    }
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_owned()
    } else {
        format!("{rel}/{name}")
    }
}

fn mtime_ns(meta: &fs::Metadata) -> i128 {
    i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec())
}
