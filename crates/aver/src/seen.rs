//! The notices a workspace has already shown (`.aver/notices-seen.json`,
//! ARCHITECTURE.md §2.11), so verify and `aver check` print only new or
//! changed overlaps.
//!
//! Every item of a workspace's notices has a [`NoticeKey`]: the other task,
//! the path, the overlap kind and the other side's version (or, for a path
//! changed under another task's lease, the lease). The workspace keeps the
//! keys it has shown. `aver verify` and `aver check` share that set: each
//! prints only items whose key it does not hold, plus a count of the
//! unchanged ones; `aver notices` prints everything.
//!
//! An item the other side recorded a change to within [`SETTLE_MS`] may
//! change again: a report shows it marked "still changing".

use std::collections::BTreeSet;
use std::path::Path;

use aver_core::Timestamp;
use serde::{Deserialize, Serialize};

use crate::coord::{NoticeKey, Notices};
use crate::workspace::WorkspaceFile;

/// Where the seen-set lives in a workspace directory.
pub const PATH: &str = ".aver/notices-seen.json";
/// How long after the other side's last recorded change an item counts as
/// still changing.
pub const SETTLE_MS: i64 = 60_000;

#[derive(Debug, Default, Serialize, Deserialize)]
struct SeenFile {
    keys: BTreeSet<NoticeKey>,
}

fn read(root: &Path) -> BTreeSet<NoticeKey> {
    std::fs::read(root.join(PATH))
        .ok()
        .and_then(|b| serde_json::from_slice::<SeenFile>(&b).ok())
        .map(|f| f.keys)
        .unwrap_or_default()
}

/// The part of `n` that workspace `root` has not been shown: new or
/// changed overlaps and leased paths, with `unchanged` counting the rest.
/// At `now`, an item the other side changed within [`SETTLE_MS`] is
/// marked `changing`. Reads the seen-set and writes nothing. What only
/// `aver notices` lists (other tasks' changes and leases elsewhere) is
/// dropped.
pub fn delta(root: &Path, n: &Notices, now: Timestamp) -> Notices {
    let seen = read(root);
    let mut out = Notices::default();
    for notice in &n.notices {
        if seen.contains(&notice.key()) {
            out.unchanged += 1;
            continue;
        }
        let changing = notice
            .changed_at
            .is_some_and(|at| now.unix_millis() - at.unix_millis() < SETTLE_MS);
        let mut notice = notice.clone();
        notice.changing = changing;
        out.notices.push(notice);
    }
    for leased in &n.leased {
        if seen.contains(&leased.key()) {
            out.unchanged += 1;
        } else {
            out.leased.push(leased.clone());
        }
    }
    out
}

/// Records that `shown` were shown in `root`. Keys of items no longer in
/// `current` are dropped, so an overlap that goes away and comes back is
/// told again.
pub fn record(root: &Path, current: &Notices, shown: &[NoticeKey]) {
    let live: BTreeSet<NoticeKey> = current
        .notices
        .iter()
        .map(|n| n.key())
        .chain(current.leased.iter().map(|l| l.key()))
        .collect();
    let mut keys: BTreeSet<NoticeKey> = read(root).intersection(&live).cloned().collect();
    keys.extend(shown.iter().filter(|k| live.contains(*k)).cloned());
    if let Ok(json) = serde_json::to_vec(&SeenFile { keys }) {
        let _ = WorkspaceFile::write_meta(root, PATH, json);
    }
}

/// For a report: the notices to put in it ([`delta`]) after recording them
/// as shown, as the report renders them.
pub fn for_report(root: &Path, n: Option<&Notices>, now: Timestamp) -> Option<Notices> {
    let n = n?;
    let d = delta(root, n, now);
    record(root, n, &crate::coord::render(&d, false).shown);
    Some(d)
}
