//! Scan-on-demand (§2.3.1 Sync): the directory's state as a desired
//! overlay, from a parallel `lstat` walk and a stat cache that lets
//! unchanged files skip hashing.
//!
//! A stat cache entry is trusted only when its `(ino, size, mtime, ctime,
//! kind)` still match and its mtime is older than the filesystem clock at
//! the start of the scan that recorded it (the racy-clean rule of Git and
//! jj). The clock is read from a file the scan touches, so it uses the same
//! timestamp source and granularity as the files it compares against.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use aver_core::{BlobId, Name, Path};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use rayon::prelude::*;

use super::{BaseFiles, DiskError, ScanStats, SkipReason, Skipped};
use crate::workspace::{FileEntry, FileKind};

/// Reads of a file whose stat changes during the read before it is
/// reported unstable.
const READ_ATTEMPTS: usize = 3;
const STAT_MAGIC: &[u8; 8] = b"AVERSTAT";
const STAT_VERSION: u32 = 1;

/// What a cache entry remembers about a path's last scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatEntry {
    ino: u64,
    size: u64,
    mtime_ns: i64,
    ctime_ns: i64,
    kind: FileKind,
    state: Seen,
}

/// The outcome of hashing a path: its blob, or why it was not synced.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Blob(BlobId),
    Skipped(SkipReason),
}

/// Per-path stat records of one scan (§1.8.1 "Stat cache").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatCache {
    /// Filesystem clock when the scan that wrote this cache started.
    recorded_at_ns: i64,
    entries: HashMap<String, StatEntry>,
}

impl StatCache {
    /// Reads a cache file; a missing or unreadable one is an empty cache,
    /// which only costs re-hashing.
    pub(crate) fn load(path: &FsPath) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| decode(&bytes))
            .unwrap_or_default()
    }

    /// Writes the cache atomically (temporary file, then rename).
    pub(crate) fn save(&self, path: &FsPath) -> io::Result<()> {
        let tmp = path.with_extension(format!("tmp-{}", ulid::Ulid::generate()));
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&self.encode())?;
        }
        fs::rename(&tmp, path)
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.entries.len() * 96);
        out.extend_from_slice(STAT_MAGIC);
        out.extend_from_slice(&STAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.recorded_at_ns.to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        let mut paths: Vec<&String> = self.entries.keys().collect();
        paths.sort();
        for path in paths {
            let e = &self.entries[path];
            put_bytes(&mut out, path.as_bytes());
            out.extend_from_slice(&e.ino.to_le_bytes());
            out.extend_from_slice(&e.size.to_le_bytes());
            out.extend_from_slice(&e.mtime_ns.to_le_bytes());
            out.extend_from_slice(&e.ctime_ns.to_le_bytes());
            out.push(match e.kind {
                FileKind::File => 0,
                FileKind::Exec => 1,
                FileKind::Symlink => 2,
            });
            match &e.state {
                Seen::Blob(blob) => {
                    out.push(0);
                    put_bytes(&mut out, blob.to_string().as_bytes());
                }
                Seen::Skipped(reason) => {
                    out.push(1);
                    put_bytes(
                        &mut out,
                        &serde_json::to_vec(reason).expect("skip reasons serialize"),
                    );
                }
            }
        }
        out
    }
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn decode(bytes: &[u8]) -> Option<StatCache> {
    let mut r = Reader(bytes);
    if r.take(8)? != STAT_MAGIC || r.u32()? != STAT_VERSION {
        return None;
    }
    let recorded_at_ns = r.i64()?;
    let count = r.u32()? as usize;
    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        let path = String::from_utf8(r.bytes()?.to_vec()).ok()?;
        let (ino, size, mtime_ns, ctime_ns) = (r.u64()?, r.u64()?, r.i64()?, r.i64()?);
        let kind = match r.take(1)?[0] {
            0 => FileKind::File,
            1 => FileKind::Exec,
            2 => FileKind::Symlink,
            _ => return None,
        };
        let state = match r.take(1)?[0] {
            0 => Seen::Blob(std::str::from_utf8(r.bytes()?).ok()?.parse().ok()?),
            1 => Seen::Skipped(serde_json::from_slice(r.bytes()?).ok()?),
            _ => return None,
        };
        entries.insert(
            path,
            StatEntry {
                ino,
                size,
                mtime_ns,
                ctime_ns,
                kind,
                state,
            },
        );
    }
    Some(StatCache {
        recorded_at_ns,
        entries,
    })
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

/// The filesystem's notion of now: the mtime of a file just written in
/// `dir`, on the same volume and clock as the files a scan compares. The
/// probe has a fresh name and is created exclusively, so it never replaces
/// or follows anything already in `dir`.
pub(crate) fn fs_clock(dir: &FsPath) -> io::Result<i64> {
    let path = dir.join(format!(".aver-clock-{}", ulid::Ulid::generate()));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let probed = file.set_len(1).and_then(|()| file.metadata());
    let _ = fs::remove_file(&path);
    Ok(mtime_ns(&probed?))
}

fn mtime_ns(meta: &fs::Metadata) -> i64 {
    meta.mtime() * 1_000_000_000 + meta.mtime_nsec()
}

fn ctime_ns(meta: &fs::Metadata) -> i64 {
    meta.ctime() * 1_000_000_000 + meta.ctime_nsec()
}

fn kind_of(meta: &fs::Metadata) -> FileKind {
    if meta.file_type().is_symlink() {
        FileKind::Symlink
    } else if meta.mode() & 0o111 != 0 {
        FileKind::Exec
    } else {
        FileKind::File
    }
}

fn same_stat(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.ino() == b.ino()
        && a.len() == b.len()
        && mtime_ns(a) == mtime_ns(b)
        && ctime_ns(a) == ctime_ns(b)
        && kind_of(a) == kind_of(b)
}

/// Builds a stat cache for a freshly cloned directory whose files match
/// `manifest` (path → (blob, size, mtime)) without reading any file.
pub(crate) fn seed(
    dir: &FsPath,
    clock_dir: &FsPath,
    manifest: &[super::checkout::ManifestEntry],
) -> io::Result<StatCache> {
    let recorded_at_ns = fs_clock(clock_dir)?;
    let entries = manifest
        .par_iter()
        .filter_map(|m| {
            let meta = fs::symlink_metadata(dir.join(&m.path)).ok()?;
            (meta.len() == m.size && mtime_ns(&meta) == m.mtime_ns && kind_of(&meta) == m.kind)
                .then(|| {
                    (
                        m.path.clone(),
                        StatEntry {
                            ino: meta.ino(),
                            size: meta.len(),
                            mtime_ns: m.mtime_ns,
                            ctime_ns: ctime_ns(&meta),
                            kind: m.kind,
                            state: Seen::Blob(m.blob),
                        },
                    )
                })
        })
        .collect();
    Ok(StatCache {
        recorded_at_ns,
        entries,
    })
}

/// Paths the scanner never treats as agent intent.
pub(crate) struct Exclusions {
    /// Relative paths skipped silently, with everything under them: `.aver`,
    /// seeded paths.
    pub silent: HashSet<String>,
    /// Base paths the checkout could not write.
    pub absent: HashSet<String>,
}

pub(crate) struct ScanInput<'a> {
    pub dir: &'a FsPath,
    pub clock_dir: &'a FsPath,
    pub base: &'a BaseFiles,
    pub current: &'a BTreeMap<String, Option<FileEntry>>,
    pub cache: &'a StatCache,
    pub exclusions: &'a Exclusions,
    /// Ignore files outside the directory: the repo's `.git/info/exclude`
    /// and the user's global excludes file.
    pub extra_ignores: &'a [PathBuf],
    pub max_file_bytes: u64,
}

/// The desired overlay of a scan, its new blobs and its stat cache.
pub(crate) struct DiskState {
    /// The full desired overlay: a file entry to put, or `None` to delete.
    pub desired: BTreeMap<String, Option<FileEntry>>,
    /// Bytes of blobs hashed by this scan that neither the base nor the
    /// current overlay holds.
    pub new_blobs: HashMap<BlobId, Vec<u8>>,
    pub skipped: Vec<Skipped>,
    pub stat: StatCache,
    pub stats: ScanStats,
}

/// One entry found by the walk.
enum Found {
    File {
        path: String,
        entry: FileEntry,
        stat: StatEntry,
        bytes: Option<Vec<u8>>,
        hashed: bool,
    },
    Skipped {
        path: String,
        reason: SkipReason,
        stat: Option<StatEntry>,
    },
}

/// The ignore rules in effect for a directory: its own `.gitignore` and its
/// ancestors', deepest first.
struct Chain {
    rules: Option<Gitignore>,
    parent: Option<Arc<Chain>>,
}

struct Walk<'a> {
    input: &'a ScanInput<'a>,
    root_rules: Vec<Gitignore>,
}

impl Walk<'_> {
    fn ignored(&self, chain: &Chain, abs: &FsPath, is_dir: bool) -> bool {
        let mut link = Some(chain);
        while let Some(c) = link {
            if let Some(rules) = &c.rules {
                match rules.matched(abs, is_dir) {
                    Match::Ignore(_) => return true,
                    Match::Whitelist(_) => return false,
                    Match::None => {}
                }
            }
            link = c.parent.as_deref();
        }
        for rules in &self.root_rules {
            match rules.matched(abs, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }

    fn dir(&self, rel: &str, abs: &FsPath, parent: &Arc<Chain>, ignored: bool) -> Vec<Found> {
        let entries = match fs::read_dir(abs) {
            Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
            Err(e) => {
                return vec![Found::Skipped {
                    path: rel.to_owned(),
                    reason: SkipReason::Unreadable {
                        error: e.to_string(),
                    },
                    stat: None,
                }]
            }
        };
        let rules = entries
            .iter()
            .any(|e| e.file_name() == ".gitignore")
            .then(|| {
                let mut builder = GitignoreBuilder::new(abs);
                builder.add(abs.join(".gitignore"));
                builder.build().ok()
            })
            .flatten();
        let chain = Arc::new(Chain {
            rules,
            parent: Some(parent.clone()),
        });
        entries
            .par_iter()
            .flat_map_iter(|entry| self.entry(rel, entry, &chain, ignored))
            .collect()
    }

    fn entry(
        &self,
        parent_rel: &str,
        entry: &fs::DirEntry,
        chain: &Arc<Chain>,
        parent_ignored: bool,
    ) -> Vec<Found> {
        let os_name = entry.file_name();
        if os_name.as_bytes().eq_ignore_ascii_case(b".git") {
            return Vec::new();
        }
        let join = |name: &str| {
            if parent_rel.is_empty() {
                name.to_owned()
            } else {
                format!("{parent_rel}/{name}")
            }
        };
        let name = match os_name.to_str().map(|s| s.parse::<Name>()) {
            Some(Ok(name)) => name,
            _ => {
                return vec![Found::Skipped {
                    path: join(&os_name.to_string_lossy()),
                    reason: SkipReason::InvalidPath,
                    stat: None,
                }];
            }
        };
        let rel = join(name.as_str());
        if self.input.exclusions.silent.contains(&rel) {
            return Vec::new();
        }
        let abs = entry.path();
        let Ok(meta) = fs::symlink_metadata(&abs) else {
            return Vec::new();
        };
        let tracked = self.input.base.files.contains_key(&rel);
        if meta.is_dir() {
            let has_tracked = self.input.base.dirs.contains(&rel);
            let ignored = parent_ignored || self.ignored(chain, &abs, true);
            if ignored && !has_tracked {
                return Vec::new();
            }
            return self.dir(&rel, &abs, chain, ignored);
        }
        if !tracked && (parent_ignored || self.ignored(chain, &abs, false) || os_clutter(&name)) {
            return Vec::new();
        }
        vec![self.file(rel, &abs, meta)]
    }

    fn file(&self, path: String, abs: &FsPath, meta: fs::Metadata) -> Found {
        let ft = meta.file_type();
        if !ft.is_file() && !ft.is_symlink() {
            return Found::Skipped {
                path,
                reason: SkipReason::Special,
                stat: None,
            };
        }
        let kind = kind_of(&meta);
        let cache = self.input.cache;
        if let Some(old) = cache.entries.get(&path) {
            if old.ino == meta.ino()
                && old.size == meta.len()
                && old.mtime_ns == mtime_ns(&meta)
                && old.ctime_ns == ctime_ns(&meta)
                && old.kind == kind
                && old.mtime_ns < cache.recorded_at_ns
            {
                return match &old.state {
                    Seen::Blob(blob) => Found::File {
                        path,
                        entry: FileEntry { kind, blob: *blob },
                        stat: old.clone(),
                        bytes: None,
                        hashed: false,
                    },
                    Seen::Skipped(reason) => Found::Skipped {
                        path,
                        reason: reason.clone(),
                        stat: Some(old.clone()),
                    },
                };
            }
        }
        let stat_of = |meta: &fs::Metadata, state: Seen| StatEntry {
            ino: meta.ino(),
            size: meta.len(),
            mtime_ns: mtime_ns(meta),
            ctime_ns: ctime_ns(meta),
            kind: kind_of(meta),
            state,
        };
        let skip = |reason: SkipReason, meta: &fs::Metadata| Found::Skipped {
            path: path.clone(),
            stat: Some(stat_of(meta, Seen::Skipped(reason.clone()))),
            reason,
        };
        if kind != FileKind::Symlink && meta.len() >= self.input.max_file_bytes {
            return skip(SkipReason::TooLarge { bytes: meta.len() }, &meta);
        }
        let mut before = meta;
        for _ in 0..READ_ATTEMPTS {
            let bytes = match read_entry(abs, kind) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Found::Skipped {
                        path,
                        reason: SkipReason::Unstable,
                        stat: None,
                    }
                }
                Err(e) => {
                    return skip(
                        SkipReason::Unreadable {
                            error: e.to_string(),
                        },
                        &before,
                    )
                }
            };
            let Ok(after) = fs::symlink_metadata(abs) else {
                break;
            };
            if !same_stat(&before, &after) {
                before = after;
                continue;
            }
            if kind == FileKind::Symlink && !symlink_stays_inside(&path, &bytes) {
                return skip(SkipReason::SymlinkEscapes, &after);
            }
            let blob = BlobId::of_bytes(&bytes);
            let known = self.input.base.files.get(&path).map(|f| f.blob) == Some(blob)
                || self
                    .input
                    .current
                    .get(&path)
                    .and_then(|c| c.map(|f| f.blob))
                    == Some(blob);
            // Known content is in the store already (a fixture key the
            // base holds, hashed again when its stat cache entry is racy).
            let typed: Path = path.parse().expect("a path of valid names is valid");
            if let Some(finding) = (!known)
                .then(|| crate::secrets::scan(&typed, &bytes))
                .flatten()
            {
                return skip(
                    SkipReason::Secret {
                        rule: finding.pattern.to_owned(),
                    },
                    &after,
                );
            }
            return Found::File {
                stat: stat_of(&after, Seen::Blob(blob)),
                entry: FileEntry { kind, blob },
                bytes: (!known).then_some(bytes),
                hashed: true,
                path,
            };
        }
        Found::Skipped {
            path,
            reason: SkipReason::Unstable,
            stat: None,
        }
    }
}

/// Files macOS writes into any directory a person opens (Finder state,
/// AppleDouble metadata): never agent intent unless the base tracks them.
fn os_clutter(name: &Name) -> bool {
    name.as_str() == ".DS_Store" || name.as_str().starts_with("._")
}

fn read_entry(abs: &FsPath, kind: FileKind) -> io::Result<Vec<u8>> {
    if kind == FileKind::Symlink {
        return Ok(fs::read_link(abs)?.as_os_str().as_bytes().to_vec());
    }
    let mut file = fs::File::open(abs)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Whether a symlink at `path` with `target` resolves, lexically, to a path
/// inside the workspace directory.
pub(crate) fn symlink_stays_inside(path: &str, target: &[u8]) -> bool {
    let Ok(target) = std::str::from_utf8(target) else {
        return false;
    };
    if target.is_empty() || target.starts_with('/') {
        return false;
    }
    let mut depth: Vec<&str> = path.split('/').collect();
    depth.pop();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            other => depth.push(other),
        }
    }
    true
}

/// Scans the directory into a desired overlay. Writes nothing to the store.
pub(crate) fn scan(input: &ScanInput<'_>) -> Result<DiskState, DiskError> {
    let started = Instant::now();
    if !input.dir.is_dir() {
        return Err(DiskError::DirMissing(input.dir.to_owned()));
    }
    let recorded_at_ns = fs_clock(input.clock_dir)?;
    let root_rules = input
        .extra_ignores
        .iter()
        .filter(|p| p.is_file())
        .filter_map(|p| {
            let mut builder = GitignoreBuilder::new(input.dir);
            builder.add(p);
            builder.build().ok()
        })
        .collect();
    let walk = Walk { input, root_rules };
    let root = Arc::new(Chain {
        rules: None,
        parent: None,
    });
    let found = walk.dir("", input.dir, &root, false);
    // An unreadable root would scan as an empty project and delete every
    // tracked file from the overlay.
    if let [Found::Skipped {
        path,
        reason: SkipReason::Unreadable { error },
        ..
    }] = found.as_slice()
    {
        if path.is_empty() {
            return Err(io::Error::other(format!(
                "cannot read workspace directory {}: {error}",
                input.dir.display()
            ))
            .into());
        }
    }

    let mut stats = ScanStats {
        scanned: 0,
        hashed: 0,
        took_ms: 0,
    };
    let mut stat = StatCache {
        recorded_at_ns,
        entries: HashMap::with_capacity(found.len()),
    };
    let mut on_disk: HashMap<String, FileEntry> = HashMap::with_capacity(found.len());
    let mut new_blobs = HashMap::new();
    let mut skipped = Vec::new();
    let mut held: HashSet<String> = HashSet::new();
    for item in found {
        match item {
            Found::File {
                path,
                entry,
                stat: entry_stat,
                bytes,
                hashed,
            } => {
                stats.scanned += 1;
                stats.hashed += u64::from(hashed);
                if let Some(bytes) = bytes {
                    new_blobs.insert(entry.blob, bytes);
                }
                stat.entries.insert(path.clone(), entry_stat);
                on_disk.insert(path, entry);
            }
            Found::Skipped {
                path,
                reason,
                stat: entry_stat,
            } => {
                stats.scanned += 1;
                if let Some(entry_stat) = entry_stat {
                    stat.entries.insert(path.clone(), entry_stat);
                }
                held.insert(path.clone());
                skipped.push(Skipped {
                    tracked: input.base.files.contains_key(&path),
                    path,
                    reason,
                });
            }
        }
    }
    skipped.sort_by(|a, b| a.path.cmp(&b.path));

    let kept = |path: &str| -> bool {
        if input.exclusions.absent.contains(path) {
            return true;
        }
        let mut prefix = path;
        loop {
            if held.contains(prefix) || input.exclusions.silent.contains(prefix) {
                return true;
            }
            match prefix.rfind('/') {
                Some(i) => prefix = &prefix[..i],
                None => return false,
            }
        }
    };
    let mut desired: BTreeMap<String, Option<FileEntry>> = BTreeMap::new();
    for (path, entry) in &on_disk {
        if input.base.files.get(path) != Some(entry) {
            desired.insert(path.clone(), Some(*entry));
        }
    }
    for path in input.base.files.keys() {
        if on_disk.contains_key(path) {
            continue;
        }
        if kept(path) {
            if let Some(row) = input.current.get(path) {
                desired.insert(path.clone(), *row);
            }
        } else {
            desired.insert(path.clone(), None);
        }
    }
    // A kept file (skipped, absent or under a held directory) and a file the
    // disk has under the same name as a directory cannot both exist: the disk
    // wins, so the kept file becomes a delete.
    let puts: Vec<String> = desired
        .iter()
        .filter(|(_, v)| v.is_some())
        .map(|(k, _)| k.clone())
        .collect();
    for path in puts {
        let mut prefix = path.as_str();
        while let Some(i) = prefix.rfind('/') {
            prefix = &prefix[..i];
            let file_there = match desired.get(prefix) {
                Some(row) => row.is_some(),
                None => input.base.files.contains_key(prefix),
            };
            if file_there {
                desired.insert(prefix.to_owned(), None);
            }
        }
    }
    let wanted: HashSet<BlobId> = desired.values().flatten().map(|f| f.blob).collect();
    new_blobs.retain(|blob, _| wanted.contains(blob));
    stats.took_ms = started.elapsed().as_millis() as u64;
    Ok(DiskState {
        desired,
        new_blobs,
        skipped,
        stat,
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-file directory, its base, and a stat cache entry that matches
    /// the file's current stat but names a stale blob, recorded at
    /// `recorded_at` relative to the file's mtime.
    fn stale_entry(edit_ctime: bool, recorded_after_mtime: bool) -> (BlobId, BlobId) {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let mat = dir.path().join("mat");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&mat).unwrap();
        fs::write(ws.join("a.txt"), "new!").unwrap();
        let meta = fs::symlink_metadata(ws.join("a.txt")).unwrap();
        let stale = BlobId::of_bytes(b"old!");
        let fresh = BlobId::of_bytes(b"new!");
        let entry = StatEntry {
            ino: meta.ino(),
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            ctime_ns: ctime_ns(&meta) - i64::from(edit_ctime),
            kind: FileKind::File,
            state: Seen::Blob(stale),
        };
        let cache = StatCache {
            recorded_at_ns: entry.mtime_ns + i64::from(recorded_after_mtime),
            entries: HashMap::from([("a.txt".to_owned(), entry)]),
        };
        let base = BaseFiles::new([(
            "a.txt".to_owned(),
            FileEntry {
                kind: FileKind::File,
                blob: stale,
            },
        )]);
        let current = BTreeMap::new();
        let exclusions = Exclusions {
            silent: HashSet::new(),
            absent: HashSet::new(),
        };
        let state = scan(&ScanInput {
            dir: &ws,
            clock_dir: &mat,
            base: &base,
            current: &current,
            cache: &cache,
            exclusions: &exclusions,
            extra_ignores: &[],
            max_file_bytes: u64::MAX,
        })
        .unwrap();
        let seen = state
            .desired
            .get("a.txt")
            .copied()
            .flatten()
            .map_or(stale, |f| f.blob);
        (seen, fresh)
    }

    #[test]
    fn trusted_entry_skips_hashing() {
        let (seen, fresh) = stale_entry(false, true);
        assert_ne!(
            seen, fresh,
            "a matching, old enough entry is reused without reading"
        );
    }

    /// An entry recorded in the same clock tick as the file's mtime may
    /// predate a same-size edit in that tick, so it is never trusted.
    #[test]
    fn racy_same_size_edit_detected() {
        let (seen, fresh) = stale_entry(false, false);
        assert_eq!(seen, fresh);
    }

    #[test]
    fn ctime_change_rehashes() {
        let (seen, fresh) = stale_entry(true, true);
        assert_eq!(seen, fresh);
    }

    #[test]
    fn stat_cache_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = StatCache {
            recorded_at_ns: 42,
            entries: HashMap::new(),
        };
        for (i, state) in [
            Seen::Blob(BlobId::of_bytes(b"x")),
            Seen::Skipped(SkipReason::Secret {
                rule: "aws-access-key-id".into(),
            }),
        ]
        .into_iter()
        .enumerate()
        {
            cache.entries.insert(
                format!("p{i}"),
                StatEntry {
                    ino: 1,
                    size: 2,
                    mtime_ns: 3,
                    ctime_ns: -4,
                    kind: FileKind::Exec,
                    state,
                },
            );
        }
        cache.save(&dir.path().join("stat")).unwrap();
        assert_eq!(StatCache::load(&dir.path().join("stat")), cache);
        assert_eq!(
            StatCache::load(&dir.path().join("missing")),
            StatCache::default()
        );
    }

    #[test]
    fn symlinks_must_stay_inside() {
        assert!(symlink_stays_inside("a/b", b"../c"));
        assert!(symlink_stays_inside("a/b", b"./c/../d"));
        assert!(!symlink_stays_inside("a/b", b"../../c"));
        assert!(!symlink_stays_inside("b", b"/etc/passwd"));
        assert!(!symlink_stays_inside("b", b""));
    }
}
