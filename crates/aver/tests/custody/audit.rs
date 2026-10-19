//! The custody invariants (custody plan §1.1), checked against a world
//! after a fault and again after the operation is run once more.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use aver_core::{Receipt, ReceiptId, Verdict};

use super::world::{git, text, World};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Inv {
    /// The user's repository is untouched but for what the operation names.
    I1,
    /// No exported commit without its durable PASS or REVIEW receipt.
    I2,
    /// No branch that no run, batch, combine or decided attempt names.
    I3,
    /// Workspace directories and Aver's host files are whole.
    I4,
    /// The store passes its integrity check.
    I5,
    /// `status`, `workspace ls` and `doctor` answer without a panic.
    I6,
}

/// The user's repository as I1 compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoState {
    pub head: String,
    pub branch: String,
    pub status: String,
    pub config: String,
    pub refs: BTreeMap<String, String>,
    /// Working tree files and their contents (outside `.git`).
    pub files: BTreeMap<String, Vec<u8>>,
}

fn files(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path == root.join(".git") {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            files(root, &path, out);
        } else {
            let rel = path.strip_prefix(root).unwrap().display().to_string();
            out.insert(rel, fs::read(&path).unwrap_or_default());
        }
    }
}

impl RepoState {
    pub fn of(repo: &Path) -> Self {
        let refs = git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"])
            .lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(r, s)| (r.to_owned(), s.to_owned()))
            .collect();
        let mut tree = BTreeMap::new();
        files(repo, repo, &mut tree);
        Self {
            head: git(repo, &["rev-parse", "HEAD"]),
            branch: git(repo, &["symbolic-ref", "--quiet", "HEAD"]),
            status: git(repo, &["status", "--porcelain=v1", "--untracked-files=all"]),
            config: fs::read_to_string(repo.join(".git/config")).unwrap(),
            refs,
            files: tree,
        }
    }
}

/// What an operation may change, for I1.
#[derive(Debug, Clone, Default)]
pub struct Allowed {
    /// Working-tree paths it may write or remove.
    pub paths: Vec<&'static str>,
    /// Branch name prefixes it may create (`aver/`).
    pub branches: Vec<&'static str>,
    /// It pushes those branches to `origin` (`--pr`): their remote refs
    /// and upstream settings may appear.
    pub pushes: bool,
}

/// `config` without the `[branch "…"]` sections of allowed branches.
fn without_upstreams(config: &str, allowed: &Allowed) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in config.lines() {
        if line.starts_with('[') {
            skip = line
                .strip_prefix("[branch \"")
                .is_some_and(|b| allowed.branches.iter().any(|p| b.starts_with(p)));
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The invariants that do not hold in `w` (relative to the repository
/// as it was, `before`), each with what broke.
pub fn audit(w: &World, before: &RepoState, allowed: &Allowed) -> BTreeMap<Inv, Vec<String>> {
    let mut broken: BTreeMap<Inv, Vec<String>> = BTreeMap::new();
    let mut fail = |inv: Inv, why: String| broken.entry(inv).or_default().push(why);
    let now = RepoState::of(&w.repo);

    // I1.
    let (config_before, config_now) = if allowed.pushes {
        (
            without_upstreams(&before.config, allowed),
            without_upstreams(&now.config, allowed),
        )
    } else {
        (before.config.clone(), now.config.clone())
    };
    for (what, a, b) in [
        ("HEAD", &before.head, &now.head),
        ("the current branch", &before.branch, &now.branch),
        ("the Git config", &config_before, &config_now),
    ] {
        if a != b {
            fail(Inv::I1, format!("{what} changed: {a:?} -> {b:?}"));
        }
    }
    let paths: BTreeSet<&String> = before.files.keys().chain(now.files.keys()).collect();
    for path in paths {
        let changed = before.files.get(path) != now.files.get(path);
        if changed && !allowed.paths.iter().any(|p| p == path) {
            fail(Inv::I1, format!("{path} changed"));
        }
    }
    let new_refs: Vec<&String> = now
        .refs
        .keys()
        .filter(|r| before.refs.get(*r) != now.refs.get(*r))
        .collect();
    let mut new_branches = Vec::new();
    for r in &new_refs {
        let pushed = r
            .strip_prefix("refs/remotes/origin/")
            .is_some_and(|b| allowed.pushes && allowed.branches.iter().any(|p| b.starts_with(p)));
        match r.strip_prefix("refs/heads/") {
            Some(b) if allowed.branches.iter().any(|p| b.starts_with(p)) => {
                new_branches.push((b.to_owned(), now.refs[*r].clone()))
            }
            _ if pushed => {}
            _ => fail(Inv::I1, format!("ref {r} appeared or moved")),
        }
    }
    for leftover in aver::atomic::leftovers(&w.repo) {
        fail(Inv::I1, format!("temporary file {}", leftover.display()));
    }

    // I2 and I3 read the store; a missing one has no receipts.
    let store = aver::store::Store::open(w.home.join("aver.db")).ok();
    let conn = rusqlite::Connection::open(w.home.join("aver.db")).ok();
    let named = named_branches(w);
    for (branch, sha) in &new_branches {
        let msg = git(&w.repo, &["log", "-1", "--format=%B", sha]);
        let trailer = |key: &str| {
            msg.lines()
                .find_map(|l| l.strip_prefix(key).map(|v| v.trim().to_owned()))
        };
        let receipt = trailer("Aver-Receipt:").and_then(|r| r.parse::<ReceiptId>().ok());
        let Some(receipt) = receipt else {
            fail(Inv::I2, format!("{branch} has no Aver-Receipt trailer"));
            continue;
        };
        match store.as_ref().map(|s| s.get::<Receipt>(&receipt)) {
            Some(Ok(r)) => {
                if !matches!(r.body.verdict, Verdict::Pass | Verdict::Review) {
                    fail(
                        Inv::I2,
                        format!("{branch}'s receipt is {:?}", r.body.verdict),
                    );
                }
                if trailer("Aver-Snapshot:") != Some(r.body.head_snapshot.to_string()) {
                    fail(Inv::I2, format!("{branch} is not the receipt's head"));
                }
            }
            _ => fail(
                Inv::I2,
                format!("{branch}'s receipt {receipt} is not stored"),
            ),
        }
        let mapped: i64 = conn
            .as_ref()
            .and_then(|c| {
                c.query_row(
                    "SELECT count(*) FROM git_map WHERE git_sha1 = ?1 AND kind = 'commit'",
                    [sha],
                    |r| r.get(0),
                )
                .ok()
            })
            .unwrap_or(0);
        if mapped == 0 {
            fail(Inv::I2, format!("{branch}'s commit is in no git_map row"));
        }
        let attempt = trailer("Aver-Attempt:");
        let decided = attempt.as_ref().is_some_and(|a| {
            conn.as_ref().is_some_and(|c| {
                c.query_row("SELECT state FROM attempts WHERE id = ?1", [a], |r| {
                    r.get::<_, String>(0)
                })
                .is_ok_and(|s| s == "accepted")
            })
        });
        if !named.contains(branch) && !decided {
            fail(Inv::I3, format!("nothing names {branch}"));
        }
    }

    // I4: every live directory is there and names its workspace, every
    // directory's workspace file that exists parses, no Aver temporary file
    // sits where a sync records it, and every overlay blob is stored.
    let live = live_dirs(conn.as_ref());
    for dir in &live {
        if !dir.is_dir() {
            fail(Inv::I4, format!("live directory {} is gone", dir.display()));
        }
    }
    for dir in workspace_dirs(w) {
        let meta = dir.join(".aver/workspace.toml");
        match fs::read_to_string(&meta) {
            Ok(t) if t.parse::<toml::Table>().is_ok() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !live.contains(&dir) => {}
            _ => fail(Inv::I4, format!("{} does not parse", meta.display())),
        }
        if !live.contains(&dir) {
            // An orphan of a cut materialize: gc's to trash.
            continue;
        }
        for leftover in aver::atomic::leftovers(&dir) {
            fail(Inv::I4, format!("temporary file {}", leftover.display()));
        }
        for leftover in restore_leftovers(&dir, &dir) {
            fail(Inv::I4, format!("temporary file {}", leftover.display()));
        }
    }
    if let Some(c) = &conn {
        let missing: Vec<String> = c
            .prepare(
                "SELECT workspace || ' ' || path FROM overlay o WHERE blob IS NOT NULL
                 AND NOT EXISTS (SELECT 1 FROM objects WHERE id = o.blob)",
            )
            .and_then(|mut q| q.query_map([], |r| r.get(0))?.collect())
            .unwrap_or_default();
        for row in missing {
            fail(Inv::I4, format!("overlay blob of {row} is not stored"));
        }
        let recorded: Vec<String> = c
            .prepare("SELECT path FROM overlay")
            .and_then(|mut q| q.query_map([], |r| r.get(0))?.collect())
            .unwrap_or_default();
        for path in recorded {
            let name = path.rsplit('/').next().unwrap_or(&path);
            if name.starts_with(".aver-restore-") || aver::atomic::is_tmp(name) {
                fail(
                    Inv::I4,
                    format!("a sync recorded the temporary file {path}"),
                );
            }
        }
    }
    // Every overlay blob is stored, so the workspace can be rebuilt.
    let lost: i64 = conn
        .as_ref()
        .and_then(|c| {
            c.query_row(
                "SELECT count(*) FROM overlay o WHERE o.blob IS NOT NULL
                 AND NOT EXISTS (SELECT 1 FROM objects WHERE id = o.blob)",
                [],
                |r| r.get(0),
            )
            .ok()
        })
        .unwrap_or(0);
    if lost > 0 {
        fail(
            Inv::I4,
            format!("{lost} overlay row(s) name a blob not stored"),
        );
    }
    for file in state_files(w) {
        let ok = fs::read(&file)
            .ok()
            .is_some_and(|b| serde_json::from_slice::<serde_json::Value>(&b).is_ok());
        if !ok {
            fail(Inv::I4, format!("{} does not parse", file.display()));
        }
    }

    // I5.
    if let Some(c) = &conn {
        let check: String = c
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap_or_else(|e| e.to_string());
        if check != "ok" {
            fail(Inv::I5, format!("integrity_check: {check}"));
        }
    }
    drop(conn);
    drop(store);

    // I6.
    for args in [&["status"][..], &["workspace", "ls"], &["doctor"]] {
        let out = w.aver(&w.repo, args, None);
        let err = String::from_utf8_lossy(&out.stderr);
        if out.status.code() == Some(101) || err.contains("panicked") {
            fail(Inv::I6, format!("aver {args:?} panicked: {}", text(&out)));
        }
    }
    broken
}

/// Branches a host record names: runs' decisions, batches' combines and
/// the combines Aver exported.
fn named_branches(w: &World) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for file in state_files(w) {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&fs::read(&file).unwrap()) else {
            continue;
        };
        for b in [&v["decided"]["branch"], &v["branch"]] {
            if let Some(b) = b.as_str() {
                out.insert(b.to_owned());
            }
        }
    }
    out
}

fn workspace_dirs(w: &World) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(repos) = fs::read_dir(w.home.join("work")) else {
        return out;
    };
    for repo in repos.flatten() {
        for dir in fs::read_dir(repo.path()).into_iter().flatten().flatten() {
            if dir.path().is_dir() && !dir.file_name().to_string_lossy().starts_with('.') {
                out.push(dir.path());
            }
        }
    }
    out
}

/// The directories of live materializations.
fn live_dirs(conn: Option<&rusqlite::Connection>) -> Vec<std::path::PathBuf> {
    conn.and_then(|c| {
        c.prepare("SELECT dir FROM materializations WHERE state = 'live'")
            .and_then(|mut q| {
                q.query_map([], |r| r.get::<_, String>(0))?
                    .map(|d| d.map(std::path::PathBuf::from))
                    .collect()
            })
            .ok()
    })
    .unwrap_or_default()
}

/// Temporary files a restore left where a sync records them: anywhere in
/// the directory but `.aver/`.
fn restore_leftovers(root: &Path, dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path == root.join(".aver") {
            continue;
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            out.extend(restore_leftovers(root, &path));
        } else if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".aver-restore-")
        {
            out.push(path);
        }
    }
    out
}

/// `runs/*/run.json`, `batches/*/{batch,tasks,combine}.json`,
/// `combines/*.json`.
fn state_files(w: &World) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = fs::read_dir(w.home.join("combines"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    for (dir, names) in [
        ("runs", &["run.json"][..]),
        ("batches", &["batch.json", "tasks.json", "combine.json"]),
    ] {
        for entry in fs::read_dir(w.home.join(dir))
            .into_iter()
            .flatten()
            .flatten()
        {
            for name in names {
                let f = entry.path().join(name);
                if f.exists() {
                    out.push(f);
                }
            }
        }
    }
    out
}
