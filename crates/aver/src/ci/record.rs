//! Cross-PR base records (ARCHITECTURE.md §2.9.2): a push to the default
//! branch records the runs of its head as a base would run them, and a
//! later pull request on that commit restores them, so its base side is a
//! cache hit instead of a second test run.
//!
//! A record is valid only where its runs would repeat: on the same runner
//! ([`RunnerKey`]), for the same base tree, with the dependencies the job's
//! install step put in the checkout, which are the head's ([`Identity`]).
//! `aver ci keys` names that identity as an `actions/cache` key without
//! importing anything; `aver ci --record` and `--base-record` check it
//! again on load, and rehash every object the record holds.

use std::collections::BTreeSet;
use std::path::Path as FsPath;
use std::process::Command;

use aver_core::{BlobId, CheckRun, EnvId, Outcome, RunId, RunIndex, SnapshotId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::exec::cache::CacheKey;
use crate::store::{Store, StoreError};

/// The record's index file in its directory.
pub const RECORD_FILE: &str = "record.json";
/// The directory of the runs' output and report blobs.
const BLOBS_DIR: &str = "blobs";
const SCHEMA: &str = "aver.ci-record/v0";
/// What every cache key starts with.
pub const KEY_PREFIX: &str = "aver-base-";

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("git {args}: {message}")]
    Git { args: String, message: String },
}

/// The runner a run's result can depend on beyond the tree and its
/// dependencies: the machine, its image, the workflow that set it up, and
/// the Aver and toolchains that ran the check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerKey {
    pub os: String,
    pub arch: String,
    /// GitHub's `ImageOS`, when set.
    pub image_os: Option<String>,
    /// GitHub's `ImageVersion`, when set.
    pub image_version: Option<String>,
    /// The Git blob of the running workflow file in the head commit.
    pub workflow: Option<String>,
    pub aver: String,
    /// The executor id: `native-v0.<host profile>`.
    pub executor: String,
}

impl RunnerKey {
    /// This process's runner, with `executor` and the workflow blob.
    pub fn here(executor: &str, workflow: Option<String>) -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            image_os: var("ImageOS"),
            image_version: var("ImageVersion"),
            workflow,
            aver: env!("CARGO_PKG_VERSION").to_owned(),
            executor: executor.to_owned(),
        }
    }

    pub fn id(&self) -> String {
        digest(&serde_json::to_vec(self).expect("a runner key serializes"))
    }
}

/// Where a record's runs hold: a runner, the tree they ran, and the
/// dependency inputs the checkout's dependencies were installed from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub runner: String,
    /// The Git tree of the commit whose runs these are.
    pub tree: String,
    /// The hash of the head's `deps.inputs` files, by path and Git blob.
    pub deps: String,
}

impl Identity {
    /// The `actions/cache` key: `aver-base-<hex>`.
    pub fn cache_key(&self) -> String {
        let body = format!("{}\0{}\0{}", self.runner, self.tree, self.deps);
        format!("{KEY_PREFIX}{}", &digest(body.as_bytes())[..40])
    }

    /// What differs from `other`, for the one line an ignored record gets.
    fn mismatch(&self, other: &Identity) -> Option<&'static str> {
        if self.runner != other.runner {
            Some("on another runner (image, workflow, Aver or toolchain)")
        } else if self.tree != other.tree {
            Some("for another base tree")
        } else if self.deps != other.deps {
            Some("with other dependency inputs")
        } else {
            None
        }
    }
}

/// `aver ci keys`: what this job restores for its base and saves for its
/// head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Keys {
    pub base: String,
    pub head: String,
    pub runner: RunnerKey,
    /// The base the keys name: `--base`, or its merge base with the head
    /// when it is not an ancestor, as `aver ci` compares.
    pub base_commit: String,
    pub head_commit: String,
}

/// The keys of comparing `head` with `base` in `repo` on `runner`.
pub fn keys(repo: &FsPath, base: &str, head: &str, runner: RunnerKey) -> Result<Keys, RecordError> {
    let head_commit = commit(repo, head)?;
    let base_commit = effective_base(repo, &commit(repo, base)?, &head_commit)?;
    let id = runner.id();
    let deps = deps_state(repo, &head_commit)?;
    let of = |commit: &str| -> Result<String, RecordError> {
        Ok(Identity {
            runner: id.clone(),
            tree: tree(repo, commit)?,
            deps: deps.clone(),
        }
        .cache_key())
    };
    Ok(Keys {
        base: of(&base_commit)?,
        head: of(&head_commit)?,
        runner,
        base_commit,
        head_commit,
    })
}

/// The identity of `commit`'s runs in a job whose head is `head`.
pub fn identity(
    repo: &FsPath,
    commit_rev: &str,
    head: &str,
    runner: &RunnerKey,
) -> Result<Identity, RecordError> {
    Ok(Identity {
        runner: runner.id(),
        tree: tree(repo, commit_rev)?,
        deps: deps_state(repo, head)?,
    })
}

/// The Git blob of the workflow file that runs this job: `path` when
/// given, else the one `GITHUB_WORKFLOW_REF` names
/// (`<owner>/<repo>/<path>@<ref>`), in `head`. `None` when neither names a
/// file there.
pub fn workflow_blob(
    repo: &FsPath,
    head: &str,
    path: Option<&str>,
) -> Result<Option<String>, RecordError> {
    let from_ref = std::env::var("GITHUB_WORKFLOW_REF").ok().and_then(|r| {
        let (path, _) = r.rsplit_once('@')?;
        let mut parts = path.splitn(3, '/');
        let (_, _, path) = (parts.next()?, parts.next()?, parts.next()?);
        Some(path.to_owned())
    });
    let Some(path) = path.map(str::to_owned).or(from_ref) else {
        return Ok(None);
    };
    git_opt(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{head}:{path}"),
        ],
    )
}

/// What restoring a record did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restored {
    /// The runs now in the cache, and how many rows did not qualify.
    Loaded { runs: Vec<RunId>, rejected: usize },
    /// The record was not for this job; why.
    Ignored(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordFile {
    schema: String,
    key: String,
    identity: Identity,
    runner: RunnerKey,
    rows: Vec<Row>,
}

/// One `run_cache` row; its snapshot is the run's.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    env: EnvId,
    check: String,
    executor: String,
    index: RunIndex,
    run: CheckRun,
}

/// Writes every cached run of `snapshot` to `dir`, with the blobs they
/// name, under `identity`. Returns how many runs it wrote.
pub fn write(
    store: &Store,
    dir: &FsPath,
    snapshot: &SnapshotId,
    identity: Identity,
    runner: RunnerKey,
) -> Result<usize, RecordError> {
    let blobs = dir.join(BLOBS_DIR);
    std::fs::create_dir_all(&blobs)?;
    let mut rows = Vec::new();
    let mut kept: BTreeSet<String> = BTreeSet::new();
    for (key, id) in CacheKey::rows_of(store, snapshot)? {
        let run: CheckRun = store.get(&id)?;
        for blob in std::iter::once(&run.output).chain(run.report.as_ref()) {
            let name = blob.to_string();
            let path = blobs.join(&name);
            if !path.is_file() {
                crate::atomic::write(&path, &store.get_raw(blob)?, crate::atomic::Mode::Keep)?;
            }
            kept.insert(name);
        }
        rows.push(Row {
            env: key.env,
            check: key.check,
            executor: key.executor,
            index: key.index,
            run,
        });
    }
    let n = rows.len();
    let file = RecordFile {
        schema: SCHEMA.to_owned(),
        key: identity.cache_key(),
        identity,
        runner,
        rows,
    };
    crate::atomic::write(
        &dir.join(RECORD_FILE),
        &serde_json::to_vec_pretty(&file)?,
        crate::atomic::Mode::Keep,
    )?;
    // Blobs are named by their hash and rehashed on load, so a concurrent
    // writer's removal here only drops rows from the record it wins with.
    for entry in std::fs::read_dir(&blobs)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !kept.contains(name.as_ref()) && !crate::atomic::is_tmp(&name) {
            match std::fs::remove_file(entry.path()) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
    }
    Ok(n)
}

/// Restores the record in `dir` into the run cache when it was made under
/// `expected`, keeping only rows of `base` whose objects rehash to what
/// they claim; anything else is ignored.
pub fn load(
    store: &Store,
    dir: &FsPath,
    expected: &Identity,
    base: &SnapshotId,
) -> Result<Restored, RecordError> {
    let bytes = match std::fs::read(dir.join(RECORD_FILE)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Restored::Ignored(format!("no {RECORD_FILE} in it")))
        }
        Err(e) => return Err(e.into()),
    };
    let file: RecordFile = match serde_json::from_slice(&bytes) {
        Ok(f) => f,
        Err(e) => {
            return Ok(Restored::Ignored(format!(
                "{RECORD_FILE} does not parse: {e}"
            )))
        }
    };
    if file.schema != SCHEMA {
        return Ok(Restored::Ignored(format!(
            "schema {:?}, not {SCHEMA}",
            file.schema
        )));
    }
    if let Some(why) = file.identity.mismatch(expected) {
        return Ok(Restored::Ignored(format!("it was recorded {why}")));
    }
    let mut blobs: BTreeSet<BlobId> = BTreeSet::new();
    match std::fs::read_dir(dir.join(BLOBS_DIR)) {
        Ok(entries) => {
            for entry in entries {
                let bytes = std::fs::read(entry?.path())?;
                if let Ok(id) = store.put_blob(&bytes) {
                    blobs.insert(id);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let (mut runs, mut rejected) = (Vec::new(), 0);
    store.tx(|tx| -> Result<(), StoreError> {
        for row in file.rows {
            let run = &row.run;
            let check_matches = row.check == run.check.as_str()
                || row
                    .check
                    .strip_prefix(run.check.as_str())
                    .is_some_and(|rest| rest.starts_with(':'));
            let ok = run.snapshot == *base
                && run.env == row.env
                && run.index == row.index
                && check_matches
                && run.result.outcome() != Outcome::Infra
                && blobs.contains(&run.output)
                && run.report.as_ref().is_none_or(|r| blobs.contains(r));
            if !ok {
                rejected += 1;
                continue;
            }
            let id = tx.put(run)?;
            CacheKey {
                snapshot: *base,
                env: row.env,
                check: row.check,
                executor: row.executor,
                index: row.index,
            }
            .insert(tx, &id)?;
            runs.push(id);
        }
        Ok(())
    })?;
    Ok(Restored::Loaded { runs, rejected })
}

/// The variables that mark an ephemeral CI job, whose home is gone after
/// it: there `aver ci` keeps no records in the home by default.
pub const CI_VARS: [&str; 8] = [
    "CI",
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "BUILDKITE",
    "CIRCLECI",
    "JENKINS_URL",
    "TF_BUILD",
    "TEAMCITY_VERSION",
];

/// The first of [`CI_VARS`] set in this process's environment, other than
/// to an empty string, `0` or `false`.
pub fn ci_environment() -> Option<&'static str> {
    CI_VARS.into_iter().find(|var| {
        std::env::var(var).is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false" | "FALSE"))
    })
}

/// The directory of the home's base records, one per [`Identity::cache_key`].
pub fn home_dir(home: &FsPath) -> std::path::PathBuf {
    home.join("records")
}

/// How long a home record is kept unwritten: every local `aver ci`
/// rewrites the record of its base.
pub const HOME_RECORD_AGE: std::time::Duration = std::time::Duration::from_secs(14 * 24 * 3600);
/// How many home records are kept, newest first.
pub const HOME_RECORDS_KEPT: usize = 64;

/// Removes the home's records under `root` that were last written more
/// than `max_age` ago, and all but the `keep` newest. Returns how many it
/// removed.
pub fn prune(root: &FsPath, max_age: std::time::Duration, keep: usize) -> std::io::Result<usize> {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with(KEY_PREFIX) {
            continue;
        }
        let written = std::fs::metadata(entry.path().join(RECORD_FILE))
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        records.push((written, entry.path()));
    }
    records.sort_by_key(|r| std::cmp::Reverse(r.0));
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for (n, (written, path)) in records.into_iter().enumerate() {
        let old = now.duration_since(written).is_ok_and(|age| age > max_age);
        if n >= keep || old {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(removed)
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn git_opt(repo: &FsPath, args: &[&str]) -> Result<Option<String>, RecordError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| RecordError::Git {
            args: args.join(" "),
            message: e.to_string(),
        })?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned()))
}

fn git(repo: &FsPath, args: &[&str]) -> Result<String, RecordError> {
    git_opt(repo, args)?.ok_or_else(|| RecordError::Git {
        args: args.join(" "),
        message: "failed".into(),
    })
}

fn commit(repo: &FsPath, rev: &str) -> Result<String, RecordError> {
    if rev.starts_with('-') {
        return Err(RecordError::Git {
            args: rev.to_owned(),
            message: "not a revision".into(),
        });
    }
    git(
        repo,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )
}

fn tree(repo: &FsPath, rev: &str) -> Result<String, RecordError> {
    git(repo, &["rev-parse", "--verify", &format!("{rev}^{{tree}}")])
}

/// `base`, or its merge base with `head` when it is not an ancestor, as
/// [`crate::git::resolve_base`] picks.
fn effective_base(repo: &FsPath, base: &str, head: &str) -> Result<String, RecordError> {
    if git_opt(repo, &["merge-base", "--is-ancestor", base, head])?.is_some() {
        return Ok(base.to_owned());
    }
    Ok(git_opt(repo, &["merge-base", base, head])?.unwrap_or_else(|| base.to_owned()))
}

/// The hash of the `(path, blob)` pairs of `head`'s lockfiles, package
/// manifests and the files its contract's `deps.inputs` name.
fn deps_state(repo: &FsPath, head: &str) -> Result<String, RecordError> {
    let config = git_opt(
        repo,
        &[
            "cat-file",
            "-p",
            &format!("{head}:{}", crate::env::CONFIG_PATH),
        ],
    )?;
    let inputs = config
        .and_then(|text| crate::contract::parse(&text).ok())
        .map(|p| p.contract.deps.inputs)
        .unwrap_or_else(aver_core::Set::empty);
    // Also the files that identify what an install put in the checkout,
    // whatever the contract names (its legacy shape names only
    // package-lock.json; no aver.toml names nothing): base runs use the
    // head's dependencies, so a head with another lockfile or manifest
    // never reuses them.
    let default = |path: &str| {
        let name = path.rsplit('/').next().unwrap_or(path);
        crate::env::kind_of_lockfile(name).is_some()
            || crate::place::sides::NODE_INPUTS.contains(&name)
            || crate::place::sides::PYTHON_INPUTS.contains(&name)
    };
    let mut rows: Vec<String> = Vec::new();
    let listing = git(repo, &["ls-tree", "-r", "-z", "--full-tree", head])?;
    for line in listing.split('\0') {
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let mut meta = meta.split_whitespace();
        let (Some(_), Some("blob"), Some(sha)) = (meta.next(), meta.next(), meta.next()) else {
            continue;
        };
        if default(path) || inputs.iter().any(|g| g.covers(path)) {
            rows.push(format!("{path}\0{sha}"));
        }
    }
    rows.sort();
    Ok(digest(rows.join("\n").as_bytes()))
}
