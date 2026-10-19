//! `aver workspace rebase` (ARCHITECTURE.md §2.3.1): move a workspace onto a
//! newer base by a three-way merge of its directory, keeping only the
//! agent's own changes in the overlay.
//!
//! The directory (ours) and the new base B' (theirs) are merged against the
//! old base B. A clean merge writes the files B' changed that the directory
//! lacks, moves the workspace and its attempt to B' (a new version of the
//! live row; no object changes) and the materialization's checkout with
//! it, and syncs again, so the overlay holds only what the agent changed
//! relative to B'. After an agent already merged B' into the directory
//! the merge is clean and writes nothing.
//!
//! A conflict in text files moves the workspace all the same: each
//! conflicting file gets Git's markers (`<<<<<<< yours`, `=======`,
//! `>>>>>>> <commit>`), the paths are recorded in `mat/<ws>/rebase.json`,
//! and `aver check`, `aver verify` and the next rebase refuse while a
//! marker line remains in one of them; once none does, the directory's
//! content is the resolution. A conflict no marker can show (binary
//! files, a changed file kind) changes nothing.

use std::fs;
use std::path::Path as FsPath;
use std::process::Command;

use aver_core::{AgentId, SnapshotId, Timestamp, WorkspaceId};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::disk::real_ancestors;
use super::merge::{self, Conflict, Hunk, MergeRule};
use super::{check_open, FileEntry, FileKind, Result, WorkspaceError, WorkspaceMode, Workspaces};
use crate::disk::MatState;

/// Paths to write into the directory: a file, or `None` to remove it.
type Steps = Vec<(String, Option<FileEntry>)>;

/// The conflicting paths a rebase marked, in `mat/<ws>/`.
const PENDING: &str = "rebase.json";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    onto: SnapshotId,
    paths: Vec<String>,
}

/// What a rebase did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebaseOutcome {
    pub from: SnapshotId,
    pub onto: SnapshotId,
    /// Whether the workspace now sits on `onto` (false on a conflict).
    pub rebased: bool,
    /// Files written into (or removed from) the directory: the new base's
    /// changes the directory did not hold yet, and line-merged files.
    pub written: Vec<String>,
    /// Paths the workspace still changes, relative to `onto`.
    pub changed: Vec<String>,
    /// Paths both sides changed incompatibly. Unless `marked` lists them,
    /// nothing was written.
    pub conflicts: Vec<Conflict>,
    /// Paths written with conflict markers, for the agent to resolve.
    #[serde(default)]
    pub marked: Vec<String>,
}

impl Workspaces {
    /// Rebases disk-mode workspace `id` onto `onto`, which must descend
    /// from its base in the repository the base came from.
    pub fn rebase(
        &self,
        caller: &AgentId,
        id: &WorkspaceId,
        onto: &SnapshotId,
    ) -> Result<RebaseOutcome> {
        let ws = self.get(caller, id)?;
        check_open(&ws)?;
        if ws.mode != WorkspaceMode::Disk {
            return Err(WorkspaceError::Invalid(format!(
                "workspace {id} has no directory to rebase"
            )));
        }
        let m = self
            .disk
            .materialization(id)?
            .filter(|m| m.state == MatState::Live)
            .ok_or_else(|| {
                WorkspaceError::Invalid(format!("workspace {id} has no live directory"))
            })?;
        let left = self.unresolved(id)?;
        if !left.is_empty() {
            return Err(unresolved_error(&left));
        }
        let from = ws.base;
        if from != *onto {
            self.check_descends(m.repo.as_deref(), &from, onto)?;
        }
        self.sync_row(&ws)?;
        let head = self.snapshot(caller, id)?;
        let mut out = RebaseOutcome {
            from,
            onto: *onto,
            rebased: false,
            written: Vec::new(),
            changed: Vec::new(),
            conflicts: Vec::new(),
            marked: Vec::new(),
        };
        if from != *onto {
            let merged = self.three_way(&from, &[head, *onto], MergeRule::Strict)?;
            let dir_files: std::collections::BTreeMap<String, FileEntry> =
                self.snapshot_files(&head)?.into_iter().collect();
            let changes: Steps = if merged.conflicts.is_empty() {
                merged
                    .changes
                    .iter()
                    .map(|(p, e)| (p.as_str().to_owned(), *e))
                    .collect()
            } else {
                let label = crate::git::imported_commit(&self.store, onto)?
                    .map_or_else(|| onto.to_string(), |c| c.as_str()[..12].to_owned());
                let Some((changes, marked)) = self.marked(&from, onto, &dir_files, &label)? else {
                    out.conflicts = merged.conflicts;
                    return Ok(out);
                };
                out.conflicts = merged.conflicts;
                out.marked = marked;
                changes
            };
            for (rel, entry) in &changes {
                let rel = rel.as_str();
                if dir_files.get(rel) == entry.as_ref() {
                    continue;
                }
                match entry {
                    Some(file) => self.write_base_file(&m.dir, rel, file)?,
                    None => remove_in(&m.dir, rel)?,
                }
                out.written.push(rel.to_owned());
            }
            let root = self.base_root(onto)?;
            let files = self.snapshot_files(onto)?;
            self.disk
                .ensure_checkout(&self.store, &root, || Ok(files))?;
            self.store.tx(|tx| {
                let row = super::load_row(&tx.sql, id)?.ok_or_else(|| super::not_found(id))?;
                check_open(&row)?;
                let now = Timestamp::now().to_string();
                tx.sql.execute(
                    "UPDATE workspaces SET base = ?2, version = ?3, updated_at = ?4 WHERE id = ?1",
                    params![
                        id.to_string(),
                        onto.to_string(),
                        (row.version + 1) as i64,
                        now
                    ],
                )?;
                tx.sql.execute(
                    "UPDATE attempts SET base = ?2 WHERE workspace = ?1",
                    params![id.to_string(), onto.to_string()],
                )?;
                tx.sql.execute(
                    "DELETE FROM overlay WHERE workspace = ?1",
                    params![id.to_string()],
                )?;
                tx.sql.execute(
                    "UPDATE materializations SET checkout = ?2 WHERE workspace = ?1",
                    params![id.to_string(), root.to_string()],
                )?;
                Ok::<_, WorkspaceError>(())
            })?;
            if !out.marked.is_empty() {
                let pending = Pending {
                    onto: *onto,
                    paths: out.marked.clone(),
                };
                crate::atomic::write(
                    &self.disk.mat_dir(id).join(PENDING),
                    &serde_json::to_vec(&pending).expect("pending rebases serialize"),
                    crate::atomic::Mode::Keep,
                )?;
            }
            let moved = self.get(caller, id)?;
            self.sync_row(&moved)?;
        }
        out.rebased = true;
        let (_, diff) = self.diff(caller, id)?;
        out.changed = diff.into_iter().map(|f| f.path.to_string()).collect();
        Ok(out)
    }

    /// What a conflicting rebase from `from` onto `onto` writes, for a
    /// directory holding `ours`: every path `onto` changed, as the clean
    /// merge has it or, where both sides changed lines incompatibly, with
    /// conflict markers; and the marked paths. `None` when a conflict
    /// cannot be marked (not text, a symlink, a changed file kind).
    fn marked(
        &self,
        from: &SnapshotId,
        onto: &SnapshotId,
        ours: &std::collections::BTreeMap<String, FileEntry>,
        label: &str,
    ) -> Result<Option<(Steps, Vec<String>)>> {
        let mut changes = Vec::new();
        let mut marked = Vec::new();
        for (path, old, theirs) in self.changed_files(from, onto)? {
            let mine = ours.get(&path).copied();
            if mine == old {
                changes.push((path, theirs));
                continue;
            }
            if mine == theirs {
                continue;
            }
            let present: Vec<FileEntry> = [old, mine, theirs].into_iter().flatten().collect();
            let kind = present[0].kind;
            if kind == FileKind::Symlink || present.iter().any(|f| f.kind != kind) {
                return Ok(None);
            }
            let read = |f: Option<FileEntry>| -> Result<Option<String>> {
                Ok(match f {
                    None => Some(String::new()),
                    Some(f) => String::from_utf8(self.store.get_raw(&f.blob)?)
                        .ok()
                        .filter(|t| !t.contains('\0')),
                })
            };
            let (Some(base), Some(yours), Some(new)) = (read(old)?, read(mine)?, read(theirs)?)
            else {
                return Ok(None);
            };
            let text = match merge::merge_text(&base, &[&yours, &new], MergeRule::Strict) {
                Ok(text) => text,
                Err(_) => {
                    marked.push(path.clone());
                    with_markers(&base, &yours, &new, label)
                }
            };
            let blob = self.store.put_blob(text.as_bytes())?;
            changes.push((path, Some(FileEntry { kind, blob })));
        }
        Ok(Some((changes, marked)))
    }

    /// The files a conflicting rebase of `id` marked that still hold a
    /// marker line. Once none does, the record is dropped: the directory's
    /// content is the resolution.
    pub fn unresolved(&self, id: &WorkspaceId) -> Result<Vec<String>> {
        let record = self.disk.mat_dir(id).join(PENDING);
        let pending: Pending = match fs::read(&record) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| WorkspaceError::Invalid(format!("{}: {e}", record.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let Some(m) = self
            .disk
            .materialization(id)?
            .filter(|m| m.state == MatState::Live)
        else {
            return Ok(Vec::new());
        };
        let left: Vec<String> = pending
            .paths
            .into_iter()
            .filter(|rel| {
                let at = m.dir.join(rel);
                real_ancestors(&m.dir, rel, false).unwrap_or(false)
                    && fs::symlink_metadata(&at).is_ok_and(|md| md.is_file())
                    && fs::read(&at).is_ok_and(|b| has_markers(&b))
            })
            .collect();
        if left.is_empty() {
            fs::remove_file(&record)?;
        }
        Ok(left)
    }

    /// Refuses `onto` unless both are imported commits of `repo` and
    /// `onto`'s descends from `from`'s.
    fn check_descends(
        &self,
        repo: Option<&FsPath>,
        from: &SnapshotId,
        onto: &SnapshotId,
    ) -> Result<()> {
        let invalid = |why: String| Err(WorkspaceError::Invalid(why));
        let (Some(old), Some(new)) = (
            crate::git::imported_commit(&self.store, from)?,
            crate::git::imported_commit(&self.store, onto)?,
        ) else {
            return invalid("a rebase needs both bases imported from Git commits".into());
        };
        let Some(repo) = repo else {
            return invalid("the workspace does not know its repository".into());
        };
        let descends = Command::new("git")
            .args(["merge-base", "--is-ancestor", old.as_str(), new.as_str()])
            .current_dir(repo)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .status()
            .is_ok_and(|s| s.success());
        if !descends {
            return invalid(format!(
                "{} does not descend from the workspace's base {}; rebase only moves forward",
                &new.as_str()[..12],
                &old.as_str()[..12]
            ));
        }
        Ok(())
    }
}

/// Removes `rel` from the directory, unless it lies behind a symlink.
pub(super) fn remove_in(dir: &FsPath, rel: &str) -> std::io::Result<()> {
    if !real_ancestors(dir, rel, false)? {
        return Ok(());
    }
    let target = dir.join(rel);
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(&target),
        Ok(_) => fs::remove_file(&target),
        Err(_) => Ok(()),
    }
}

/// The refusal while conflict markers remain in `paths`.
pub fn unresolved_error(paths: &[String]) -> WorkspaceError {
    WorkspaceError::Invalid(format!(
        "conflict markers from aver workspace rebase remain in {}: resolve each conflict and delete the marker lines, then run the command again",
        paths.join(", ")
    ))
}

fn has_markers(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes)
        .lines()
        .any(|l| l.starts_with("<<<<<<< ") || l == "=======" || l.starts_with(">>>>>>> "))
}

/// `base` with the changes of `yours` and `theirs`, where they conflict
/// both versions between Git's conflict markers.
fn with_markers(base: &str, yours: &str, theirs: &str, label: &str) -> String {
    let sides = [
        yours.split_inclusive('\n').collect::<Vec<_>>(),
        theirs.split_inclusive('\n').collect::<Vec<_>>(),
    ];
    let mut all: Vec<(usize, Hunk)> = merge::hunks(base, yours)
        .into_iter()
        .map(|h| (0, h))
        .chain(merge::hunks(base, theirs).into_iter().map(|h| (1, h)))
        .collect();
    all.sort_by_key(|(i, h)| (h.base.start, h.base.end, *i));
    let mut groups: Vec<Vec<(usize, Hunk)>> = Vec::new();
    for item in all {
        match groups.last_mut() {
            Some(group) if group.iter().any(|(_, h)| h.base.meets(&item.1.base)) => {
                group.push(item)
            }
            _ => groups.push(vec![item]),
        }
    }
    let base_lines: Vec<&str> = base.split_inclusive('\n').collect();
    let text = |i: usize, h: &Hunk| &sides[i][h.head.start as usize..h.head.end as usize];
    let mut out = String::with_capacity(base.len() + yours.len() + theirs.len());
    let mut at = 0usize;
    for group in groups {
        let (i0, first) = &group[0];
        let same = group
            .iter()
            .all(|(i, h)| h.base == first.base && text(*i, h) == text(*i0, first));
        let start = group.iter().map(|(_, h)| h.base.start).min().unwrap() as usize;
        let end = group.iter().map(|(_, h)| h.base.end).max().unwrap() as usize;
        out.extend(base_lines[at..start].iter().copied());
        if same {
            out.extend(text(*i0, first).iter().copied());
        } else {
            let side = |n: usize| {
                let mut part = String::new();
                let mut pos = start;
                for (_, h) in group.iter().filter(|(i, _)| *i == n) {
                    part.extend(base_lines[pos..h.base.start as usize].iter().copied());
                    part.extend(text(n, h).iter().copied());
                    pos = h.base.end as usize;
                }
                part.extend(base_lines[pos..end].iter().copied());
                if !part.is_empty() && !part.ends_with('\n') {
                    part.push('\n');
                }
                part
            };
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("<<<<<<< yours\n");
            out.push_str(&side(0));
            out.push_str("=======\n");
            out.push_str(&side(1));
            out.push_str(&format!(">>>>>>> {label}\n"));
        }
        at = end;
    }
    out.extend(base_lines[at..].iter().copied());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_hold_both_versions_of_the_conflicting_lines_only() {
        let base = "a\nb\nc\nd\n";
        let yours = "a\nB1\nc\nd\n";
        let theirs = "a\nB2\nc\nD\n";
        assert_eq!(
            with_markers(base, yours, theirs, "abc123"),
            "a\n<<<<<<< yours\nB1\n=======\nB2\n>>>>>>> abc123\nc\nD\n"
        );
        assert!(has_markers(
            with_markers(base, yours, theirs, "x").as_bytes()
        ));
        assert!(!has_markers(b"a\n== not a marker\n"));
    }

    #[test]
    fn a_file_both_sides_added_is_marked_whole() {
        assert_eq!(
            with_markers("", "one\n", "two", "abc"),
            "<<<<<<< yours\none\n=======\ntwo\n>>>>>>> abc\n"
        );
    }
}
