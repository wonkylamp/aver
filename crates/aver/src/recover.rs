//! Recovery after an unclean stop (ARCHITECTURE.md §2.7.3). Every process
//! that opens a home's services records itself in `<home>/running/<pid>-<start>`
//! and removes the record when it ends. A record naming a dead process
//! means that process was killed (SIGKILL, power loss, a full disk), so
//! before anything else runs, [`on_open`] recovers for each dead owner,
//! touching only what no live process holds:
//!
//! - drops the stat cache (`mat/<ws>/stat`) of every workspace whose
//!   `mat/<ws>/lock` it can take: the cache is saved after the overlay
//!   commits, but the store runs `synchronous = NORMAL`, so after power loss
//!   it can name blobs the WAL lost, and a sync trusting it would skip files
//!   whose blobs no longer exist;
//! - kills the check runs left running in their own process groups
//!   (`sides/<repo16>/<slot>.pid`) in slots no live run holds, so none
//!   writes into a later run's report;
//! - empties `.aver/tmp` and removes Aver's temporary files under `.aver`
//!   in the directories of those same workspaces;
//! - rewrites a torn `.aver/workspace.toml` from its materialization row;
//! - lists the runs whose supervisor died mid-run.
//!
//! What it did goes to `<home>/recovery.json`, which `aver status` and
//! `aver doctor` show.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use aver_core::{AgentId, AttemptId, SnapshotId, TaskId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::disk::FileLock;
use crate::keys::{AgentKey, Home};
use crate::run::state::{process_start, same_process, RunState};
use crate::store::Store;
use crate::workspace::WorkspaceFile;

/// A record in `<home>/running`: a process using the home's services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Owner {
    pid: u32,
    /// [`process_start`], so a reused pid is never taken for the owner.
    #[serde(default)]
    start: Option<u64>,
}

/// What a recovery found and did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recovery {
    pub at: Option<Timestamp>,
    /// A process that stopped uncleanly (the first, when several did).
    pub pid: u32,
    /// Workspaces whose stat cache was dropped (their next sync reads
    /// every file).
    pub stat_caches: u32,
    /// Slots (`<repo16>/<slot>`) whose leftover run was killed.
    #[serde(default)]
    pub killed: Vec<String>,
    /// Temporary files removed from workspace directories.
    pub swept: u32,
    /// Workspace directories whose torn `.aver/workspace.toml` was rewritten.
    #[serde(default)]
    pub rewritten: Vec<PathBuf>,
    /// Runs whose supervisor died before they finished.
    #[serde(default)]
    pub interrupted: Vec<String>,
}

impl Recovery {
    /// One line for `aver status`.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} stat cache(s) dropped", self.stat_caches)];
        if !self.killed.is_empty() {
            parts.push(format!(
                "{} leftover check run(s) killed",
                self.killed.len()
            ));
        }
        if self.swept > 0 {
            parts.push(format!("{} temporary file(s) removed", self.swept));
        }
        if !self.rewritten.is_empty() {
            parts.push(format!(
                "{} workspace file(s) rewritten",
                self.rewritten.len()
            ));
        }
        if !self.interrupted.is_empty() {
            parts.push(format!(
                "run(s) {} interrupted (`aver status <run>`)",
                self.interrupted.join(", ")
            ));
        }
        format!(
            "the Aver process {} stopped uncleanly; recovered at {}: {}",
            self.pid,
            self.at.map(|t| t.to_string()).unwrap_or_default(),
            parts.join(", ")
        )
    }
}

fn running_dir(home: &Home) -> PathBuf {
    home.root().join("running")
}

pub fn recovery_path(home: &Home) -> PathBuf {
    home.root().join("recovery.json")
}

/// The last recovery of `home`, if there was one.
pub fn last(home: &Home) -> Option<Recovery> {
    serde_json::from_slice(&fs::read(recovery_path(home)).ok()?).ok()
}

/// The last recovery of `home` if it was within the last day: what `aver
/// status` mentions.
pub fn recent(home: &Home) -> Option<Recovery> {
    let r = last(home)?;
    let age = Timestamp::now().unix_millis() - r.at?.unix_millis();
    (age < 24 * 3600 * 1000).then_some(r)
}

/// This process's record in `<home>/running`; dropping it marks a clean
/// stop.
#[derive(Debug)]
pub struct Running {
    path: PathBuf,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Run by every process that opens the home's services: recovers from the
/// unclean stop of each recorded process that is dead, then records this
/// one. Recovery is best effort: a step that fails is skipped, never a
/// reason not to open.
pub fn on_open(home: &Home, store: &Store) -> io::Result<(Running, Option<Recovery>)> {
    let dir = running_dir(home);
    // Two processes opening at once would both recover one dead owner, and
    // the second, finding nothing left to do, would overwrite the first's
    // recovery record with an empty one.
    let _scan = FileLock::exclusive(&home.root().join("running.lock"))?;
    // Before 0.4 one owner at a time held the home, recorded in a file.
    if fs::symlink_metadata(&dir).is_ok_and(|m| m.is_file()) {
        let legacy = fs::read(&dir).ok();
        fs::remove_file(&dir)?;
        fs::create_dir_all(&dir)?;
        if let Some(bytes) = legacy {
            let owner = serde_json::from_slice(&bytes).unwrap_or(Owner {
                pid: 0,
                start: None,
            });
            let _ = fs::write(dir.join(record_name(&owner)), bytes);
        }
    }
    fs::create_dir_all(&dir)?;
    atomic::sweep(&dir);
    let me = Owner {
        pid: std::process::id(),
        start: process_start(std::process::id()),
    };
    let mut dead = Vec::new();
    for entry in fs::read_dir(&dir)?.flatten() {
        let path = entry.path();
        let owner = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Owner>(&b).ok())
            .unwrap_or(Owner {
                pid: 0,
                start: None,
            });
        if owner.pid != me.pid && !owner_alive(&owner) {
            dead.push((path, owner.pid));
        }
    }
    let recovery = match dead.first() {
        Some((_, pid)) => {
            let r = recover(home, store, *pid);
            tracing::warn!(summary = %r.summary(), "recovered from an unclean stop");
            let _ = atomic::write(
                &recovery_path(home),
                &serde_json::to_vec_pretty(&r).expect("recoveries serialize"),
                atomic::Mode::Keep,
            );
            Some(r)
        }
        None => None,
    };
    for (path, _) in &dead {
        let _ = fs::remove_file(path);
    }
    let path = dir.join(record_name(&me));
    atomic::write(
        &path,
        &serde_json::to_vec(&me).expect("owners serialize"),
        atomic::Mode::Keep,
    )?;
    Ok((Running { path }, recovery))
}

fn record_name(o: &Owner) -> String {
    format!("{}-{}", o.pid, o.start.unwrap_or(0))
}

/// Whether a process other than this one has the home's services open.
pub fn others_alive(home: &Home) -> bool {
    let me = std::process::id();
    fs::read_dir(running_dir(home))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| serde_json::from_slice::<Owner>(&fs::read(e.path()).ok()?).ok())
        .any(|o| o.pid != me && owner_alive(&o))
}

fn owner_alive(o: &Owner) -> bool {
    o.pid != 0 && same_process(o.pid, o.start)
}

fn recover(home: &Home, store: &Store, pid: u32) -> Recovery {
    let mut r = Recovery {
        at: Some(Timestamp::now()),
        pid,
        ..Recovery::default()
    };
    let mat = home.root().join("mat");
    let mut idle = Vec::new();
    for entry in fs::read_dir(&mat).into_iter().flatten().flatten() {
        // A live process syncing or restoring the workspace holds its lock.
        if let Ok(Some(lock)) = FileLock::try_exclusive(&entry.path().join("lock")) {
            idle.push((entry.file_name().to_string_lossy().into_owned(), Some(lock)));
        } else if !entry.path().join("lock").exists() {
            idle.push((entry.file_name().to_string_lossy().into_owned(), None));
        }
    }
    for (ws, _) in &idle {
        if fs::remove_file(mat.join(ws).join("stat")).is_ok() {
            r.stat_caches += 1;
        }
    }
    r.killed = crate::place::proc::kill_leftovers(&home.root().join("sides"));
    let idle_dirs = dirs_of(store, idle.iter().map(|(ws, _)| ws.as_str()));
    for dir in workspace_dirs(home) {
        if idle_dirs.contains(&fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())) {
            r.swept += sweep_meta(&dir);
        }
    }
    drop(idle);
    r.rewritten = rewrite_torn(home, store);
    r.interrupted = RunState::all(home)
        .into_iter()
        .filter(|run| run.finished_at.is_none() && !run.supervisor_alive())
        .map(|run| run.id)
        .collect();
    r
}

/// The directories of workspaces `ids`, from their materialization rows.
fn dirs_of<'a>(
    store: &Store,
    ids: impl Iterator<Item = &'a str>,
) -> std::collections::BTreeSet<PathBuf> {
    let ids: Vec<String> = ids.map(str::to_owned).collect();
    store
        .read(|conn| {
            let mut stmt = conn.prepare("SELECT workspace, dir FROM materializations")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|(ws, _)| ids.contains(ws))
        .map(|(_, dir)| fs::canonicalize(&dir).unwrap_or_else(|_| PathBuf::from(dir)))
        .collect()
}

/// `work/<repo>/<dir>`: every workspace directory under the home.
fn workspace_dirs(home: &Home) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for repo in fs::read_dir(home.root().join("work"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for dir in fs::read_dir(repo.path()).into_iter().flatten().flatten() {
            if dir.file_type().is_ok_and(|t| t.is_dir()) {
                out.push(dir.path());
            }
        }
    }
    out
}

/// Empties `dir/.aver/tmp` and removes Aver's temporary files under
/// `dir/.aver`, never following a link: how many entries went.
fn sweep_meta(dir: &Path) -> u32 {
    let meta = dir.join(".aver");
    if !fs::symlink_metadata(&meta).is_ok_and(|m| m.is_dir()) {
        return 0;
    }
    let mut swept = 0;
    let tmp = meta.join("tmp");
    if fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_dir()) {
        for entry in fs::read_dir(&tmp).into_iter().flatten().flatten() {
            let path = entry.path();
            let gone = match entry.file_type() {
                Ok(t) if t.is_dir() => fs::remove_dir_all(&path),
                _ => fs::remove_file(&path),
            };
            if gone.is_ok() {
                swept += 1;
            }
        }
    }
    for leftover in atomic::leftovers(&meta) {
        if fs::remove_file(leftover).is_ok() {
            swept += 1;
        }
    }
    swept
}

/// Rewrites each live workspace directory's `.aver/workspace.toml` that
/// does not parse, from the store's rows: the directories rewritten.
fn rewrite_torn(home: &Home, store: &Store) -> Vec<PathBuf> {
    let rows = store.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT m.workspace, m.dir, w.base, w.owner, w.attempt,
                    (SELECT task FROM attempts WHERE id = w.attempt)
             FROM materializations m JOIN workspaces w ON w.id = m.workspace
             WHERE m.state = 'live'",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (ws, dir, base, owner, attempt, task) in rows {
        let dir = PathBuf::from(dir);
        let path = dir.join(WorkspaceFile::PATH);
        let text = match fs::read_to_string(&path) {
            Ok(t) if toml::from_str::<WorkspaceFile>(&t).is_ok() => continue,
            Ok(t) => t,
            // A missing directory is the materialization's own state;
            // a missing file in a live directory is torn too.
            Err(_) if !dir.is_dir() => continue,
            Err(_) => String::new(),
        };
        let (Ok(workspace), Ok(base), Ok(owner)) = (
            ws.parse::<WorkspaceId>(),
            base.parse::<SnapshotId>(),
            owner.parse::<AgentId>(),
        ) else {
            continue;
        };
        let file = WorkspaceFile {
            workspace,
            home: fs::canonicalize(home.root()).unwrap_or_else(|_| home.root().to_owned()),
            base,
            owner,
            attempt: attempt.and_then(|a| a.parse::<AttemptId>().ok()),
            task: task.and_then(|t| t.parse::<TaskId>().ok()),
            key: salvaged(&text, "key").or_else(|| key_of(home, &owner)),
        };
        let mut body = String::from(
            "# Written by Aver for commands run in this directory (aver verify, diff, restore).\n",
        );
        body.push_str(&toml::to_string(&file).expect("workspace files serialize"));
        let written = atomic::open_dir(&dir.join(".aver")).and_then(|fd| {
            use std::os::fd::AsFd as _;
            atomic::write_in(fd.as_fd(), "workspace.toml", body.as_bytes())
        });
        if written.is_ok() {
            out.push(dir);
        }
    }
    out
}

/// A whole `name = "value"` line that survived in a torn file.
fn salvaged(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix(name)?.trim_start().strip_prefix('=')?;
        toml::from_str::<toml::Table>(&format!("v = {}", value.trim()))
            .ok()?
            .get("v")?
            .as_str()
            .map(str::to_owned)
    })
}

/// The name of the key in `home` whose agent is `owner`.
fn key_of(home: &Home, owner: &AgentId) -> Option<String> {
    fs::read_dir(home.keys_dir())
        .ok()?
        .flatten()
        .find_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_str()?.to_owned();
            (AgentKey::read(&path).ok()?.id() == *owner).then_some(name)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_torn_line_is_not_salvaged() {
        let text = "workspace = \"ws_1\"\nkey = \"agent-1\"\nhome = \"/tm";
        assert_eq!(salvaged(text, "key").as_deref(), Some("agent-1"));
        assert_eq!(salvaged(text, "home"), None);
        assert_eq!(salvaged(text, "missing"), None);
    }

    #[test]
    fn a_live_owner_is_not_unclean() {
        let me = Owner {
            pid: std::process::id(),
            start: process_start(std::process::id()),
        };
        assert!(owner_alive(&me));
        assert!(!owner_alive(&Owner {
            pid: me.pid,
            start: me.start.map(|s| s + 1),
        }));
        assert!(!owner_alive(&Owner {
            pid: 0,
            start: None
        }));
    }
}
