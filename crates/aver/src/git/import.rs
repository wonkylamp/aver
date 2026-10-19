//! Git import (ARCHITECTURE.md §2.2): one commit becomes one snapshot.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use aver_core::id::kind;
use aver_core::{
    AgentId, BlobId, EnvId, GitSha, LedgerId, LedgerKind, Name, Node, Object, ParseError, Path,
    RefName, Schema, Set, SigningKey, Snapshot, SnapshotId, Timestamp, Transition, TransitionBody,
    TransitionId, Tree, TreeEntry, TreeId, Via, MAX_BLOB_BYTES,
};

use super::{
    git_command, map_get, map_insert, parse_sha, record_transition, run, transitions_for_snapshot,
    GitError, Repo, ScratchDir,
};
use crate::env::{self, Detection, EnvError, ImageResolver, ObjectSource};
use crate::ledger::{self, Entry, Scope};
use crate::secrets;
use crate::store::{Store, StoreError, Tx};

/// What to import and on whose behalf.
pub struct ImportOptions<'a> {
    /// A local repository path (bare or not) or a URL, which is cloned bare
    /// into a scratch directory first.
    pub source: &'a str,
    /// Any revision `git rev-parse` resolves to a commit.
    pub rev: &'a str,
    /// The agent that requested the import.
    pub agent: AgentId,
    /// Signs the transition and the ledger entry.
    pub server_key: &'a SigningKey,
    pub resolver: &'a dyn ImageResolver,
    /// A ref to point at the imported snapshot in the same transaction.
    pub update_ref: Option<RefUpdate>,
    /// Paths (files or directories) left out of the snapshot, unchecked.
    pub exclude: &'a [Path],
    /// What a secret-scanner finding does to the import.
    pub secrets: SecretPolicy,
    /// What a blob over [`MAX_BLOB_BYTES`] does to the import.
    pub oversize: OversizePolicy,
    /// Whether a tree without `[[check]]` gets detected checks: only in
    /// `aver ci`.
    pub detection: Detection,
}

/// What a secret-scanner finding does to an import (ARCHITECTURE.md §2.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretPolicy {
    /// Any finding fails the import: for repositories whose snapshots agents
    /// read through workspaces.
    #[default]
    Reject,
    /// Findings are reported in [`ImportReport::secrets`] and the import
    /// proceeds: for `aver ci`, where the content is already in the user's
    /// Git and only network-less sandboxes read it.
    Warn,
    /// A [`Severity::High`](crate::secrets::Severity) finding fails the
    /// import; low ones (fixtures, test data, templates, docs) are reported
    /// as under [`Self::Warn`]: for workspaces and `aver run`, where a test
    /// key in `tests/fixtures/` must not stop the work.
    #[serde(rename = "reject-high")]
    RejectHigh,
}

impl SecretPolicy {
    fn rejects(self, finding: &secrets::SecretFinding) -> bool {
        match self {
            Self::Reject => true,
            Self::Warn => false,
            Self::RejectHigh => finding.severity == secrets::Severity::High,
        }
    }
}

/// One line naming the files whose secret-like content an import let
/// through, never the content; `None` when there are none.
pub fn secrets_warning(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    Some(format!(
        "aver: imported anyway, secret-like content in test or example files: {}",
        paths.join(", ")
    ))
}

/// What a blob over [`MAX_BLOB_BYTES`] does to an import.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OversizePolicy {
    /// The import fails.
    #[default]
    Reject,
    /// The file is imported empty and listed in
    /// [`ImportReport::placeholders`]; a check that reads it fails visibly.
    Placeholder,
}

/// A file imported empty because its blob was over [`MAX_BLOB_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Placeholder {
    pub path: String,
    pub bytes: u64,
}

impl<'a> ImportOptions<'a> {
    pub fn new(
        source: &'a str,
        rev: &'a str,
        agent: AgentId,
        server_key: &'a SigningKey,
        resolver: &'a dyn ImageResolver,
    ) -> Self {
        Self {
            source,
            rev,
            agent,
            server_key,
            resolver,
            update_ref: None,
            exclude: &[],
            secrets: SecretPolicy::Reject,
            oversize: OversizePolicy::Reject,
            detection: Detection::Off,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate {
    pub name: RefName,
    pub expect: RefPrecondition,
}

/// What the ref must point at for the update to apply. A ref that already
/// points at the imported snapshot always satisfies it, so retries succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefPrecondition {
    Any,
    Absent,
    Is(SnapshotId),
}

#[derive(Debug, Clone)]
pub struct ImportReport {
    pub snapshot: SnapshotId,
    pub env: EnvId,
    pub transition: TransitionId,
    /// The ledger entry written, if the import changed anything.
    pub ledger: Option<LedgerId>,
    pub commit: GitSha,
    pub files: usize,
    /// Total size of all files, counting a blob once per path.
    pub bytes: u64,
    /// Objects that were not already in the store.
    pub new_objects: usize,
    /// True when an earlier import already recorded this commit as this snapshot.
    pub already_imported: bool,
    pub duration: Duration,
    /// The commit's message.
    pub message: String,
    /// Secret-scanner findings the policy let through, by path.
    pub secrets: Vec<secrets::SecretFinding>,
    /// Files imported empty under [`OversizePolicy::Placeholder`], by path.
    pub placeholders: Vec<Placeholder>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Violation {
    /// The path as Git stores it, lossily decoded if it is not UTF-8.
    pub path: String,
    pub reason: ViolationReason,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "kebab-case")]
pub enum ViolationReason {
    InvalidPath(String),
    Submodule,
    UnsupportedMode(String),
    TooLarge(u64),
    /// The name of the matched pattern, never the matched bytes.
    Secret(&'static str),
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: ", self.path)?;
        match &self.reason {
            ViolationReason::InvalidPath(why) => write!(f, "invalid path ({why})"),
            ViolationReason::Submodule => f.write_str("submodules are not supported"),
            ViolationReason::UnsupportedMode(mode) => write!(f, "unsupported file mode {mode}"),
            ViolationReason::TooLarge(size) => {
                write!(
                    f,
                    "{size} bytes exceeds the {MAX_BLOB_BYTES}-byte blob limit"
                )
            }
            ViolationReason::Secret(pattern) => write!(f, "looks like a secret ({pattern})"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Env(#[from] EnvError),
    #[error("{0:?} is neither an existing path nor a URL")]
    Source(String),
    #[error("revision {0:?} does not name a commit")]
    Rev(String),
    #[error("import rejected, {} violation(s):\n{}", .0.len(), list(.0))]
    Rejected(Vec<Violation>),
    #[error("commit {commit}: {detail}")]
    Commit { commit: GitSha, detail: String },
}

fn list(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|v| format!("  {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Imports `options.rev` from `options.source` into `store`.
///
/// Every path is validated and every blob secret-scanned before anything is
/// written; any violation fails the import with all of them listed. On success
/// one transaction writes the objects, snapshot, signed `git-import`
/// transition, `git_map` rows, an `import` ledger entry chained on
/// `ledger/<scope>`, and the optional ref update. Importing a commit that is
/// already recorded as the same snapshot writes nothing new.
pub fn import(store: &Store, options: &ImportOptions<'_>) -> Result<ImportReport, ImportError> {
    let started = Instant::now();
    if let Some(report) = memoized(store, options, started)? {
        return Ok(report);
    }
    let (repo, repo_label, _scratch) = open_source(store, options.source)?;
    let (commit, header) = resolve_commit(&repo, options.rev)?;
    let mut listing = classify(ls_tree(&repo, &commit)?, options.exclude);

    let blobs = read_blobs(&repo, &listing.blob_shas, MAX_BLOB_BYTES as u64)?;
    let mut placeholders = Vec::new();
    for file in &listing.files {
        if let Blob::TooLarge(size) = blobs[file.blob] {
            let path = file.path.as_str().to_owned();
            match options.oversize {
                OversizePolicy::Reject => listing.violations.push(Violation {
                    path,
                    reason: ViolationReason::TooLarge(size),
                }),
                OversizePolicy::Placeholder => placeholders.push(Placeholder { path, bytes: size }),
            }
        }
    }
    let findings = par_map(&listing.files, |file| match &blobs[file.blob] {
        Blob::Bytes(bytes) => secrets::scan(&file.path, bytes),
        Blob::TooLarge(_) => None,
    });
    let mut findings: Vec<secrets::SecretFinding> = findings.into_iter().flatten().collect();
    findings.sort_by(|a, b| a.path.cmp(&b.path));
    let (rejected, findings): (Vec<_>, Vec<_>) = findings
        .into_iter()
        .partition(|finding| options.secrets.rejects(finding));
    listing
        .violations
        .extend(rejected.into_iter().map(|finding| Violation {
            path: finding.path,
            reason: ViolationReason::Secret(finding.pattern),
        }));
    if !listing.violations.is_empty() {
        listing.violations.sort_by(|a, b| a.path.cmp(&b.path));
        return Err(ImportError::Rejected(listing.violations));
    }
    placeholders.sort_by(|a, b| a.path.cmp(&b.path));
    // A placeholder's blob is not the Git blob, so Git SHAs recorded for it,
    // its trees or the commit would be wrong for export.
    let altered = listing.excluded_any || !placeholders.is_empty();
    let placeholder_blobs: HashSet<usize> = blobs
        .iter()
        .enumerate()
        .filter(|(_, b)| matches!(b, Blob::TooLarge(_)))
        .map(|(i, _)| i)
        .collect();
    let blobs: Vec<Vec<u8>> = blobs
        .into_iter()
        .map(|b| match b {
            Blob::Bytes(bytes) => bytes,
            Blob::TooLarge(_) => Vec::new(),
        })
        .collect();
    let blob_ids = par_map(&blobs, |bytes| BlobId::of_bytes(bytes));

    let mut root_dir = Dir::default();
    for file in &listing.files {
        let id = blob_ids[file.blob];
        let node = match file.kind {
            FileKind::File => Node::File(id),
            FileKind::Exec => Node::Exec(id),
            FileKind::Symlink => Node::Symlink(id),
        };
        root_dir.insert(&file.path, node);
    }
    let mut trees = Vec::new();
    let root = root_dir.finish(String::new(), &mut trees);

    let staged = Staged {
        trees: trees.iter().map(|t| (t.id, &t.tree)).collect(),
        blobs: blob_ids
            .iter()
            .zip(&blobs)
            .map(|(id, b)| (*id, b.as_slice()))
            .collect(),
    };
    let environment = env::environment(&staged, &root, options.resolver, options.detection)?;
    let env_id = environment.id();
    let snapshot = Snapshot::new(root, env_id);
    let snapshot_id = snapshot.id();

    let bytes: u64 = listing
        .files
        .iter()
        .map(|f| blobs[f.blob].len() as u64)
        .sum();
    let (transition, ledger, new_objects, already_imported) = store.tx(|tx| {
        let mut new_objects = 0;
        let mut put = |is_new: bool| new_objects += usize::from(is_new);
        let existing = find_existing(tx, &snapshot_id, &commit)?;
        let transition_id = match existing {
            Some(id) => id,
            None => {
                for (id, bytes) in blob_ids.iter().zip(&blobs) {
                    let is_new = !tx.has(id)?;
                    if is_new {
                        tx.put_blob(bytes)?;
                    }
                    put(is_new);
                }
                for staged in &trees {
                    let is_new = !tx.has(&staged.id)?;
                    if is_new {
                        tx.put(&staged.tree)?;
                    }
                    put(is_new);
                }
                put(!tx.has(&env_id)?);
                tx.put(&environment)?;
                put(!tx.has(&snapshot_id)?);
                tx.put(&snapshot)?;

                for (i, (id, sha)) in blob_ids.iter().zip(&listing.blob_shas).enumerate() {
                    if !placeholder_blobs.contains(&i) {
                        map_insert(&tx.sql, &id.to_string(), sha, "blob")?;
                    }
                }
                // With exclusions or placeholders the trees and the commit
                // differ from Git's, so their Git SHAs would be wrong for export.
                if !altered {
                    for staged in &trees {
                        let sha = if staged.path.is_empty() {
                            Some(&header.tree)
                        } else {
                            listing.dir_shas.get(&staged.path)
                        };
                        if let Some(sha) = sha {
                            map_insert(&tx.sql, &staged.id.to_string(), sha, "tree")?;
                        }
                    }
                }

                let transition = Transition::sign(
                    TransitionBody {
                        schema: Schema::new(),
                        from: Vec::new(),
                        to: snapshot_id,
                        via: Via::GitImport {
                            repo: repo_label.clone(),
                            commit: commit.clone(),
                        },
                        agent: options.agent,
                        message: header.message.clone(),
                        at: header.committed_at,
                    },
                    options.server_key,
                );
                let id = record_transition(tx, &transition)?;
                put(true);
                if !altered {
                    map_insert(&tx.sql, &id.to_string(), &commit, "commit")?;
                }
                id
            }
        };
        if !altered {
            let findings: Vec<(&str, &str)> = findings
                .iter()
                .map(|f| (f.path.as_str(), f.pattern))
                .collect();
            tx.sql.execute(
                "INSERT OR REPLACE INTO import_memo (git_commit, transition, files, bytes, findings)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    commit.as_str(),
                    transition_id.to_string(),
                    listing.files.len() as i64,
                    bytes as i64,
                    serde_json::to_string(&findings).expect("findings serialize"),
                ],
            )
            .map_err(StoreError::from)?;
        }

        let moved_from = match &options.update_ref {
            Some(update) => update_ref(tx, update, &snapshot_id)?,
            None => None,
        };
        let ledger = if existing.is_none() || moved_from.is_some() {
            put(true);
            Some(append_ledger(
                tx,
                options,
                moved_from.is_some(),
                moved_from.flatten(),
                snapshot_id,
                format!("Imported {repo_label} at {commit}"),
            )?)
        } else {
            None
        };
        Ok::<_, ImportError>((transition_id, ledger, new_objects, existing.is_some()))
    })?;

    Ok(ImportReport {
        snapshot: snapshot_id,
        env: env_id,
        transition,
        ledger,
        commit,
        files: listing.files.len(),
        bytes,
        new_objects,
        already_imported,
        duration: started.elapsed(),
        message: header.message,
        secrets: findings,
        placeholders,
    })
}

/// The report of an earlier import of the same commit, read from
/// `import_memo` without running Git: for a local repository whose `rev`
/// resolves from its files, an import that moves no ref and excludes
/// nothing, and a snapshot whose environment this binary, resolver and
/// detection still derive. `None` means import in full, as for a finding
/// the policy now rejects, so the full import refuses it.
fn memoized(
    store: &Store,
    options: &ImportOptions<'_>,
    started: Instant,
) -> Result<Option<ImportReport>, ImportError> {
    if options.update_ref.is_some() || !options.exclude.is_empty() || looks_like_url(options.source)
    {
        return Ok(None);
    }
    let Some(commit) = super::local::resolve(std::path::Path::new(options.source), options.rev)
    else {
        return Ok(None);
    };
    let rows: Vec<(String, i64, i64, String)> = store.read(|conn| {
        let mut stmt = conn.prepare_cached(
            "SELECT transition, files, bytes, findings FROM import_memo WHERE git_commit = ?1",
        )?;
        let rows = stmt.query_map([commit.as_str()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })?;
    for (transition, files, bytes, findings) in rows {
        let Ok(transition_id) = transition.parse::<TransitionId>() else {
            continue;
        };
        let body = store.get::<Transition>(&transition_id)?.body;
        if !matches!(&body.via, Via::GitImport { commit: c, .. } if *c == commit) {
            continue;
        }
        let snapshot: Snapshot = store.get(&body.to)?;
        let environment =
            env::environment(store, &snapshot.root, options.resolver, options.detection)?;
        if snapshot.env != Some(environment.id()) {
            continue;
        }
        let Ok(pairs) = serde_json::from_str::<Vec<(String, String)>>(&findings) else {
            continue;
        };
        let mut found = Vec::with_capacity(pairs.len());
        for (path, pattern) in &pairs {
            let Some(f) = path
                .parse::<Path>()
                .ok()
                .and_then(|p| secrets::finding(&p, pattern))
            else {
                return Ok(None);
            };
            found.push(f);
        }
        if found.iter().any(|f| options.secrets.rejects(f)) {
            return Ok(None);
        }
        return Ok(Some(ImportReport {
            snapshot: body.to,
            env: environment.id(),
            transition: transition_id,
            ledger: None,
            commit,
            files: files as usize,
            bytes: bytes as u64,
            new_objects: 0,
            already_imported: true,
            duration: started.elapsed(),
            message: body.message,
            secrets: found,
            placeholders: Vec::new(),
        }));
    }
    Ok(None)
}

/// The ls-tree listing sorted into files to import and violations.
struct Listing {
    files: Vec<FileEntry>,
    /// Unique blobs; `FileEntry::blob` indexes this.
    blob_shas: Vec<GitSha>,
    /// Git tree SHA of every directory, by path.
    dir_shas: HashMap<String, GitSha>,
    excluded_any: bool,
    violations: Vec<Violation>,
}

fn classify(entries: Vec<LsEntry>, exclude: &[Path]) -> Listing {
    let mut listing = Listing {
        files: Vec::new(),
        blob_shas: Vec::new(),
        dir_shas: HashMap::new(),
        excluded_any: false,
        violations: Vec::new(),
    };
    let mut blob_index: HashMap<GitSha, usize> = HashMap::new();
    // Git's fsck rejects a tree naming one entry twice, but such trees can
    // still be written; repository content is untrusted.
    let mut seen: HashSet<String> = HashSet::new();
    for entry in entries {
        let path = match std::str::from_utf8(&entry.path) {
            Ok(s) => s.parse::<Path>().map_err(|e| match e {
                ParseError::Invalid { reason, .. } => reason,
                other => other.to_string(),
            }),
            Err(_) => Err("not valid UTF-8".to_owned()),
        };
        let violation = |reason| Violation {
            path: String::from_utf8_lossy(&entry.path).into_owned(),
            reason,
        };
        let path = match path {
            Ok(path) => path,
            // An invalid directory name is reported through the entries under it.
            Err(_) if entry.mode == "040000" => continue,
            Err(reason) => {
                listing
                    .violations
                    .push(violation(ViolationReason::InvalidPath(reason)));
                continue;
            }
        };
        if is_excluded(&path, exclude) {
            listing.excluded_any = true;
            continue;
        }
        if !seen.insert(path.as_str().to_owned()) {
            listing
                .violations
                .push(violation(ViolationReason::InvalidPath(
                    "names more than one entry".to_owned(),
                )));
            continue;
        }
        let kind = match entry.mode.as_str() {
            "040000" => {
                listing.dir_shas.insert(path.as_str().to_owned(), entry.sha);
                continue;
            }
            "100644" => FileKind::File,
            "100755" => FileKind::Exec,
            "120000" => FileKind::Symlink,
            "160000" => {
                listing
                    .violations
                    .push(violation(ViolationReason::Submodule));
                continue;
            }
            other => {
                listing
                    .violations
                    .push(violation(ViolationReason::UnsupportedMode(
                        other.to_owned(),
                    )));
                continue;
            }
        };
        let blob_shas = &mut listing.blob_shas;
        let blob = *blob_index.entry(entry.sha.clone()).or_insert_with(|| {
            blob_shas.push(entry.sha.clone());
            blob_shas.len() - 1
        });
        listing.files.push(FileEntry { path, kind, blob });
    }
    listing
}

/// A transition already recording `commit` as `snapshot`, if any.
fn find_existing(
    tx: &Tx<'_>,
    snapshot: &SnapshotId,
    commit: &GitSha,
) -> Result<Option<TransitionId>, StoreError> {
    for id in transitions_for_snapshot(&tx.sql, snapshot)? {
        let recorded = match tx.get::<Transition>(&id)?.body.via {
            Via::GitImport { commit: c, .. } => c == *commit,
            Via::Workspace { .. } => map_get(&tx.sql, &id.to_string())?.as_ref() == Some(commit),
        };
        if recorded {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Appends an `import` entry to `ledger/ref:<name>` when the import moved
/// that ref, else to `ledger/imports` (§2.2).
fn append_ledger(
    tx: &Tx<'_>,
    options: &ImportOptions<'_>,
    moved: bool,
    base: Option<SnapshotId>,
    head: SnapshotId,
    summary: String,
) -> Result<LedgerId, StoreError> {
    let scope = match &options.update_ref {
        Some(update) if moved => Scope::Ref(update.name.clone()),
        _ => Scope::Imports,
    };
    ledger::append(
        tx,
        options.server_key,
        Entry {
            scope,
            kind: LedgerKind::Import,
            task: None,
            attempts: Vec::new(),
            winner: None,
            base,
            head: Some(head),
            summary,
            rationale: "Git import".to_owned(),
            lessons: Vec::new(),
            agent: options.agent,
        },
    )
}

/// Applies `update`. Returns `Some(previous target)` if the ref moved, `None`
/// if it already pointed at `snapshot`.
fn update_ref(
    tx: &Tx<'_>,
    update: &RefUpdate,
    snapshot: &SnapshotId,
) -> Result<Option<Option<SnapshotId>>, StoreError> {
    let current = tx
        .get_ref::<kind::Snapshot>(&update.name)?
        .map(|r| r.target);
    if current.as_ref() == Some(snapshot) {
        return Ok(None);
    }
    let expected = match &update.expect {
        RefPrecondition::Any => current,
        RefPrecondition::Absent => None,
        RefPrecondition::Is(id) => Some(*id),
    };
    tx.compare_and_set(&update.name, expected.as_ref(), snapshot)?;
    Ok(Some(current))
}

fn is_excluded(path: &Path, exclude: &[Path]) -> bool {
    exclude.iter().any(|e| {
        path.as_str()
            .strip_prefix(e.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// The commits a comparison of `base` and `head` should use (§2.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseResolution {
    /// The commit `base` resolved to.
    pub requested: GitSha,
    /// `requested` when it is an ancestor of the head (or they share no
    /// history); otherwise their merge base, the commit the head forked from.
    pub effective: GitSha,
    pub head: GitSha,
}

impl BaseResolution {
    /// Whether the effective base differs from the requested one.
    pub fn moved(&self) -> bool {
        self.requested != self.effective
    }
}

/// Resolves `base` and `head` in `source` and picks the base to judge
/// against: a pull request's recorded base is the base branch's tip when the
/// request was last updated, which can be ahead of where the head forked,
/// and diffing against it would count the base branch's newer commits as the
/// change, reversed. A URL is cloned bare into a scratch directory first.
pub fn resolve_base(
    store: &Store,
    source: &str,
    base: &str,
    head: &str,
) -> Result<BaseResolution, ImportError> {
    let (repo, _, _scratch) = open_source(store, source)?;
    let (requested, _) = resolve_commit(&repo, base)?;
    let (head, _) = resolve_commit(&repo, head)?;
    let (ancestor, _) = repo.status([
        "merge-base",
        "--is-ancestor",
        requested.as_str(),
        head.as_str(),
    ])?;
    let effective = if ancestor {
        requested.clone()
    } else {
        match repo.status(["merge-base", requested.as_str(), head.as_str()])? {
            (true, out) => parse_sha(
                "merge-base",
                String::from_utf8_lossy(&out).trim().as_bytes(),
            )?,
            // No common history: there is nothing better to compare with.
            (false, _) => requested.clone(),
        }
    };
    Ok(BaseResolution {
        requested,
        effective,
        head,
    })
}

/// Opens a local repository in place, or clones a URL bare into a scratch
/// directory that lives as long as the returned guard.
fn open_source(
    store: &Store,
    source: &str,
) -> Result<(Repo, String, Option<ScratchDir>), ImportError> {
    let local = std::path::Path::new(source);
    if local.exists() {
        let canonical = local
            .canonicalize()
            .map_err(|e| ImportError::Git(GitError::Spawn(e)))?;
        let label = canonical.to_string_lossy().into_owned();
        return Ok((Repo::new(canonical), label, None));
    }
    if !looks_like_url(source) {
        return Err(ImportError::Source(source.to_owned()));
    }
    let scratch =
        ScratchDir::new(store, "clone").map_err(|e| ImportError::Git(GitError::Spawn(e)))?;
    let dest = scratch.path().join("repo.git");
    let mut cmd = git_command();
    cmd.args([
        "-c",
        "protocol.ext.allow=never",
        "clone",
        "--bare",
        "--quiet",
        "--",
        source,
    ])
    .arg(&dest);
    let label = redact_url(source);
    // The failure names the command line, which holds the URL as given.
    run(cmd).map_err(|e| match e {
        GitError::Failed { command, stderr } => GitError::Failed {
            command: command.replace(source, &label),
            stderr: stderr.replace(source, &label),
        },
        other => other,
    })?;
    Ok((Repo::new(dest), label, Some(scratch)))
}

fn looks_like_url(source: &str) -> bool {
    if source.contains("://") {
        return true;
    }
    // scp-like `host:path`: a colon before any slash.
    match (source.find(':'), source.find('/')) {
        (Some(colon), Some(slash)) => colon < slash && colon > 0,
        (Some(colon), None) => colon > 0,
        _ => false,
    }
}

/// Drops `user:password@` from a URL so credentials never enter a signed object.
pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_owned(),
    }
}

struct CommitHeader {
    tree: GitSha,
    committed_at: Timestamp,
    message: String,
}

/// Resolves `rev` to a commit and reads it, with one `cat-file --batch`.
fn resolve_commit(repo: &Repo, rev: &str) -> Result<(GitSha, CommitHeader), ImportError> {
    let not_a_commit = || ImportError::Rev(rev.to_owned());
    if rev.is_empty() || rev.chars().any(char::is_control) {
        return Err(not_a_commit());
    }
    let mut child = repo
        .command(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(GitError::from)?;
    let mut stdin = child.stdin.take().expect("piped");
    stdin
        .write_all(format!("{rev}^{{commit}}\n").as_bytes())
        .map_err(GitError::from)?;
    drop(stdin);
    let out = child.wait_with_output().map_err(GitError::from)?;
    if !out.status.success() {
        return Err(GitError::Failed {
            command: "cat-file --batch".into(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        }
        .into());
    }
    let newline = out
        .stdout
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(not_a_commit)?;
    let header = std::str::from_utf8(&out.stdout[..newline]).map_err(|_| not_a_commit())?;
    let mut fields = header.split(' ');
    let (Some(sha), Some("commit"), Some(size), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(not_a_commit());
    };
    let commit = parse_sha("cat-file --batch", sha.as_bytes())?;
    let size: usize = size.parse().map_err(|_| not_a_commit())?;
    let body = out
        .stdout
        .get(newline + 1..newline + 1 + size)
        .ok_or_else(not_a_commit)?;
    let header = parse_commit(&commit, body)?;
    Ok((commit, header))
}

fn parse_commit(commit: &GitSha, raw: &[u8]) -> Result<CommitHeader, ImportError> {
    let bad = |detail: &str| ImportError::Commit {
        commit: commit.clone(),
        detail: detail.to_owned(),
    };
    let (headers, message) = raw
        .windows(2)
        .position(|w| w == b"\n\n")
        .map_or((raw, &[][..]), |i| (&raw[..i], &raw[i + 2..]));
    let mut tree = None;
    let mut committed_at = None;
    for line in headers.split(|b| *b == b'\n') {
        if let Some(sha) = line.strip_prefix(b"tree ") {
            tree = Some(parse_sha("cat-file commit", sha)?);
        } else if let Some(rest) = line.strip_prefix(b"committer ") {
            let rest = String::from_utf8_lossy(rest);
            let mut fields = rest.rsplit(' ');
            let _tz = fields.next();
            let secs: i64 = fields
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| bad("malformed committer line"))?;
            committed_at = Some(
                secs.checked_mul(1000)
                    .and_then(|ms| Timestamp::from_unix_millis(ms).ok())
                    .ok_or_else(|| bad("committer time is out of range"))?,
            );
        }
    }
    Ok(CommitHeader {
        tree: tree.ok_or_else(|| bad("no tree header"))?,
        committed_at: committed_at.ok_or_else(|| bad("no committer header"))?,
        message: String::from_utf8_lossy(message).into_owned(),
    })
}

struct LsEntry {
    mode: String,
    sha: GitSha,
    path: Vec<u8>,
}

fn ls_tree(repo: &Repo, commit: &GitSha) -> Result<Vec<LsEntry>, GitError> {
    let out = repo.run(["ls-tree", "-r", "-t", "-z", "--full-tree", commit.as_str()])?;
    let protocol = |detail: String| GitError::Protocol {
        command: "ls-tree".into(),
        detail,
    };
    out.split(|b| *b == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .ok_or_else(|| protocol("record without a tab".into()))?;
            let meta = std::str::from_utf8(&record[..tab])
                .map_err(|_| protocol("non-UTF-8 metadata".into()))?;
            let mut fields = meta.split(' ');
            let (Some(mode), Some(_type), Some(sha), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(protocol(format!("malformed record {meta:?}")));
            };
            Ok(LsEntry {
                mode: mode.to_owned(),
                sha: parse_sha("ls-tree", sha.as_bytes())?,
                path: record[tab + 1..].to_vec(),
            })
        })
        .collect()
}

enum Blob {
    Bytes(Vec<u8>),
    TooLarge(u64),
}

/// A file of a commit's tree: its path, blob and kind.
pub(crate) struct TrackedFile {
    pub path: String,
    pub sha: GitSha,
    pub kind: crate::workspace::FileKind,
}

/// Every file of `commit`'s tree (submodules left out), in Git's order.
pub(crate) fn tracked_files(repo: &Repo, commit: &GitSha) -> Result<Vec<TrackedFile>, GitError> {
    use crate::workspace::FileKind as Kind;
    Ok(ls_tree(repo, commit)?
        .into_iter()
        .filter_map(|e| {
            let kind = match e.mode.as_str() {
                "100644" | "100664" => Kind::File,
                "100755" => Kind::Exec,
                "120000" => Kind::Symlink,
                _ => return None,
            };
            Some(TrackedFile {
                path: String::from_utf8_lossy(&e.path).into_owned(),
                sha: e.sha,
                kind,
            })
        })
        .collect())
}

/// The bytes of each blob in `shas`, whatever their size, in order.
pub(crate) fn read_blobs_whole(repo: &Repo, shas: &[GitSha]) -> Result<Vec<Vec<u8>>, GitError> {
    Ok(read_blobs(repo, shas, u64::MAX)?
        .into_iter()
        .map(|b| match b {
            Blob::Bytes(bytes) => bytes,
            Blob::TooLarge(_) => unreachable!("no size is over u64::MAX"),
        })
        .collect())
}

/// Streams blobs through one `git cat-file --batch`, in order. (Splitting
/// the set across several processes measured no faster.)
fn read_blobs(repo: &Repo, shas: &[GitSha], max: u64) -> Result<Vec<Blob>, GitError> {
    let protocol = |detail: String| GitError::Protocol {
        command: "cat-file --batch".into(),
        detail,
    };
    let mut child = repo
        .command(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let mut stdout = BufReader::with_capacity(1 << 20, child.stdout.take().expect("piped"));
    let mut read_all = || -> Result<Vec<Blob>, GitError> {
        let mut blobs = Vec::with_capacity(shas.len());
        let mut header = String::new();
        for sha in shas {
            header.clear();
            stdout.read_line(&mut header)?;
            let mut fields = header.trim_end().split(' ');
            let (Some(got), Some("blob"), Some(size)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(protocol(format!("unexpected header {header:?} for {sha}")));
            };
            if got != sha.as_str() {
                return Err(protocol(format!("expected {sha}, got {got}")));
            }
            let size: u64 = size
                .parse()
                .map_err(|_| protocol(format!("bad size in {header:?}")))?;
            if size > max {
                std::io::copy(&mut (&mut stdout).take(size), &mut std::io::sink())?;
                blobs.push(Blob::TooLarge(size));
            } else {
                let mut bytes = vec![0; size as usize];
                stdout.read_exact(&mut bytes)?;
                blobs.push(Blob::Bytes(bytes));
            }
            let mut newline = [0u8; 1];
            stdout.read_exact(&mut newline)?;
        }
        Ok(blobs)
    };
    let result = std::thread::scope(|scope| {
        let writer = scope.spawn(move || -> std::io::Result<()> {
            let mut stdin = std::io::BufWriter::new(stdin);
            for sha in shas {
                writeln!(stdin, "{sha}")?;
            }
            stdin.flush()
        });
        let result = read_all();
        if result.is_err() {
            // Unblocks a writer stuck on a full pipe.
            let _ = child.kill();
        }
        let written = writer.join().expect("writer thread panicked");
        result.and_then(|blobs| written.map(|()| blobs).map_err(GitError::from))
    });
    let status = child.wait()?;
    let blobs = result?;
    if !status.success() {
        return Err(protocol(format!("exited with {status}")));
    }
    Ok(blobs)
}

/// Maps `f` over `items` on all cores, preserving order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    if threads <= 1 || items.len() < 64 {
        return items.iter().map(f).collect();
    }
    let chunk_len = items.len().div_ceil(threads * 8);
    let chunks: Vec<&[T]> = items.chunks(chunk_len).collect();
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, Vec<R>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(chunk) = chunks.get(i) else {
                            return out;
                        };
                        out.push((i, chunk.iter().map(&f).collect()));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("worker panicked"))
            .collect()
    });
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().flat_map(|(_, r)| r).collect()
}

#[derive(Clone, Copy)]
enum FileKind {
    File,
    Exec,
    Symlink,
}

struct FileEntry {
    path: Path,
    kind: FileKind,
    blob: usize,
}

#[derive(Default)]
struct Dir {
    entries: BTreeMap<String, Child>,
}

enum Child {
    Leaf(Node),
    Dir(Dir),
}

struct StagedTree {
    id: TreeId,
    tree: Tree,
    /// Directory path, empty for the root.
    path: String,
}

impl Dir {
    fn insert(&mut self, path: &Path, node: Node) {
        let mut dir = self;
        let mut segments = path.segments().peekable();
        while let Some(segment) = segments.next() {
            if segments.peek().is_none() {
                dir.entries.insert(segment.to_owned(), Child::Leaf(node));
                return;
            }
            let child = dir
                .entries
                .entry(segment.to_owned())
                .or_insert_with(|| Child::Dir(Dir::default()));
            dir = match child {
                Child::Dir(d) => d,
                // Git cannot hold a file and a directory under one name.
                Child::Leaf(_) => unreachable!("file and directory share a path"),
            };
        }
    }

    /// Builds this directory's tree and all trees under it, children first.
    fn finish(self, path: String, out: &mut Vec<StagedTree>) -> TreeId {
        let entries = self
            .entries
            .into_iter()
            .map(|(name, child)| {
                let node = match child {
                    Child::Leaf(node) => node,
                    Child::Dir(dir) => {
                        let sub = if path.is_empty() {
                            name.clone()
                        } else {
                            format!("{path}/{name}")
                        };
                        Node::Tree(dir.finish(sub, out))
                    }
                };
                let name: Name = name.parse().expect("segments of a valid path are names");
                TreeEntry { name, node }
            })
            .collect();
        let tree = Tree::new(Set::new(entries).expect("names are unique in a directory"));
        let id = tree.id();
        out.push(StagedTree { id, tree, path });
        id
    }
}

/// Objects of an import that are not yet written.
struct Staged<'a> {
    trees: HashMap<TreeId, &'a Tree>,
    blobs: HashMap<BlobId, &'a [u8]>,
}

impl ObjectSource for Staged<'_> {
    fn tree(&self, id: &TreeId) -> Result<Tree, StoreError> {
        self.trees
            .get(id)
            .map(|t| (*t).clone())
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }
    fn blob(&self, id: &BlobId) -> Result<Vec<u8>, StoreError> {
        self.blobs
            .get(id)
            .map(|b| b.to_vec())
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_lose_their_credentials() {
        assert_eq!(
            redact_url("https://user:tok@github.com/a/b.git"),
            "https://github.com/a/b.git"
        );
        assert_eq!(
            redact_url("https://github.com/a@b"),
            "https://github.com/a@b"
        );
        assert_eq!(redact_url("git@github.com:a/b"), "git@github.com:a/b");
    }

    #[test]
    fn url_detection() {
        assert!(looks_like_url("https://x/y"));
        assert!(looks_like_url("file:///tmp/x"));
        assert!(looks_like_url("git@github.com:a/b"));
        assert!(!looks_like_url("relative/dir"));
        assert!(!looks_like_url("/abs/path:with-colon"));
    }

    #[test]
    fn par_map_preserves_order() {
        let items: Vec<u32> = (0..10_000).collect();
        let doubled = par_map(&items, |x| x * 2);
        assert_eq!(doubled, items.iter().map(|x| x * 2).collect::<Vec<_>>());
    }
}
