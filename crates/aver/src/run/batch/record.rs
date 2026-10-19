//! `batches/<id>/combine.json`: what the batch's last `aver combine` came
//! to, so the summary's hints never suggest the same failing combine again.
//! `combines/<commit>.json`: every combine Aver exported, by its commit, so
//! `aver rerun --onto <branch>` builds on the snapshot the combine verified.

use std::fs;
use std::io;

use std::path::PathBuf;

use aver_core::{ReceiptId, SnapshotId, Timestamp, Verdict};
use serde::{Deserialize, Serialize};

use super::batch_dir;
use crate::keys::Home;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombineStatus {
    /// Every task merged, passed and was exported.
    Exported,
    /// A compatible subset was exported; the rest are in `excluded`.
    Partial,
    /// The heads conflict; nothing exported.
    Conflicts,
    /// Merged, but not every task passes on the result; nothing exported.
    NotPass,
}

/// Why a task was left out of a combine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExcludedWhy {
    /// Its change conflicts with task `with`'s at `paths`.
    Conflict { with: u32, paths: Vec<String> },
    /// It does not pass on the merged result.
    NotPass {
        verdict: Verdict,
        receipt: ReceiptId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Excluded {
    pub number: u32,
    pub run: String,
    pub why: ExcludedWhy,
}

/// One `aver combine` over a batch's tasks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CombineRecord {
    pub at: Timestamp,
    pub status: CombineStatus,
    /// The task numbers in the result.
    pub included: Vec<u32>,
    pub excluded: Vec<Excluded>,
    /// The exported branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

fn path(home: &Home, batch: &str) -> std::path::PathBuf {
    batch_dir(home, batch).join("combine.json")
}

/// Records `record` as the batch's last combine.
pub fn save(home: &Home, batch: &str, record: &CombineRecord) -> io::Result<()> {
    let path = path(home, batch);
    fs::create_dir_all(path.parent().expect("combine.json has a directory"))?;
    let bytes = serde_json::to_vec_pretty(record).expect("combine records serialize");
    crate::atomic::write(&path, &bytes, crate::atomic::Mode::Keep)
}

/// The batch's last combine, if any.
pub fn last(home: &Home, batch: &str) -> Option<CombineRecord> {
    serde_json::from_slice(&fs::read(path(home, batch)).ok()?).ok()
}

/// A combine Aver verified and exported (`combines/<commit>.json`, a host
/// file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportedCombine {
    /// The exported commit.
    pub commit: String,
    /// The snapshot every task was verified on, which the commit holds.
    pub snapshot: SnapshotId,
    pub branch: String,
    pub repo: PathBuf,
    /// The runs whose tasks the result holds, prerequisites included.
    pub runs: Vec<String>,
    pub at: Timestamp,
}

fn exported_path(home: &Home, commit: &str) -> Option<PathBuf> {
    (!commit.is_empty() && commit.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| home.root().join("combines").join(format!("{commit}.json")))
}

/// Records `combine` under its commit.
pub fn save_exported(home: &Home, combine: &ExportedCombine) -> io::Result<()> {
    let path = exported_path(home, &combine.commit).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{:?} is not a commit id", combine.commit),
        )
    })?;
    fs::create_dir_all(path.parent().expect("combines/ is a directory"))?;
    let bytes = serde_json::to_vec_pretty(combine).expect("combine records serialize");
    crate::atomic::write(&path, &bytes, crate::atomic::Mode::Keep)
}

/// The combine Aver exported as `commit`, if any.
pub fn exported(home: &Home, commit: &str) -> Option<ExportedCombine> {
    serde_json::from_slice(&fs::read(exported_path(home, commit)?).ok()?).ok()
}

/// The newest combine Aver exported to `branch` of `repo`.
pub fn exported_to(home: &Home, repo: &std::path::Path, branch: &str) -> Option<ExportedCombine> {
    fs::read_dir(home.root().join("combines"))
        .ok()?
        .flatten()
        .filter_map(|e| serde_json::from_slice::<ExportedCombine>(&fs::read(e.path()).ok()?).ok())
        .filter(|c| c.repo == repo && c.branch == branch)
        .max_by_key(|c| c.at)
}
