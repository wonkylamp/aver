//! Three-way merge of snapshots that share a base (§2.10, `aver combine`).
//!
//! Per path: a path one head changed is taken from it, the same change made
//! by several heads is taken once, and when several heads change the same
//! text file differently their line hunks are merged when the base ranges
//! they replace are disjoint and do not touch. Under
//! [`MergeRule::UnionInsertions`], heads that only add lines at the same
//! point of the base each keep theirs, in head order. Anything else is a
//! conflict, reported with the base lines and each head's lines involved;
//! nothing is ever resolved by guessing.

use std::collections::BTreeMap;
use std::ops::Range;

use aver_core::{AgentId, Path, SnapshotId, Timestamp};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use similar::{capture_diff_slices, Algorithm, DiffTag};

use super::{FileEntry, FileKind, Result, Workspace, Workspaces};
use crate::coord::MAX_HUNK_LINES;

/// Lines `start..end` (0-based, end exclusive). An empty range is an
/// insertion point before line `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lines {
    pub start: u32,
    pub end: u32,
}

impl Lines {
    fn of(r: &Range<usize>) -> Self {
        Self {
            start: r.start as u32,
            end: r.end as u32,
        }
    }

    /// `12-18`, `12`, or `before 12` for an insertion point (1-based).
    pub fn human(&self) -> String {
        match self.end.saturating_sub(self.start) {
            0 => format!("before line {}", self.start + 1),
            1 => format!("line {}", self.start + 1),
            _ => format!("lines {}-{}", self.start + 1, self.end),
        }
    }

    /// Whether two base ranges overlap or touch: such edits cannot be
    /// combined line by line.
    pub fn meets(&self, other: &Lines) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// One contiguous change a head made: base lines `base` became the head's
/// lines `head`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub base: Lines,
    pub head: Lines,
}

/// Splits text into lines that keep their terminator.
fn split(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The hunks turning `base` into `head`, in base order. Adjacent edits are
/// one hunk.
pub fn hunks(base: &str, head: &str) -> Vec<Hunk> {
    let (b, h) = (split(base), split(head));
    let mut out: Vec<Hunk> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, &b, &h) {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        match out.last_mut() {
            Some(last)
                if last.base.end as usize == old.start && last.head.end as usize == new.start =>
            {
                last.base.end = old.end as u32;
                last.head.end = new.end as u32;
            }
            _ => out.push(Hunk {
                base: Lines::of(&old),
                head: Lines::of(&new),
            }),
        }
    }
    out
}

/// One head's part in a conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    /// The head's position in the merge's input (0-based).
    pub head: usize,
    /// The base lines it replaced.
    pub base: Lines,
    /// What it put there, as lines of its version of the file.
    pub lines: Lines,
    /// Those lines' text: at most [`MAX_HUNK_LINES`] lines of at most
    /// [`MAX_SIDE_CHARS`] characters each, without line terminators
    /// (`lines` says how many there are). Repository content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<String>,
}

/// The longest line of a [`Side`]'s `text`; longer ones end in `…`.
pub const MAX_SIDE_CHARS: usize = 160;

/// A side's lines as [`Side::text`] keeps them.
fn side_text(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .take(MAX_HUNK_LINES)
        .map(|line| {
            let line = line.trim_end_matches(['\n', '\r']);
            match line.char_indices().nth(MAX_SIDE_CHARS) {
                Some((i, _)) => format!("{}…", &line[..i]),
                None => line.to_owned(),
            }
        })
        .collect()
}

/// Base lines several heads changed differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictHunk {
    pub base: Lines,
    pub sides: Vec<Side>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Overlapping or touching line changes.
    Content,
    /// One head deleted the file, another changed it.
    DeleteModify,
    /// Several heads added the file with different contents.
    AddAdd,
    /// A head changed the file's kind (file, executable, symlink).
    Kind,
    /// Not text (not UTF-8, or holds a NUL byte).
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    pub kind: ConflictKind,
    /// The heads (positions) that changed the path.
    pub heads: Vec<usize>,
    /// For `content`: each group of base lines changed differently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hunks: Vec<ConflictHunk>,
}

/// How a merge treats heads that change the same place of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRule {
    /// Different changes that overlap or touch conflict, insertions at one
    /// point included.
    Strict,
    /// As [`MergeRule::Strict`], except that heads which only insert lines
    /// at one point of the base, deleting and changing none there, all keep
    /// their lines, in head order (an insertion several heads made alike is
    /// taken once): two tasks each appending a test to the end of one file.
    /// The result is only as sound as its verification, so `aver combine`,
    /// which verifies the merge against every task, uses it.
    UnionInsertions,
}

/// Merges several heads' versions of one text file onto the base's under
/// `rule`. `Err` holds the conflicting hunks.
pub fn merge_text(
    base: &str,
    heads: &[&str],
    rule: MergeRule,
) -> std::result::Result<String, Vec<ConflictHunk>> {
    let mut all: Vec<(usize, Hunk, Vec<&str>)> = Vec::new();
    for (i, head) in heads.iter().enumerate() {
        let lines = split(head);
        for h in hunks(base, head) {
            let text = lines[h.head.start as usize..h.head.end as usize].to_vec();
            all.push((i, h, text));
        }
    }
    all.sort_by_key(|(i, h, _)| (h.base.start, h.base.end, *i));
    let mut groups: Vec<Vec<(usize, Hunk, Vec<&str>)>> = Vec::new();
    for item in all {
        match groups.last_mut() {
            Some(group) if group.iter().any(|(_, h, _)| h.base.meets(&item.1.base)) => {
                group.push(item)
            }
            _ => groups.push(vec![item]),
        }
    }
    let base_lines = split(base);
    let mut out = String::with_capacity(base.len());
    let mut at = 0usize;
    let mut conflicts = Vec::new();
    for group in groups {
        let (_, first, text) = &group[0];
        let same = group
            .iter()
            .all(|(_, h, t)| h.base == first.base && t == text);
        // Insertion points that meet are one point.
        let insertions = group.iter().all(|(_, h, _)| h.base.start == h.base.end);
        if !same && insertions && rule == MergeRule::UnionInsertions {
            out.extend(base_lines[at..first.base.start as usize].iter().copied());
            let mut taken: Vec<&Vec<&str>> = Vec::new();
            for (_, _, t) in &group {
                if !taken.contains(&t) {
                    out.extend(t.iter().copied());
                    taken.push(t);
                }
            }
            at = first.base.start as usize;
            continue;
        }
        if !same {
            let start = group.iter().map(|(_, h, _)| h.base.start).min().unwrap();
            let end = group.iter().map(|(_, h, _)| h.base.end).max().unwrap();
            conflicts.push(ConflictHunk {
                base: Lines { start, end },
                sides: group
                    .iter()
                    .map(|(i, h, t)| Side {
                        head: *i,
                        base: h.base,
                        lines: h.head,
                        text: side_text(t),
                    })
                    .collect(),
            });
            continue;
        }
        let (s, e) = (first.base.start as usize, first.base.end as usize);
        out.extend(base_lines[at..s].iter().copied());
        out.extend(text.iter().copied());
        at = e;
    }
    if !conflicts.is_empty() {
        return Err(conflicts);
    }
    out.extend(base_lines[at..].iter().copied());
    Ok(out)
}

fn parse(path: &str) -> Result<Path> {
    path.parse()
        .map_err(|e| super::WorkspaceError::Invalid(format!("{path}: {e}")))
}

/// The text of a file, or `None` when it is not text.
fn text(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|t| !t.contains('\0'))
}

/// The result of [`Workspaces::three_way`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Merged {
    /// The merged changes against the base, in path order: a file entry to
    /// put, or `None` to delete. Empty when there are conflicts.
    pub changes: Vec<(Path, Option<FileEntry>)>,
    /// Paths changed by more than one head and merged line by line.
    pub merged: Vec<String>,
    pub conflicts: Vec<Conflict>,
}

impl Workspaces {
    /// Merges `heads`, each a snapshot of the same `base`, onto it under
    /// `rule`. Writes the blobs of line-merged files; nothing else.
    pub fn three_way(
        &self,
        base: &SnapshotId,
        heads: &[SnapshotId],
        rule: MergeRule,
    ) -> Result<Merged> {
        let mut paths: BTreeMap<String, (Option<FileEntry>, Vec<Option<FileEntry>>)> =
            BTreeMap::new();
        for (i, head) in heads.iter().enumerate() {
            for (path, old, new) in self.changed_files(base, head)? {
                let entry = paths
                    .entry(path)
                    .or_insert_with(|| (old, vec![old; heads.len()]));
                entry.1[i] = new;
            }
        }
        let mut out = Merged::default();
        for (path, (old, values)) in paths {
            let changed: Vec<usize> = (0..values.len()).filter(|i| values[*i] != old).collect();
            let first = values[changed[0]];
            if changed.iter().all(|i| values[*i] == first) {
                out.changes.push((parse(&path)?, first));
                continue;
            }
            let conflict = |kind| Conflict {
                path: path.clone(),
                kind,
                heads: changed.clone(),
                hunks: Vec::new(),
            };
            let Some(base_file) = old else {
                out.conflicts.push(conflict(ConflictKind::AddAdd));
                continue;
            };
            if changed.iter().any(|i| values[*i].is_none()) {
                out.conflicts.push(conflict(ConflictKind::DeleteModify));
                continue;
            }
            let kinds_change = changed
                .iter()
                .any(|i| values[*i].map(|f| f.kind) != Some(base_file.kind));
            if kinds_change {
                out.conflicts.push(conflict(ConflictKind::Kind));
                continue;
            }
            let base_bytes = self.store.get_raw(&base_file.blob)?;
            let mut head_bytes = Vec::new();
            for f in values.iter().flatten() {
                head_bytes.push(self.store.get_raw(&f.blob)?);
            }
            let texts: Option<Vec<&str>> = head_bytes.iter().map(|b| text(b)).collect();
            let (Some(base_text), Some(texts), false) = (
                text(&base_bytes),
                texts,
                base_file.kind == FileKind::Symlink,
            ) else {
                out.conflicts.push(conflict(ConflictKind::Binary));
                continue;
            };
            match merge_text(base_text, &texts, rule) {
                Ok(merged) => {
                    let blob = self.store.put_blob(merged.as_bytes())?;
                    out.merged.push(path.clone());
                    out.changes.push((
                        parse(&path)?,
                        Some(FileEntry {
                            kind: base_file.kind,
                            blob,
                        }),
                    ));
                }
                Err(hunks) => out.conflicts.push(Conflict {
                    hunks,
                    ..conflict(ConflictKind::Content)
                }),
            }
        }
        if !out.conflicts.is_empty() {
            out.changes.clear();
            out.merged.clear();
        }
        Ok(out)
    }
}

impl Workspaces {
    /// Opens a workspace on `base` for `owner` whose overlay is `changes`
    /// (blobs already stored), at version 1: a merge result to verify and
    /// commit like any workspace.
    pub fn open_merged(
        &self,
        owner: &AgentId,
        base: &SnapshotId,
        changes: &[(Path, Option<FileEntry>)],
    ) -> Result<Workspace> {
        self.store.tx(|tx| {
            let mut ws = self.open_in(tx, owner, base, None)?;
            for (path, file) in changes {
                let (kind, blob) = match file {
                    Some(f) => (f.kind.as_str(), Some(f.blob.to_string())),
                    None => ("delete", None),
                };
                tx.sql.execute(
                    "INSERT INTO overlay (workspace, path, kind, blob) VALUES (?1, ?2, ?3, ?4)",
                    params![ws.id.to_string(), path.as_str(), kind, blob],
                )?;
            }
            if !changes.is_empty() {
                ws.version = 1;
                ws.updated_at = Timestamp::now();
                tx.sql.execute(
                    "UPDATE workspaces SET version = 1, updated_at = ?2 WHERE id = ?1",
                    params![ws.id.to_string(), ws.updated_at.to_string()],
                )?;
            }
            Ok(ws)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "a\nb\nc\nd\ne\nf\n";

    #[test]
    fn disjoint_edits_merge() {
        let one = "A\nb\nc\nd\ne\nf\n";
        let two = "a\nb\nc\nd\ne\nF\n";
        assert_eq!(
            merge_text(BASE, &[one, two], MergeRule::Strict).unwrap(),
            "A\nb\nc\nd\ne\nF\n"
        );
    }

    #[test]
    fn the_same_edit_twice_is_taken_once() {
        let one = "a\nB\nc\nd\ne\nf\n";
        assert_eq!(
            merge_text(BASE, &[one, one], MergeRule::Strict).unwrap(),
            one
        );
    }

    #[test]
    fn adjacent_edits_conflict() {
        let one = "a\nB\nc\nd\ne\nf\n";
        let two = "a\nb\nC\nd\ne\nf\n";
        let conflicts = merge_text(BASE, &[one, two], MergeRule::Strict).unwrap_err();
        assert_eq!(conflicts.len(), 1);
        let c = &conflicts[0];
        assert_eq!(c.base, Lines { start: 1, end: 3 });
        assert_eq!(
            c.sides,
            vec![
                Side {
                    head: 0,
                    base: Lines { start: 1, end: 2 },
                    lines: Lines { start: 1, end: 2 },
                    text: vec!["B".into()],
                },
                Side {
                    head: 1,
                    base: Lines { start: 2, end: 3 },
                    lines: Lines { start: 2, end: 3 },
                    text: vec!["C".into()],
                },
            ]
        );
        assert_eq!(c.base.human(), "lines 2-3");
    }

    #[test]
    fn overlapping_edits_conflict_and_insertions_at_one_point_too() {
        let one = "a\nX\nd\ne\nf\n";
        let two = "a\nb\nY\nd\ne\nf\n";
        assert!(merge_text(BASE, &[one, two], MergeRule::Strict).is_err());
        assert!(merge_text(BASE, &[one, two], MergeRule::UnionInsertions).is_err());
        let three = "a\nb\nc\nnew\nd\ne\nf\n";
        let four = "a\nb\nc\nother\nd\ne\nf\n";
        let c = merge_text(BASE, &[three, four], MergeRule::Strict).unwrap_err();
        assert_eq!(c[0].base.human(), "before line 4");
    }

    #[test]
    fn union_takes_every_insertion_at_one_point_in_head_order() {
        let end = |t: &str| format!("{BASE}{t}");
        let (one, two, three) = (end("t1\nt1b\n"), end("t2\n"), end("t3\n"));
        assert_eq!(
            merge_text(BASE, &[&one, &two, &three], MergeRule::UnionInsertions).unwrap(),
            end("t1\nt1b\nt2\nt3\n")
        );
        assert_eq!(
            merge_text(BASE, &[&three, &one], MergeRule::UnionInsertions).unwrap(),
            end("t3\nt1\nt1b\n"),
            "the input order decides"
        );
        // Alike insertions once; an edit elsewhere merges beside them.
        let edit = "A\nb\nc\nd\ne\nf\nt2\n";
        assert_eq!(
            merge_text(BASE, &[&two, edit, &two], MergeRule::UnionInsertions).unwrap(),
            "A\nb\nc\nd\ne\nf\nt2\n"
        );
        let mid = "a\nb\nc\nnew\nd\ne\nf\n";
        let other = "a\nb\nc\nother\nd\ne\nf\n";
        assert_eq!(
            merge_text(BASE, &[mid, other], MergeRule::UnionInsertions).unwrap(),
            "a\nb\nc\nnew\nother\nd\ne\nf\n"
        );
    }

    #[test]
    fn union_still_conflicts_where_an_insertion_meets_a_change() {
        // One head inserts before line 4, another changes line 3 or 4.
        let insert = "a\nb\nc\nnew\nd\ne\nf\n";
        for change in ["a\nb\nC\nd\ne\nf\n", "a\nb\nc\nD\ne\nf\n"] {
            assert!(
                merge_text(BASE, &[insert, change], MergeRule::UnionInsertions).is_err(),
                "{change:?}"
            );
        }
        // An insertion beside a deletion at the same point.
        let delete = "a\nb\nc\ne\nf\n";
        assert!(merge_text(BASE, &[insert, delete], MergeRule::UnionInsertions).is_err());
    }

    #[test]
    fn a_side_keeps_its_first_lines_cut_short() {
        let long = "x".repeat(MAX_SIDE_CHARS + 5);
        let many: String = (0..10).map(|i| format!("n{i}\n")).collect();
        let one = format!("a\n{long}\nc\nd\ne\nf\n");
        let two = format!("a\n{many}c\nd\ne\nf\n");
        let c = merge_text(BASE, &[&one, &two], MergeRule::Strict).unwrap_err();
        let sides = &c[0].sides;
        assert_eq!(
            sides[0].text,
            vec![format!("{}…", "x".repeat(MAX_SIDE_CHARS))]
        );
        assert_eq!(sides[1].text.len(), MAX_HUNK_LINES);
        assert_eq!(sides[1].lines, Lines { start: 1, end: 11 });
        assert_eq!(sides[1].text[0], "n0");
    }

    #[test]
    fn hunks_are_in_base_coordinates() {
        let h = hunks(BASE, "a\nb\nC\nC2\nd\ne\n");
        assert_eq!(
            h,
            vec![
                Hunk {
                    base: Lines { start: 2, end: 3 },
                    head: Lines { start: 2, end: 4 }
                },
                Hunk {
                    base: Lines { start: 5, end: 6 },
                    head: Lines { start: 6, end: 6 }
                },
            ]
        );
    }
}
