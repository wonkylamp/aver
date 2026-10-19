//! One run per hook event. The Claude Code plugin and a repository's
//! settings may both install `aver hook <name>` for the same event, and
//! Claude Code runs both, in parallel, with the same input. The first to
//! claim the event runs; any other exits 0 having done nothing, except a
//! second `worktree-create`, which prints the first one's directory, since
//! Claude Code compares the paths every WorktreeCreate hook prints.
//!
//! A claim is a file under `$AVER_HOME/tmp/hook-claims/`, named by a hash
//! of the hook's name and its raw input, read and written under an
//! exclusive lock. Claude Code's hook inputs carry no field unique to one
//! firing (a Stop's input is its session, transcript, last message and
//! `stop_hook_active`), so the same input may fire again later as a new
//! event: a claim holds while its process runs and for [`GRACE`] after it
//! finishes, and is then taken by whoever comes next. A claim whose process
//! died unfinished, or whose run failed (other than `worktree-create`'s,
//! which its duplicate must report), is taken at once. Claims older than [`SWEEP_AFTER`] are
//! deleted.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aver::keys::Home;
use aver::run::state::{process_start, same_process};
use serde::{Deserialize, Serialize};

/// How long a finished run still answers for its event: longer than the
/// gap between two hooks Claude Code starts for one event, shorter than
/// the gap between two events with the same input.
pub(super) const GRACE: Duration = Duration::from_secs(5);
/// Claims older than this are deleted; longer than any hook's timeout.
const SWEEP_AFTER: Duration = Duration::from_secs(3600);
/// How often a waiting `worktree-create` looks at the first run's claim.
const POLL: Duration = Duration::from_millis(100);
/// The claims' directory under the home, as tests find it.
pub(super) const DIR: &str = "tmp/hook-claims";

/// What a claim file holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Record {
    pid: u32,
    #[serde(default)]
    start: Option<u64>,
    /// When it finished, in milliseconds since the epoch.
    #[serde(default)]
    done_ms: Option<u64>,
    /// What a finished `worktree-create` printed last: the directory.
    #[serde(default)]
    out: Option<String>,
}

/// What this call does about its event.
pub(super) enum Outcome {
    /// It runs the hook, then calls [`Claim::finish`].
    Run(Claim),
    /// Another run has it, or just had it: do nothing.
    Skip,
    /// Another run finished it and printed this directory.
    Reuse(String),
    /// Another run finished it without a directory.
    Failed,
}

/// The claim this process holds.
pub(super) struct Claim {
    path: PathBuf,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The claim file of hook `name` with input `input`.
fn path_for(home: &Home, name: &str, input: &str) -> PathBuf {
    let mut h = blake3::Hasher::new();
    h.update(name.as_bytes());
    h.update(b"\0");
    h.update(input.as_bytes());
    home.root()
        .join(DIR)
        .join(&h.finalize().to_hex().as_str()[..32])
}

/// Runs `f` on the record at `path` under its exclusive lock, storing what
/// `f` returns, if anything.
fn locked<T>(
    path: &Path,
    f: impl FnOnce(Option<Record>) -> (Option<Record>, T),
) -> std::io::Result<T> {
    let mut file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.lock()?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let (write, out) = f(serde_json::from_str(&text).ok());
    if let Some(record) = write {
        file.set_len(0)?;
        file.rewind()?;
        file.write_all(&serde_json::to_vec(&record).expect("records serialize"))?;
    }
    Ok(out)
}

/// Deletes claims nobody has touched for [`SWEEP_AFTER`].
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > SWEEP_AFTER);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

enum Seen {
    Free,
    Running,
    Finished(Option<String>),
}

fn seen(record: Option<&Record>) -> Seen {
    match record {
        None => Seen::Free,
        Some(r) => match r.done_ms {
            Some(done) if now_ms().saturating_sub(done) < GRACE.as_millis() as u64 => {
                Seen::Finished(r.out.clone())
            }
            Some(_) => Seen::Free,
            None if same_process(r.pid, r.start) => Seen::Running,
            None => Seen::Free,
        },
    }
}

/// Claims the event of hook `name` with input `input`. With `wait`, a run
/// that finds another one still running waits for it to finish.
pub(super) fn claim(home: &Home, name: &str, input: &str, wait: bool) -> std::io::Result<Outcome> {
    let dir = home.root().join(DIR);
    std::fs::create_dir_all(&dir)?;
    sweep(&dir);
    let path = path_for(home, name, input);
    let pid = std::process::id();
    let mine = Record {
        pid,
        start: process_start(pid),
        ..Record::default()
    };
    loop {
        let outcome = locked(&path, |record| match seen(record.as_ref()) {
            Seen::Free => (
                Some(mine.clone()),
                Some(Outcome::Run(Claim { path: path.clone() })),
            ),
            Seen::Running if wait => (None, None),
            Seen::Running => (None, Some(Outcome::Skip)),
            Seen::Finished(_) if !wait => (None, Some(Outcome::Skip)),
            Seen::Finished(Some(out)) => (None, Some(Outcome::Reuse(out))),
            Seen::Finished(None) => (None, Some(Outcome::Failed)),
        })?;
        match outcome {
            Some(o) => return Ok(o),
            None => std::thread::sleep(POLL),
        }
    }
}

impl Claim {
    /// Marks the event finished; `out` is what a `worktree-create` printed
    /// last, `None` when it failed or printed nothing.
    pub(super) fn finish(self, out: Option<String>) {
        let _ = locked(&self.path, |record| {
            let mut r = record.unwrap_or_default();
            r.done_ms = Some(now_ms());
            r.out = out;
            (Some(r), ())
        });
    }

    /// Gives the event up: the next run with the same input runs again
    /// instead of taking this failure for its own (a `worktree-remove`
    /// refused for unfetched commits, retried once they are fetched).
    pub(super) fn release(self) {
        let _ = locked(&self.path, |_| {
            let free = Record {
                done_ms: Some(0),
                ..Record::default()
            };
            (Some(free), ())
        });
    }
}
