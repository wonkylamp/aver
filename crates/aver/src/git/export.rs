//! Git export (ARCHITECTURE.md §2.2): a snapshot's lineage becomes commits.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{BufWriter, Read, Write};
use std::process::{ChildStdin, Stdio};
use std::time::{Duration, Instant};

use aver_core::{
    BlobId, GitSha, Node, Receipt, ReceiptId, Snapshot, SnapshotId, Transition, TransitionId, Tree,
    TreeId,
};

use super::{
    earliest_transition, git_command, map_get, map_insert, mode, parse_sha, run, GitError, Probe,
    Repo, ScratchDir,
};
use crate::store::{Store, StoreError};

/// A person in a commit header: `name <email>`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    /// Refuses what cannot sit in a commit header (`<`, `>`, a newline,
    /// NUL) and empty parts.
    pub fn new(name: &str, email: &str) -> Result<Self, String> {
        let (name, email) = (name.trim(), email.trim());
        let bad = |s: &str| s.is_empty() || s.contains(['<', '>', '\n', '\0']);
        if bad(name) || bad(email) {
            return Err(format!("{name:?} <{email:?}> is not a usable Git identity"));
        }
        Ok(Self {
            name: name.to_owned(),
            email: email.to_owned(),
        })
    }
}

/// How an export writes its commits.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Author and committer of every commit the export writes. Without it,
    /// the author is the agent that made the change and the committer
    /// `aver`.
    pub identity: Option<Identity>,
    /// The branch must be new. One that exists is refused unless it is
    /// already at the commit this export writes: an accept run again after
    /// the branch was written adopts it instead of failing.
    pub new_branch: bool,
    /// The receipt this export hands off: refused unless it verified
    /// exactly the snapshot exported, so nothing but what a receipt
    /// describes leaves Aver under its name.
    pub receipt: Option<ReceiptId>,
}

#[derive(Debug, Clone)]
pub struct ExportReport {
    /// The commit the branch now points at.
    pub commit: GitSha,
    /// Commits emitted to `fast-import`; zero when the target already had
    /// the whole chain. An imported commit the target lacks is re-created,
    /// and re-created again on every export, which Git deduplicates.
    pub commits_written: usize,
    pub duration: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("snapshot {0} has no recorded transition")]
    NoLineage(SnapshotId),
    #[error("the transition chain of snapshot {0} loops back on itself")]
    Cycle(SnapshotId),
    #[error("{0:?} is not a valid branch name")]
    InvalidBranch(String),
    #[error("branch {branch} is at {current}, which the exported commit does not contain")]
    NonFastForward { branch: String, current: GitSha },
    #[error("branch {branch} already exists at {current}, not at the commit of snapshot {snapshot}; pass --branch <new name>")]
    BranchExists {
        branch: String,
        current: GitSha,
        snapshot: SnapshotId,
    },
    #[error("receipt {receipt} verified snapshot {verified}, not {snapshot}; only what a receipt verified is exported under it")]
    NotReviewed {
        receipt: ReceiptId,
        verified: SnapshotId,
        snapshot: SnapshotId,
    },
    #[error("git fast-import failed: {0}")]
    FastImport(String),
}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        Self::Git(GitError::Spawn(e))
    }
}

/// Exports `snapshot` to `branch` of the repository at `target`, creating a
/// bare repository there if the path does not exist.
///
/// Walks the earliest transition of each snapshot back to one whose commit
/// the target already has (an imported commit, or an earlier export) or to a
/// root, and writes one commit per transition through `git fast-import`:
/// author the requesting agent, committer `aver`, time the transition's
/// `at`, message the transition message plus an `Aver-Snapshot:` trailer.
/// Commits are deterministic functions of their transitions. The branch is
/// created or fast-forwarded, never rewound.
pub fn export(
    store: &Store,
    snapshot: &SnapshotId,
    target: &std::path::Path,
    branch: &str,
) -> Result<ExportReport, ExportError> {
    export_with(store, snapshot, target, branch, &ExportOptions::default())
}

/// [`export`] with options: `identity` authors and commits every commit
/// written (`aver accept` and `aver combine` commit as the user).
pub fn export_with(
    store: &Store,
    snapshot: &SnapshotId,
    target: &std::path::Path,
    branch: &str,
    opts: &ExportOptions,
) -> Result<ExportReport, ExportError> {
    let started = Instant::now();
    let branch_ref = format!("refs/heads/{branch}");
    let mut check = git_command();
    check.args(["check-ref-format", &branch_ref]);
    if run(check).is_err() {
        return Err(ExportError::InvalidBranch(branch.to_owned()));
    }
    if let Some(id) = &opts.receipt {
        let receipt: Receipt = store.get(id)?;
        if receipt.body.head_snapshot != *snapshot {
            return Err(ExportError::NotReviewed {
                receipt: *id,
                verified: receipt.body.head_snapshot,
                snapshot: *snapshot,
            });
        }
    }
    // The receipts and mappings a commit names must survive a power loss
    // before Git holds the commit.
    store.barrier()?;
    let repo = Repo::new(target);
    let probe = if target.exists() {
        Probe::open(&repo)?
    } else {
        let mut init = git_command();
        init.args([
            "init",
            "--bare",
            "--quiet",
            "--initial-branch",
            branch,
            "--",
        ])
        .arg(target);
        run(init)?;
        Probe::empty()
    };
    if target.exists() {
        sweep_temp_refs(&repo)?;
    }
    let scratch = ScratchDir::new(store, "export")?;
    let marks_path = scratch.path().join("marks");
    let tmp_ref = temp_ref();

    let mut planner = Planner {
        store,
        probe,
        stream: None,
        repo: &repo,
        marks_path: &marks_path,
        tmp_ref: &tmp_ref,
        next_mark: 1,
        memo: HashMap::new(),
        visiting: HashSet::new(),
        blob_refs: HashMap::new(),
        commits: Vec::new(),
        blobs: Vec::new(),
        identity: opts.identity.clone(),
    };
    let head = planner.commit_for(snapshot);
    let streamed = planner.stream.is_some();
    let head = match (head, planner.stream.take()) {
        (Ok(head), stream) => {
            if let Some(stream) = stream {
                stream.finish()?;
            }
            head
        }
        (Err(e), Some(stream)) => {
            stream.abort();
            return Err(e);
        }
        (Err(e), None) => return Err(e),
    };
    let Planner { commits, blobs, .. } = planner;

    // A transition-less root commit has a mark but no entry in `commits`,
    // so whether anything was streamed decides, not the commit count.
    let marks = if !streamed {
        HashMap::new()
    } else {
        let marks = read_marks(&marks_path)?;
        crate::fault::point("export.streamed")?;
        repo.run(["update-ref", "-d", &tmp_ref])?;
        marks
    };
    let resolve = |rev: &Rev| -> Result<GitSha, ExportError> {
        match rev {
            Rev::Sha(sha) => Ok(sha.clone()),
            Rev::Mark(n) => marks
                .get(n)
                .cloned()
                .ok_or_else(|| ExportError::FastImport(format!("mark :{n} was not exported"))),
        }
    };
    let commit = resolve(&head)?;

    store.tx(|tx| {
        for (transition, mark) in &commits {
            map_insert(
                &tx.sql,
                &transition.to_string(),
                &resolve(&Rev::Mark(*mark))?,
                "commit",
            )?;
        }
        for (blob, mark) in &blobs {
            map_insert(
                &tx.sql,
                &blob.to_string(),
                &resolve(&Rev::Mark(*mark))?,
                "blob",
            )?;
        }
        Ok::<_, ExportError>(())
    })?;
    crate::fault::point("export.mapped")?;

    advance_branch(
        &repo,
        branch,
        &branch_ref,
        &commit,
        opts.new_branch.then_some(snapshot),
    )?;
    crate::fault::point("export.ref")?;
    Ok(ExportReport {
        commit,
        commits_written: commits.len(),
        duration: started.elapsed(),
    })
}

/// `refs/aver/export/<pid>-<start>-<ulid>`: the fast-import ref of one
/// export, named after the process writing it so a later export can tell
/// one whose process died (and left it behind) from one still running.
fn temp_ref() -> String {
    let pid = std::process::id();
    let start = crate::run::state::process_start(pid).unwrap_or(0);
    format!("{TEMP_REFS}{pid}-{start}-{}", ulid::Ulid::generate())
}

const TEMP_REFS: &str = "refs/aver/export/";

/// Whether the export that wrote temp ref `name` (without the prefix) can
/// no longer be running: its process is gone, or, for a name from before
/// refs carried the process, it is over an hour old.
fn temp_ref_dead(name: &str) -> bool {
    let mut parts = name.splitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(pid), Some(start), Some(_)) => {
            let (Ok(pid), Ok(start)) = (pid.parse::<u32>(), start.parse::<u64>()) else {
                return true;
            };
            !crate::run::state::same_process(pid, (start != 0).then_some(start)) || zombie(pid)
        }
        _ => match name.parse::<ulid::Ulid>() {
            Ok(u) => u
                .datetime()
                .elapsed()
                .is_ok_and(|age| age > Duration::from_secs(3600)),
            Err(_) => true,
        },
    }
}

/// Whether `pid` has exited and waits to be reaped: it keeps its start
/// time, yet will never finish its export.
fn zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
        s.rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with(['Z', 'X']))
    })
}

/// Deletes the temp refs of exports that died between fast-import and
/// their cleanup, so a crash leaves nothing under `refs/aver/` for longer
/// than the next export.
fn sweep_temp_refs(repo: &Repo) -> Result<(), ExportError> {
    let (ok, out) = repo.status(["for-each-ref", "--format=%(refname)", TEMP_REFS])?;
    if !ok {
        return Ok(());
    }
    for name in String::from_utf8_lossy(&out).lines() {
        if name.strip_prefix(TEMP_REFS).is_some_and(temp_ref_dead) {
            repo.run(["update-ref", "-d", name])?;
        }
    }
    Ok(())
}

fn advance_branch(
    repo: &Repo,
    branch: &str,
    branch_ref: &str,
    commit: &GitSha,
    new_branch: Option<&SnapshotId>,
) -> Result<(), ExportError> {
    let (exists, out) = repo.status(["rev-parse", "--verify", "--quiet", branch_ref])?;
    let current = if exists {
        Some(parse_sha("rev-parse", out.trim_ascii())?)
    } else {
        None
    };
    if current.as_ref() == Some(commit) {
        return Ok(());
    }
    if let (Some(current), Some(snapshot)) = (&current, new_branch) {
        return Err(ExportError::BranchExists {
            branch: branch.to_owned(),
            current: current.clone(),
            snapshot: *snapshot,
        });
    }
    if let Some(current) = &current {
        let (is_ancestor, _) = repo.status([
            "merge-base",
            "--is-ancestor",
            current.as_str(),
            commit.as_str(),
        ])?;
        if !is_ancestor {
            return Err(ExportError::NonFastForward {
                branch: branch.to_owned(),
                current: current.clone(),
            });
        }
    }
    let zero = "0".repeat(40);
    let old = current.as_ref().map_or(zero.as_str(), GitSha::as_str);
    repo.run(["update-ref", branch_ref, commit.as_str(), old])?;
    Ok(())
}

fn read_marks(path: &std::path::Path) -> Result<HashMap<u64, GitSha>, ExportError> {
    let text = std::fs::read_to_string(path)?;
    text.lines()
        .map(|line| {
            let parsed = line
                .strip_prefix(':')
                .and_then(|l| l.split_once(' '))
                .and_then(|(n, sha)| Some((n.parse().ok()?, sha.parse().ok()?)));
            parsed.ok_or_else(|| ExportError::FastImport(format!("bad marks line {line:?}")))
        })
        .collect()
}

/// A commit-ish in the stream: an object the target has, or a mark.
#[derive(Debug, Clone)]
enum Rev {
    Sha(GitSha),
    Mark(u64),
}

impl fmt::Display for Rev {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rev::Sha(sha) => write!(f, "{sha}"),
            Rev::Mark(n) => write!(f, ":{n}"),
        }
    }
}

/// A running `git fast-import`.
struct Stream {
    child: std::process::Child,
    stdin: BufWriter<ChildStdin>,
    stderr: std::thread::JoinHandle<String>,
}

impl Stream {
    fn start(repo: &Repo, marks: &std::path::Path) -> Result<Self, ExportError> {
        let mut export_marks = std::ffi::OsString::from("--export-marks=");
        export_marks.push(marks);
        let mut child = repo
            .command(["fast-import", "--quiet", "--done", "--force"])
            .arg(export_marks)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = BufWriter::with_capacity(1 << 20, child.stdin.take().expect("piped"));
        let mut stderr = child.stderr.take().expect("piped");
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Ok(Self {
            child,
            stdin,
            stderr,
        })
    }

    fn finish(mut self) -> Result<(), ExportError> {
        self.stdin.write_all(b"done\n")?;
        self.stdin.flush()?;
        drop(self.stdin);
        let status = self.child.wait()?;
        let stderr = self.stderr.join().unwrap_or_default();
        if !status.success() {
            return Err(ExportError::FastImport(stderr.trim().to_owned()));
        }
        Ok(())
    }

    /// Kills fast-import; without `done` it commits nothing.
    fn abort(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Planner<'a> {
    store: &'a Store,
    probe: Probe,
    stream: Option<Stream>,
    repo: &'a Repo,
    marks_path: &'a std::path::Path,
    tmp_ref: &'a str,
    next_mark: u64,
    memo: HashMap<SnapshotId, Rev>,
    visiting: HashSet<SnapshotId>,
    blob_refs: HashMap<BlobId, Rev>,
    /// Commits written, by the transition they record.
    commits: Vec<(TransitionId, u64)>,
    blobs: Vec<(BlobId, u64)>,
    identity: Option<Identity>,
}

impl Planner<'_> {
    fn out(&mut self) -> Result<&mut BufWriter<ChildStdin>, ExportError> {
        if self.stream.is_none() {
            self.stream = Some(Stream::start(self.repo, self.marks_path)?);
        }
        Ok(&mut self.stream.as_mut().expect("started").stdin)
    }

    fn mark(&mut self) -> u64 {
        let mark = self.next_mark;
        self.next_mark += 1;
        mark
    }

    /// The commit the target has, or that this export writes, for `snapshot`.
    ///
    /// A depth-first walk with an explicit stack rather than recursion: a
    /// lineage is as long as the number of commits, which would overflow a
    /// thread stack after a few thousand.
    fn commit_for(&mut self, snapshot: &SnapshotId) -> Result<Rev, ExportError> {
        enum Step {
            Enter(SnapshotId),
            Exit(SnapshotId, TransitionId, Box<Transition>),
        }
        let mut stack = vec![Step::Enter(*snapshot)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(snapshot) => {
                    if self.memo.contains_key(&snapshot) {
                        continue;
                    }
                    if !self.visiting.insert(snapshot) {
                        return Err(ExportError::Cycle(snapshot));
                    }
                    let Some(transition_id) = self
                        .store
                        .read(|conn| earliest_transition(conn, &snapshot))?
                    else {
                        // A snapshot written by a pure snapshot or a verify
                        // has no transition, yet a workspace can be opened on
                        // it and committed: export it as a root commit.
                        let rev = self.write_root_commit(&snapshot)?;
                        self.visiting.remove(&snapshot);
                        self.memo.insert(snapshot, rev);
                        continue;
                    };
                    match self.in_target(&transition_id.to_string())? {
                        Some(sha) => {
                            self.visiting.remove(&snapshot);
                            self.memo.insert(snapshot, Rev::Sha(sha));
                        }
                        None => {
                            let transition: Transition = self.store.get(&transition_id)?;
                            let parents = transition.body.from.clone();
                            stack.push(Step::Exit(snapshot, transition_id, Box::new(transition)));
                            // Reversed, so the first parent's history is written first.
                            stack.extend(parents.into_iter().rev().map(Step::Enter));
                        }
                    }
                }
                Step::Exit(snapshot, transition_id, transition) => {
                    let parents: Vec<Rev> = transition
                        .body
                        .from
                        .iter()
                        .map(|parent| self.memo[parent].clone())
                        .collect();
                    let rev = self.write_commit(&transition_id, &transition, &parents)?;
                    self.visiting.remove(&snapshot);
                    self.memo.insert(snapshot, rev);
                }
            }
        }
        Ok(self.memo[snapshot].clone())
    }

    /// A parentless commit of `snapshot`'s tree for a snapshot with no
    /// transition. Its metadata is fixed (author and committer `aver` at
    /// time 0, a message of only the `Aver-Snapshot:` trailer), so it is a
    /// pure function of the snapshot and re-exporting it yields the same SHA.
    fn write_root_commit(&mut self, snapshot: &SnapshotId) -> Result<Rev, ExportError> {
        let root = self.store.get::<Snapshot>(snapshot)?.root;
        let mut ops = Vec::new();
        self.diff(None, &root, "", &mut ops)?;
        let mark = self.mark();
        let tmp_ref = self.tmp_ref.to_owned();
        let message = format!("Aver-Snapshot: {snapshot}\n");
        let out = self.out()?;
        writeln!(out, "reset {tmp_ref}")?;
        writeln!(out, "commit {tmp_ref}")?;
        writeln!(out, "mark :{mark}")?;
        writeln!(out, "author aver <aver@aver.invalid> 0 +0000")?;
        writeln!(out, "committer aver <aver@aver.invalid> 0 +0000")?;
        writeln!(out, "data {}", message.len())?;
        out.write_all(message.as_bytes())?;
        out.write_all(&ops)?;
        out.write_all(b"\n")?;
        Ok(Rev::Mark(mark))
    }

    fn write_commit(
        &mut self,
        id: &TransitionId,
        transition: &Transition,
        parents: &[Rev],
    ) -> Result<Rev, ExportError> {
        let body = &transition.body;
        let root = self.store.get::<Snapshot>(&body.to)?.root;
        let mut ops = Vec::new();
        match body.from.first() {
            Some(parent) => {
                let old_root = self.store.get::<Snapshot>(parent)?.root;
                self.diff(Some(&old_root), &root, "", &mut ops)?;
            }
            None => self.diff(None, &root, "", &mut ops)?,
        }

        let mark = self.mark();
        let tmp_ref = self.tmp_ref.to_owned();
        let secs = body.at.unix_millis().div_euclid(1000);
        let agent = body.agent;
        let mut message = body.message.trim_end().to_owned();
        if ends_with_trailers(&message) {
            message.push('\n');
        } else if !message.is_empty() {
            message.push_str("\n\n");
        }
        message.push_str(&format!("Aver-Snapshot: {}\n", body.to));

        let identity = self.identity.clone();
        let out = self.out()?;
        if parents.is_empty() {
            writeln!(out, "reset {tmp_ref}")?;
        }
        writeln!(out, "commit {tmp_ref}")?;
        writeln!(out, "mark :{mark}")?;
        let (author, committer) = match &identity {
            Some(who) => {
                let who = format!("{} <{}>", who.name, who.email);
                (who.clone(), who)
            }
            None => (
                format!("{agent} <{agent}@aver.invalid>"),
                "aver <aver@aver.invalid>".to_owned(),
            ),
        };
        writeln!(out, "author {author} {secs} +0000")?;
        writeln!(out, "committer {committer} {secs} +0000")?;
        writeln!(out, "data {}", message.len())?;
        out.write_all(message.as_bytes())?;
        for (i, parent) in parents.iter().enumerate() {
            let verb = if i == 0 { "from" } else { "merge" };
            writeln!(out, "{verb} {parent}")?;
        }
        out.write_all(&ops)?;
        out.write_all(b"\n")?;
        self.commits.push((*id, mark));
        Ok(Rev::Mark(mark))
    }

    /// Appends fast-import file commands turning `old` into `new` under
    /// `prefix`. Unchanged subtrees are skipped by id, so the cost is
    /// proportional to the change. Writes any blob the target lacks to the
    /// stream first, since blob commands cannot appear inside a commit.
    fn diff(
        &mut self,
        old: Option<&TreeId>,
        new: &TreeId,
        prefix: &str,
        ops: &mut Vec<u8>,
    ) -> Result<(), ExportError> {
        if old == Some(new) {
            return Ok(());
        }
        let old_tree = match old {
            Some(id) => self.store.get::<Tree>(id)?,
            None => Tree::new(Default::default()),
        };
        let new_tree = self.store.get::<Tree>(new)?;
        let mut old_entries = old_tree.entries.iter().peekable();
        let mut new_entries = new_tree.entries.iter().peekable();
        loop {
            let (o, n) = match (old_entries.peek(), new_entries.peek()) {
                (None, None) => break,
                (Some(o), Some(n)) => match o.name.cmp(&n.name) {
                    std::cmp::Ordering::Less => (old_entries.next(), None),
                    std::cmp::Ordering::Greater => (None, new_entries.next()),
                    std::cmp::Ordering::Equal => (old_entries.next(), new_entries.next()),
                },
                (Some(_), None) => (old_entries.next(), None),
                (None, Some(_)) => (None, new_entries.next()),
            };
            let name = o.or(n).expect("one side present").name.as_str();
            let path = join(prefix, name);
            match (o.map(|e| e.node), n.map(|e| e.node)) {
                (Some(a), Some(b)) if a == b => {}
                (Some(Node::Tree(a)), Some(Node::Tree(b))) => {
                    self.diff(Some(&a), &b, &path, ops)?;
                }
                (old_node, new_node) => {
                    if old_node.is_some() {
                        ops.extend_from_slice(b"D ");
                        quote(&path, ops);
                        ops.push(b'\n');
                    }
                    if let Some(node) = new_node {
                        self.add(&node, &path, ops)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn add(&mut self, node: &Node, path: &str, ops: &mut Vec<u8>) -> Result<(), ExportError> {
        let dataref = match node {
            Node::File(blob) | Node::Exec(blob) | Node::Symlink(blob) => self.blob(blob)?,
            Node::Tree(tree) => match self.in_target(&tree.to_string())? {
                Some(sha) => Rev::Sha(sha),
                None => return self.diff(None, tree, path, ops),
            },
        };
        ops.extend_from_slice(format!("M {} {dataref} ", mode(node)).as_bytes());
        quote(path, ops);
        ops.push(b'\n');
        Ok(())
    }

    /// The Git SHA of a mapped object, if the target already has it.
    fn in_target(&mut self, aver_id: &str) -> Result<Option<GitSha>, ExportError> {
        if self.probe.is_empty() {
            return Ok(None);
        }
        match self.store.read(|c| map_get(c, aver_id))? {
            Some(sha) if self.probe.has(&sha)? => Ok(Some(sha)),
            _ => Ok(None),
        }
    }

    /// Blobs are written without asking whether the target has them: only
    /// changed files reach here, and fast-import skips objects the
    /// repository already holds, so a probe per blob would cost more than it
    /// saves.
    fn blob(&mut self, blob: &BlobId) -> Result<Rev, ExportError> {
        if let Some(rev) = self.blob_refs.get(blob) {
            return Ok(rev.clone());
        }
        let bytes = self.store.get_raw(blob)?;
        let mark = self.mark();
        let out = self.out()?;
        writeln!(out, "blob\nmark :{mark}\ndata {}", bytes.len())?;
        out.write_all(&bytes)?;
        out.write_all(b"\n")?;
        self.blobs.push((*blob, mark));
        let rev = Rev::Mark(mark);
        self.blob_refs.insert(*blob, rev.clone());
        Ok(rev)
    }
}

/// Whether the message's last paragraph is a Git trailer block (every line
/// `Token: value`, after a subject). `git interpret-trailers` parses only
/// the last paragraph, so `Aver-Snapshot:` joins that block instead of
/// starting its own.
fn ends_with_trailers(message: &str) -> bool {
    let Some((head, last)) = message.rsplit_once("\n\n") else {
        return false;
    };
    !head.trim().is_empty()
        && !last.is_empty()
        && last.lines().all(|line| {
            line.split_once(": ").is_some_and(|(token, _)| {
                !token.is_empty() && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
        })
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

/// C-style quoting, which fast-import accepts for every path.
fn quote(path: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for &b in path.as_bytes() {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            0x00..=0x1f | 0x7f => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::{ends_with_trailers, quote};

    #[test]
    fn a_trailer_paragraph_counts_only_after_a_subject() {
        assert!(ends_with_trailers(
            "Fix\n\nbody\n\nAver-Task: t\nAver-Receipt: r"
        ));
        assert!(!ends_with_trailers("Fix\n\nbody text: has a colon"));
        assert!(!ends_with_trailers("Aver-Task: t"));
        assert!(!ends_with_trailers("Fix the bug"));
    }

    #[test]
    fn quoting_escapes_quotes_backslashes_and_control_bytes() {
        let mut out = Vec::new();
        quote("a\"b\\c\nd\te", &mut out);
        assert_eq!(out, b"\"a\\\"b\\\\c\\nd\\011e\"");
    }
}
