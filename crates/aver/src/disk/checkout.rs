//! Checkouts (§1.8.2): one immutable directory per base tree, written once
//! from the store and cloned into every workspace on that base. Apple
//! documents that a directory clone blocks changes to its source while it
//! runs, so workspaces are cloned from this quiet cache, never from the
//! user's live checkout.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path as FsPath, PathBuf};

use aver_core::{BlobId, Timestamp, TreeId};
use rayon::prelude::*;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{Disk, DiskError, FileLock};
use crate::store::Store;
use crate::workspace::{FileEntry, FileKind};

/// Longest file name most filesystems accept, in bytes.
const MAX_NAME_BYTES: usize = 255;

/// One written file of a checkout: what the stat cache of a clone needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManifestEntry {
    pub path: String,
    pub kind: FileKind,
    pub blob: BlobId,
    pub size: u64,
    pub mtime_ns: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct Checkout {
    pub tree: TreeId,
    pub dir: PathBuf,
    pub absent: Vec<String>,
}

impl Disk {
    pub(crate) fn checkout_dir(&self, tree: &TreeId) -> PathBuf {
        self.root().join("checkouts").join(tree.to_string())
    }

    fn manifest_path(&self, tree: &TreeId) -> PathBuf {
        self.root()
            .join("checkouts")
            .join(format!("{tree}.manifest"))
    }

    /// The checkout of `tree`, writing it from `files` (every file of the
    /// tree, in path order) if it does not exist yet.
    pub(crate) fn ensure_checkout(
        &self,
        store: &Store,
        tree: &TreeId,
        files: impl FnOnce() -> Result<Vec<(String, FileEntry)>, DiskError>,
    ) -> Result<Checkout, DiskError> {
        let dir = self.checkout_dir(tree);
        let parent = dir.parent().expect("checkouts live in a directory");
        fs::create_dir_all(parent)?;
        let _lock = FileLock::exclusive(&parent.join(format!("{tree}.lock")))?;
        if let Some(absent) = ready_row(store, tree)? {
            if dir.is_dir() && self.manifest_path(tree).is_file() {
                touch(store, tree)?;
                return Ok(Checkout {
                    tree: *tree,
                    dir,
                    absent,
                });
            }
        }
        let files = files()?;
        let tmp = parent.join(format!("{tree}.tmp-{}", ulid::Ulid::generate()));
        let built = self.write_checkout(store, &tmp, &files);
        let (manifest, absent) = match built {
            Ok(done) => done,
            Err(e) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(e);
            }
        };
        if dir.exists() {
            self.trash(&dir)?;
        }
        write_manifest(&self.manifest_path(tree), &manifest)?;
        fs::rename(&tmp, &dir)?;
        let bytes: u64 = manifest.iter().map(|m| m.size).sum();
        let now = Timestamp::now().to_string();
        store.tx(|tx| {
            tx.sql.execute(
                "INSERT INTO checkouts (tree, dir, files, bytes, absent, state, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'ready', ?6)
                 ON CONFLICT (tree) DO UPDATE SET dir = excluded.dir, files = excluded.files,
                   bytes = excluded.bytes, absent = excluded.absent, state = 'ready',
                   last_used_at = excluded.last_used_at",
                params![
                    tree.to_string(),
                    dir.to_string_lossy(),
                    manifest.len() as i64,
                    bytes as i64,
                    serde_json::to_string(&absent).expect("paths serialize"),
                    now
                ],
            )?;
            Ok::<_, DiskError>(())
        })?;
        Ok(Checkout {
            tree: *tree,
            dir,
            absent,
        })
    }

    /// The manifest of a ready checkout.
    pub(crate) fn manifest(&self, tree: &TreeId) -> Result<Vec<ManifestEntry>, DiskError> {
        let compressed = fs::read(self.manifest_path(tree))?;
        let json = zstd::decode_all(&compressed[..])?;
        serde_json::from_slice(&json).map_err(|e| DiskError::Corrupt(format!("manifest: {e}")))
    }

    fn write_checkout(
        &self,
        store: &Store,
        dir: &FsPath,
        files: &[(String, FileEntry)],
    ) -> Result<(Vec<ManifestEntry>, Vec<String>), DiskError> {
        fs::create_dir_all(dir)?;
        let mut absent: BTreeSet<String> = BTreeSet::new();
        let folds = self.folds_case()?;
        // Folded spelling → the first exact spelling written under it, for
        // every directory and file, so two paths that a case-insensitive
        // filesystem would merge never share a directory.
        let mut spellings: HashMap<String, String> = HashMap::new();
        let mut writable = Vec::with_capacity(files.len());
        let mut dirs: BTreeSet<&str> = BTreeSet::new();
        'files: for (path, file) in files {
            if path.split('/').any(|s| s.len() > MAX_NAME_BYTES) {
                absent.insert(path.clone());
                continue;
            }
            // `.aver/` holds Aver's own files: a tracked `.aver`, or a link
            // under it, would steer their writes, so none of it is written.
            let top = path.split('/').next().unwrap_or(path);
            if top == ".aver" || (folds && top.eq_ignore_ascii_case(".aver")) {
                absent.insert(path.clone());
                continue;
            }
            if folds {
                let mut end = 0;
                let segments: Vec<&str> = path.split('/').collect();
                for segment in &segments {
                    end += segment.len();
                    let exact = &path[..end];
                    let folded = exact.to_lowercase();
                    match spellings.get(&folded) {
                        Some(seen) if seen != exact => {
                            absent.insert(path.clone());
                            continue 'files;
                        }
                        Some(_) => {}
                        None => {
                            spellings.insert(folded, exact.to_owned());
                        }
                    }
                    end += 1;
                }
            }
            if let Some(i) = path.rfind('/') {
                dirs.insert(&path[..i]);
            }
            writable.push((path, file));
        }
        for d in &dirs {
            fs::create_dir_all(dir.join(d))?;
        }
        let results: Vec<Result<Option<ManifestEntry>, DiskError>> = writable
            .par_iter()
            .map(|(path, file)| {
                let bytes = store.get_raw(&file.blob)?;
                let target = dir.join(path);
                let written = match file.kind {
                    FileKind::Symlink => symlink(std::ffi::OsStr::from_bytes(&bytes), &target),
                    kind => fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(if kind == FileKind::Exec { 0o755 } else { 0o644 })
                        .open(&target)
                        .and_then(|mut f| {
                            f.write_all(&bytes)?;
                            // The umask may have dropped bits the tree records.
                            f.set_permissions(fs::Permissions::from_mode(
                                if kind == FileKind::Exec { 0o755 } else { 0o644 },
                            ))
                        }),
                };
                match written {
                    Ok(()) => {}
                    Err(e)
                        if e.kind() == io::ErrorKind::AlreadyExists
                            || e.raw_os_error() == Some(libc::ENAMETOOLONG) =>
                    {
                        return Ok(None)
                    }
                    Err(e) => return Err(e.into()),
                }
                let meta = fs::symlink_metadata(&target)?;
                Ok(Some(ManifestEntry {
                    path: (*path).clone(),
                    kind: file.kind,
                    blob: file.blob,
                    size: meta.len(),
                    mtime_ns: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
                }))
            })
            .collect();
        let mut manifest = Vec::with_capacity(results.len());
        for (result, (path, _)) in results.into_iter().zip(&writable) {
            match result? {
                Some(entry) => manifest.push(entry),
                None => {
                    absent.insert((*path).clone());
                }
            }
        }
        // The probe goes beside the checkout, never among the tree's files.
        let beside = dir.parent().expect("checkouts live in a directory");
        wait_for_clock_past(beside, manifest.iter().map(|m| m.mtime_ns).max())?;
        Ok((manifest, absent.into_iter().collect()))
    }
}

fn write_manifest(path: &FsPath, manifest: &[ManifestEntry]) -> Result<(), DiskError> {
    let json = serde_json::to_vec(manifest).expect("manifests serialize");
    let compressed = zstd::encode_all(&json[..], 3)?;
    let tmp = path.with_extension(format!("tmp-{}", ulid::Ulid::generate()));
    fs::write(&tmp, compressed)?;
    fs::rename(tmp, path)?;
    Ok(())
}

/// The absent paths of a ready checkout row, or `None` without one.
fn ready_row(store: &Store, tree: &TreeId) -> Result<Option<Vec<String>>, DiskError> {
    let row: Option<String> = store.read(|conn| {
        Ok(conn
            .query_row(
                "SELECT absent FROM checkouts WHERE tree = ?1 AND state = 'ready'",
                [tree.to_string()],
                |r| r.get(0),
            )
            .optional()?)
    })?;
    row.map(|absent| {
        serde_json::from_str(&absent).map_err(|e| DiskError::Corrupt(format!("checkout row: {e}")))
    })
    .transpose()
}

fn touch(store: &Store, tree: &TreeId) -> Result<(), DiskError> {
    store.tx(|tx| {
        tx.sql.execute(
            "UPDATE checkouts SET last_used_at = ?2 WHERE tree = ?1",
            params![tree.to_string(), Timestamp::now().to_string()],
        )?;
        Ok::<_, DiskError>(())
    })
}

/// Returns once the filesystem clock is past `newest`, so a stat cache
/// seeded from this checkout later trusts its files: on filesystems with
/// coarse timestamps a file written in the same tick as a scan's clock is
/// racy and would be rehashed on the first sync.
fn wait_for_clock_past(dir: &FsPath, newest: Option<i64>) -> Result<(), DiskError> {
    let Some(newest) = newest else {
        return Ok(());
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while super::scan::fs_clock(dir)? <= newest && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    Ok(())
}
