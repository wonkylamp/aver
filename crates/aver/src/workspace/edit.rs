//! Edit batches: validation and application of ops against the workspace's
//! current view (pending changes of this batch, then overlay, then base).

use std::collections::{BTreeMap, HashMap};

use aver_core::{Node, Path, TreeId, WorkspaceId, MAX_BLOB_BYTES};
use data_encoding::BASE64;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::tree::{node_at, walk_files, Loader};
use super::{overlay_row, overlay_rows, Change, FileEntry, FileKind, WorkspaceError};
use crate::store::{StoreError, Tx};

/// One edit op (ARCHITECTURE.md §2.3).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum EditOp {
    Write {
        path: Path,
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        content_b64: Option<String>,
    },
}

/// A batch of ops applied atomically if the workspace is at `if_version`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    pub if_version: u64,
    pub ops: Vec<EditOp>,
}

/// The state of one path after a batch: `{"path","kind","blob"}`, or
/// `{"path","deleted":true}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changed {
    pub path: Path,
    /// `None` when the batch deleted the path.
    #[serde(flatten)]
    pub file: Option<FileEntry>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

pub(super) struct Batch<'a, 'conn> {
    pub tx: &'a Tx<'conn>,
    pub workspace: WorkspaceId,
    pub base_root: TreeId,
    pub load: &'a Loader<'a>,
    pending: BTreeMap<String, Change>,
}

fn upper_bound(prefix: &str) -> String {
    format!("{prefix}0")
}

fn under(prefix: &str) -> String {
    format!("{prefix}/")
}

pub(super) fn parse_change(kind: &str, blob: Option<String>) -> Result<Change, StoreError> {
    let corrupt = |reason: String| StoreError::Corrupt {
        id: "overlay".into(),
        reason,
    };
    if kind == "delete" {
        return Ok(Change::Delete);
    }
    let kind = FileKind::parse(kind).ok_or_else(|| corrupt(format!("unknown kind {kind:?}")))?;
    let blob = blob
        .ok_or_else(|| corrupt("put row without blob".into()))?
        .parse()
        .map_err(|e: aver_core::ParseError| corrupt(e.to_string()))?;
    Ok(Change::Put(FileEntry { kind, blob }))
}

impl<'a, 'conn> Batch<'a, 'conn> {
    pub(super) fn new(
        tx: &'a Tx<'conn>,
        workspace: WorkspaceId,
        base_root: TreeId,
        load: &'a Loader<'a>,
    ) -> Self {
        Self {
            tx,
            workspace,
            base_root,
            load,
            pending: BTreeMap::new(),
        }
    }

    fn overlay_row(&self, path: &str) -> Result<Option<Change>, WorkspaceError> {
        Ok(overlay_row(&self.tx.sql, &self.workspace, path)?)
    }

    fn overlay_under(&self, dir: &str) -> Result<HashMap<String, Change>, WorkspaceError> {
        Ok(overlay_rows(&self.tx.sql, &self.workspace, Some(dir))?
            .into_iter()
            .map(|(path, change)| (path.as_str().to_owned(), change))
            .collect())
    }

    fn base_node(&self, path: &str) -> Result<Option<Node>, WorkspaceError> {
        Ok(node_at(self.load, &self.base_root, path)?)
    }

    fn base_file(&self, path: &str) -> Result<Option<FileEntry>, WorkspaceError> {
        Ok(self.base_node(path)?.as_ref().and_then(FileEntry::of_node))
    }

    /// The file at `path` in the current view.
    pub(super) fn file(&self, path: &str) -> Result<Option<FileEntry>, WorkspaceError> {
        if let Some(change) = self.pending.get(path) {
            return Ok(change.file());
        }
        if let Some(change) = self.overlay_row(path)? {
            return Ok(change.file());
        }
        self.base_file(path)
    }

    /// Every file under directory `dir` in the current view, sorted by path.
    /// Stops after the first file when `first_only` is set.
    fn files_under(
        &self,
        dir: &str,
        first_only: bool,
    ) -> Result<BTreeMap<String, FileEntry>, WorkspaceError> {
        let mut out = BTreeMap::new();
        let prefix = under(dir);
        let hi = upper_bound(dir);
        for (path, change) in self.pending.range(prefix.clone()..hi) {
            if let Change::Put(file) = change {
                out.insert(path.clone(), *file);
                if first_only {
                    return Ok(out);
                }
            }
        }
        let overlay = self.overlay_under(dir)?;
        for (path, change) in &overlay {
            if let (Change::Put(file), false) = (change, self.pending.contains_key(path)) {
                out.insert(path.clone(), *file);
                if first_only {
                    return Ok(out);
                }
            }
        }
        if let Some(Node::Tree(tree)) = self.base_node(dir)? {
            walk_files(self.load, &tree, dir, &mut |path, file| {
                if self.pending.contains_key(&path) || overlay.contains_key(&path) {
                    return Ok(false);
                }
                out.insert(path, file);
                Ok(first_only)
            })?;
        }
        Ok(out)
    }

    fn is_dir(&self, path: &str) -> Result<bool, WorkspaceError> {
        Ok(!self.files_under(path, true)?.is_empty())
    }

    /// Fails unless a new file can be created at `path`: nothing exists there
    /// and no ancestor is a file.
    fn check_creatable(&self, path: &Path) -> Result<(), WorkspaceError> {
        let text = path.as_str();
        for (i, _) in text.match_indices('/') {
            let ancestor = &text[..i];
            if self.file(ancestor)?.is_some() {
                return Err(WorkspaceError::PathConflict(format!(
                    "cannot create {text}: {ancestor} is a file"
                )));
            }
        }
        if self.file(text)?.is_some() {
            return Err(WorkspaceError::PathConflict(format!(
                "{text} already exists"
            )));
        }
        if self.is_dir(text)? {
            return Err(WorkspaceError::PathConflict(format!(
                "{text} is a directory"
            )));
        }
        Ok(())
    }

    /// Scans and stores `bytes` as the new content of `path`.
    fn store(
        &self,
        path: &Path,
        kind: FileKind,
        bytes: &[u8],
    ) -> Result<FileEntry, WorkspaceError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(WorkspaceError::BlobTooLarge(bytes.len()));
        }
        if let Some(finding) = crate::secrets::scan(path, bytes) {
            return Err(WorkspaceError::Secret(finding));
        }
        let blob = self.tx.put_blob(bytes)?;
        Ok(FileEntry { kind, blob })
    }

    fn set(&mut self, path: &Path, change: Change) {
        self.pending.insert(path.as_str().to_owned(), change);
    }

    pub(super) fn apply(&mut self, op: EditOp) -> Result<(), WorkspaceError> {
        match op {
            EditOp::Write {
                path,
                content,
                content_b64,
            } => {
                let bytes = match (content, content_b64) {
                    (Some(text), None) => text.into_bytes(),
                    (None, Some(b64)) => BASE64.decode(b64.as_bytes()).map_err(|e| {
                        WorkspaceError::Invalid(format!("{path}: content_b64 is not base64: {e}"))
                    })?,
                    _ => {
                        return Err(WorkspaceError::Invalid(format!(
                            "{path}: write needs exactly one of content and content_b64"
                        )))
                    }
                };
                let current = self.file(path.as_str())?;
                if current.is_none() {
                    self.check_creatable(&path)?;
                }
                let kind = match current {
                    Some(FileEntry {
                        kind: FileKind::Exec,
                        ..
                    }) => FileKind::Exec,
                    _ => FileKind::File,
                };
                let file = self.store(&path, kind, &bytes)?;
                self.set(&path, Change::Put(file));
            }
        }
        Ok(())
    }

    /// Writes the batch's net effect to the overlay. Paths whose final state
    /// equals the base lose their overlay row, so reverting an edit leaves
    /// nothing behind.
    pub(super) fn flush(self) -> Result<Vec<Changed>, WorkspaceError> {
        let mut changed = Vec::with_capacity(self.pending.len());
        for (path, change) in &self.pending {
            let base = self.base_file(path)?;
            let redundant = match change {
                Change::Put(file) => base == Some(*file),
                Change::Delete => base.is_none(),
            };
            if redundant {
                self.tx.sql.execute(
                    "DELETE FROM overlay WHERE workspace = ?1 AND path = ?2",
                    params![self.workspace.to_string(), path],
                )?;
            } else {
                let (kind, blob) = match change {
                    Change::Put(file) => (file.kind.as_str(), Some(file.blob.to_string())),
                    Change::Delete => ("delete", None),
                };
                self.tx.sql.execute(
                    "INSERT INTO overlay (workspace, path, kind, blob) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (workspace, path) DO UPDATE SET kind = excluded.kind, blob = excluded.blob",
                    params![self.workspace.to_string(), path, kind, blob],
                )?;
            }
            changed.push(Changed {
                path: path.parse().expect("pending paths are valid"),
                file: change.file(),
                deleted: change.file().is_none(),
            });
        }
        Ok(changed)
    }
}
