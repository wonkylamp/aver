//! Git bridge (ARCHITECTURE.md §2.2).
//!
//! [`import()`] turns one Git commit into a snapshot plus its signed lineage
//! and ledger entry, in a single store transaction, after validating every
//! path and secret-scanning every blob. [`export()`] turns a snapshot's
//! transition chain back into Git commits on a branch of a target repository.
//!
//! Git is driven only through its plumbing (`rev-parse`, `ls-tree`,
//! `cat-file`, `fast-import`, `update-ref`); no working directory is ever
//! created, and URLs are cloned with `--bare`. Tree kinds map one-to-one onto
//! Git modes, so an imported commit exports to the identical tree SHA.
//!
//! The `git_map` table memoizes Aver id to Git SHA for blobs, trees and
//! commits; commits are keyed by the transition that records them.

mod export;
mod import;
pub(crate) mod local;

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path as FsPath, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use aver_core::{GitSha, Node, SnapshotId, Transition, TransitionId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::store::{Store, StoreError, Tx};

pub use export::{export, export_with, ExportError, ExportOptions, ExportReport, Identity};
pub use import::{
    import, redact_url, resolve_base, secrets_warning, BaseResolution, ImportError, ImportOptions,
    ImportReport, OversizePolicy, Placeholder, RefPrecondition, RefUpdate, SecretPolicy, Violation,
    ViolationReason,
};
pub(crate) use import::{read_blobs_whole, tracked_files};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("cannot run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("`git {command}` failed: {stderr}")]
    Failed { command: String, stderr: String },
    #[error("unexpected output from `git {command}`: {detail}")]
    Protocol { command: String, detail: String },
}

/// A repository addressed by its directory, bare or not.
#[derive(Debug, Clone)]
pub(crate) struct Repo {
    dir: PathBuf,
    /// The parent of `dir`, so Git never walks up from a directory that is
    /// not itself a repository and acts on an enclosing one instead.
    ceiling: Option<PathBuf>,
}

impl Repo {
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let ceiling = std::path::absolute(&dir)
            .ok()
            .and_then(|abs| abs.parent().map(FsPath::to_path_buf));
        Self { dir, ceiling }
    }

    pub(crate) fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = git_command();
        if let Some(ceiling) = &self.ceiling {
            cmd.env("GIT_CEILING_DIRECTORIES", ceiling);
        }
        cmd.arg("-C").arg(&self.dir).args(args);
        cmd
    }

    /// Runs git to completion and returns its stdout, failing on a non-zero exit.
    pub(crate) fn run<I, S>(&self, args: I) -> Result<Vec<u8>, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        run(self.command(args))
    }

    /// Like [`Repo::run`], but returns the exit success instead of failing.
    pub(crate) fn status<I, S>(&self, args: I) -> Result<(bool, Vec<u8>), GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.command(args).stderr(Stdio::null()).output()?;
        Ok((out.status.success(), out.stdout))
    }
}

/// The user's Git identity as `repo` resolves it (`git config user.name`
/// and `user.email`, local over global). `Err` says how to set it.
pub fn user_identity(repo: &FsPath) -> Result<Identity, String> {
    let get = |key: &str| {
        Repo::new(repo)
            .run(["config", "--get", key])
            .ok()
            .map(|out| String::from_utf8_lossy(&out).trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    match (get("user.name"), get("user.email")) {
        (Some(name), Some(email)) => Identity::new(&name, &email),
        _ => Err(format!(
            "Aver commits as your Git identity, and {} has none set: run `git config user.name \"<your name>\"` and `git config user.email <your email>` there (or with --global), then retry",
            repo.display()
        )),
    }
}

/// A git command isolated from the caller's repository environment.
pub(crate) fn git_command() -> Command {
    let mut cmd = Command::new("git");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
    ] {
        cmd.env_remove(var);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("LC_ALL", "C");
    cmd
}

pub(crate) fn run(mut cmd: Command) -> Result<Vec<u8>, GitError> {
    let out = cmd.stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(GitError::Failed {
            command: describe(&cmd),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    Ok(out.stdout)
}

fn describe(cmd: &Command) -> String {
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    // Skip the leading `-C <dir>`.
    let skip = if args.first().map(String::as_str) == Some("-C") {
        2
    } else {
        0
    };
    args[skip.min(args.len())..].join(" ")
}

pub(crate) fn parse_sha(command: &str, bytes: &[u8]) -> Result<GitSha, GitError> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| GitError::Protocol {
            command: command.to_owned(),
            detail: format!("expected a sha, got {:?}", String::from_utf8_lossy(bytes)),
        })
}

/// Git's file mode for a tree entry.
pub(crate) fn mode(node: &Node) -> &'static str {
    match node {
        Node::File(_) => "100644",
        Node::Exec(_) => "100755",
        Node::Symlink(_) => "120000",
        Node::Tree(_) => "040000",
    }
}

/// Answers "does the repository have this object?" through one long-lived
/// `git cat-file --batch-check`, or without asking for a repository known
/// to be empty.
pub(crate) struct Probe(Option<ProbeProcess>);

struct ProbeProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    line: String,
}

impl Probe {
    pub(crate) fn open(repo: &Repo) -> Result<Self, GitError> {
        let mut child = repo
            .command(["cat-file", "--batch-check"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = BufWriter::new(child.stdin.take().expect("piped"));
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(Self(Some(ProbeProcess {
            child,
            stdin,
            stdout,
            line: String::new(),
        })))
    }

    pub(crate) fn empty() -> Self {
        Self(None)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub(crate) fn has(&mut self, sha: &GitSha) -> Result<bool, GitError> {
        let Some(p) = &mut self.0 else {
            return Ok(false);
        };
        writeln!(p.stdin, "{sha}")?;
        p.stdin.flush()?;
        p.line.clear();
        if p.stdout.read_line(&mut p.line)? == 0 {
            return Err(GitError::Protocol {
                command: "cat-file --batch-check".into(),
                detail: "exited early".into(),
            });
        }
        Ok(!p.line.trim_end().ends_with(" missing"))
    }
}

impl Drop for ProbeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A scratch directory under `$AVER_HOME/tmp` (§3), removed on drop.
pub(crate) struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Creates `tmp/<prefix>-<ulid>` next to the store's database.
    pub(crate) fn new(store: &Store, prefix: &str) -> std::io::Result<Self> {
        let base = store
            .path()
            .parent()
            .unwrap_or(FsPath::new("."))
            .join("tmp");
        std::fs::create_dir_all(&base)?;
        let dir = base.join(format!("{prefix}-{}", ulid::Ulid::generate()));
        std::fs::create_dir(&dir)?;
        Ok(Self(dir))
    }

    pub(crate) fn path(&self) -> &FsPath {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Stores a transition and its lineage index row. Every writer of transitions
/// goes through here so `transitions_by_snapshot` cannot miss one.
pub fn record_transition(tx: &Tx<'_>, transition: &Transition) -> Result<TransitionId, StoreError> {
    let id = tx.put(transition)?;
    tx.sql.execute(
        "INSERT OR IGNORE INTO transitions_by_snapshot (snapshot, transition) VALUES (?1, ?2)",
        params![transition.body.to.to_string(), id.to_string()],
    )?;
    Ok(id)
}

pub(crate) fn map_insert(
    conn: &Connection,
    aver_id: &str,
    sha: &GitSha,
    kind: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR IGNORE INTO git_map (aver_id, git_sha1, kind) VALUES (?1, ?2, ?3)",
        params![aver_id, sha.as_str(), kind],
    )?;
    Ok(())
}

pub(crate) fn map_get(conn: &Connection, aver_id: &str) -> Result<Option<GitSha>, StoreError> {
    let sha: Option<String> = conn
        .query_row(
            "SELECT git_sha1 FROM git_map WHERE aver_id = ?1",
            [aver_id],
            |row| row.get(0),
        )
        .optional()?;
    sha.map(|s| {
        s.parse().map_err(|_| StoreError::Corrupt {
            id: format!("git_map {aver_id}"),
            reason: format!("bad sha {s:?}"),
        })
    })
    .transpose()
}

/// The Git commit `snapshot` was first imported from (or exported to), if
/// any: the commit its earliest transition maps to.
pub fn imported_commit(store: &Store, snapshot: &SnapshotId) -> Result<Option<GitSha>, StoreError> {
    store.read(|conn| match earliest_transition(conn, snapshot)? {
        Some(t) => map_get(conn, &t.to_string()),
        None => Ok(None),
    })
}

/// The Git blob SHA-1s that import recorded for `blobs`; a blob Git never
/// held is left out.
pub fn blob_shas<'a>(
    store: &Store,
    blobs: impl IntoIterator<Item = &'a aver_core::BlobId>,
) -> Result<std::collections::HashMap<aver_core::BlobId, GitSha>, StoreError> {
    store.read(|conn| {
        let mut stmt = conn
            .prepare_cached("SELECT git_sha1 FROM git_map WHERE aver_id = ?1 AND kind = 'blob'")?;
        let mut out = std::collections::HashMap::new();
        for blob in blobs {
            let sha: Option<String> = stmt
                .query_row([blob.to_string()], |row| row.get(0))
                .optional()?;
            if let Some(sha) = sha.and_then(|s| s.parse().ok()) {
                out.insert(*blob, sha);
            }
        }
        Ok(out)
    })
}

/// Every transition recorded for `snapshot`, earliest first.
pub(crate) fn transitions_for_snapshot(
    conn: &Connection,
    snapshot: &SnapshotId,
) -> Result<Vec<TransitionId>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT transition FROM transitions_by_snapshot WHERE snapshot = ?1 ORDER BY rowid",
    )?;
    let rows = stmt.query_map([snapshot.to_string()], |row| row.get::<_, String>(0))?;
    rows.map(|row| parse_transition(snapshot, row?)).collect()
}

fn parse_transition(snapshot: &SnapshotId, id: String) -> Result<TransitionId, StoreError> {
    id.parse().map_err(|_| StoreError::Corrupt {
        id: format!("transitions_by_snapshot {snapshot}"),
        reason: format!("bad transition id {id:?}"),
    })
}

/// The first transition recorded for `snapshot`.
pub(crate) fn earliest_transition(
    conn: &Connection,
    snapshot: &SnapshotId,
) -> Result<Option<TransitionId>, StoreError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT transition FROM transitions_by_snapshot WHERE snapshot = ?1
             ORDER BY rowid LIMIT 1",
            [snapshot.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    id.map(|id| parse_transition(snapshot, id)).transpose()
}
