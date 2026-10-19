//! Micro-checkpoints (ARCHITECTURE.md §2.3.1): every `aver check` and
//! `aver verify` in a workspace directory appends the snapshot it judged,
//! its verdict and its test delta against the base to
//! `mat/<ws>/checkpoints.jsonl`, and `aver restore --last-green` puts the
//! directory back to the newest one with no NEW failure.
//!
//! A checkpoint names a stored snapshot, so it costs one line: the files
//! are already in the store from the sync that made the snapshot.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::Path as FsPath;

use aver_core::{
    BaseStatus, Cause, HeadStatus, ReceiptBody, SnapshotId, Timestamp, Verdict, WorkspaceId,
};
use serde::{Deserialize, Serialize};

use super::{FileEntry, Result, WorkspaceError, Workspaces};
use crate::disk::FileLock;

const LOG: &str = "checkpoints.jsonl";

/// Lines kept when the log is trimmed; it is trimmed once it holds twice
/// as many.
const KEPT: usize = 256;

/// One `aver check` or `aver verify` of a workspace directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub at: Timestamp,
    /// The base the workspace was on: a checkpoint from before a rebase is
    /// never restored onto the new base.
    pub base: SnapshotId,
    /// The directory's synced files.
    pub snapshot: SnapshotId,
    pub verdict: Verdict,
    /// Tests passing with the change minus tests passing on the base.
    pub passing: i64,
    /// Entries whose failure is the change's own.
    pub new: u32,
}

impl Checkpoint {
    pub fn of(body: &ReceiptBody) -> Self {
        let tests = body
            .checks
            .iter()
            .filter(|c| crate::run::report::is_test(c) && c.cause != Some(Cause::Env));
        let (mut head, mut base) = (0i64, 0i64);
        for c in tests {
            head += i64::from(c.head == HeadStatus::Pass);
            base += i64::from(c.base == BaseStatus::Pass);
        }
        Self {
            at: Timestamp::now(),
            base: body.base_snapshot,
            snapshot: body.head_snapshot,
            verdict: body.verdict,
            passing: head - base,
            new: body
                .checks
                .iter()
                .filter(|c| c.cause == Some(Cause::New))
                .count() as u32,
        }
    }

    /// Nothing the change broke: no NEW failure.
    pub fn green(&self) -> bool {
        self.new == 0 && self.verdict != Verdict::FailOwn
    }

    /// `+2 passing, 0 NEW`.
    pub fn delta(&self) -> String {
        format!("{:+} passing, {} NEW", self.passing, self.new)
    }
}

/// What `aver restore --last-green` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastGreen {
    pub checkpoint: Checkpoint,
    /// Paths written or removed; empty when the directory already held it.
    pub written: Vec<String>,
}

impl Workspaces {
    /// Appends the checkpoint of a check or verify of workspace `id`'s
    /// directory. A failure is logged, never returned: the verdict the
    /// caller prints matters more than the checkpoint.
    pub fn checkpoint(&self, id: &WorkspaceId, body: &ReceiptBody) {
        let dir = self.disk.mat_dir(id);
        if !dir.is_dir() {
            return;
        }
        if let Err(e) = append(&dir, &Checkpoint::of(body)) {
            tracing::warn!(error = %e, "recording a checkpoint failed");
        }
    }

    /// The checkpoints of workspace `id`, oldest first.
    pub fn checkpoints(&self, id: &WorkspaceId) -> Result<Vec<Checkpoint>> {
        read(&self.disk.mat_dir(id).join(LOG))
    }

    /// Puts the directory of workspace `id` back to its newest green
    /// checkpoint on its current base, writing only the files that differ
    /// and logging what it replaces for `aver restore --undo`.
    pub fn restore_last_green(
        &self,
        caller: &aver_core::AgentId,
        id: &WorkspaceId,
    ) -> Result<LastGreen> {
        let (ws, m) = self.live_dir(caller, id)?;
        let checkpoint = self
            .checkpoints(id)?
            .into_iter()
            .rev()
            .find(|c| c.green() && c.base == ws.base)
            .ok_or_else(|| {
                WorkspaceError::Invalid(
                    "no green checkpoint on this workspace's base: no `aver check` or `aver verify` here has passed without a NEW failure yet".into(),
                )
            })?;
        let head = self.snapshot(caller, id)?;
        if head == checkpoint.snapshot {
            return Ok(LastGreen {
                checkpoint,
                written: Vec::new(),
            });
        }
        let now: BTreeMap<String, FileEntry> = self.snapshot_files(&head)?.into_iter().collect();
        let then: BTreeMap<String, FileEntry> = self
            .snapshot_files(&checkpoint.snapshot)?
            .into_iter()
            .collect();
        // Removals first: a removed file may stand where the checkpoint has
        // a directory.
        let mut steps: Vec<(String, Option<FileEntry>)> = now
            .keys()
            .filter(|p| !then.contains_key(*p))
            .map(|p| (p.clone(), None))
            .collect();
        steps.extend(
            then.iter()
                .filter(|(p, f)| now.get(*p) != Some(f))
                .map(|(p, f)| (p.clone(), Some(*f))),
        );
        let _lock = FileLock::exclusive(&self.disk.mat_dir(id).join("lock"))?;
        let written = self.apply_restore(&m, &steps)?;
        Ok(LastGreen {
            checkpoint,
            written,
        })
    }
}

fn append(dir: &FsPath, checkpoint: &Checkpoint) -> Result<()> {
    let path = dir.join(LOG);
    let mut line = serde_json::to_vec(checkpoint).expect("checkpoints serialize");
    line.push(b'\n');
    // One write of a short line to an `O_APPEND` file: concurrent appends
    // never interleave, and a torn last line is skipped on read.
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(&line)?;
    if file.metadata()?.len() as usize > 2 * KEPT * line.len() {
        let _lock = FileLock::exclusive(&dir.join("lock"))?;
        let text = fs::read_to_string(&path)?;
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() > 2 * KEPT {
            let kept: String = lines[lines.len() - KEPT..]
                .iter()
                .map(|l| format!("{l}\n"))
                .collect();
            crate::atomic::write(&path, kept.as_bytes(), crate::atomic::Mode::Keep)?;
        }
    }
    Ok(())
}

fn read(path: &FsPath) -> Result<Vec<Checkpoint>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}
