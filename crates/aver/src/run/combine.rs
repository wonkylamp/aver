//! `aver combine` (§2.11): the winners of runs on different tasks, all
//! descended from the same base, merged into one branch.
//!
//! [`Api::merge`](super::api::Api::merge) three-way merges the heads onto
//! the shared root; a conflict stops it with the files and lines involved,
//! never a guessed resolution. A clean merge is verified against every task
//! its heads contain, a stacked run's prerequisites included, and only when
//! every receipt passes are the runs not accepted yet accepted and the
//! result exported (one commit, child of the base commit, authored as the
//! user). Nothing is accepted or exported before that, so a conflict or a
//! failing task leaves no branch behind.
//!
//! With `partial`, tasks are kept greedily by task number, each when it
//! merges cleanly with the heads kept before it (a stacked task bringing
//! its prerequisites); the rest, and a task that fails on the merge, are
//! left out, each with its reason, and what is kept, one task or more, is
//! merged, verified and exported.
//!
//! Before the first accept, the plan (the branch and each input's branch)
//! is written to `combines/pending-<runs>.json`; a combine of the same runs
//! run again after a crash takes the same names, and a branch already
//! written at the snapshot it was verified on is adopted, not refused.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use aver_core::{AttemptId, ReceiptId, SnapshotId, TaskId, Verdict};
use serde::{Deserialize, Serialize};

use super::accept::{self, branch_for, check_branch, default_branch, AcceptOptions};
use super::api::Api;
use super::batch::record::{self, CombineRecord, CombineStatus, Excluded, ExcludedWhy};
use super::compare;
use super::report::{fence, one_line, FENCE_NOTE};
use super::state::{RunState, StackOnto};
use super::supervisor::RunError;
use crate::keys::{AgentKey, Home};
use crate::workspace::merge::{Conflict, ConflictKind};

/// Another accepted run that `aver combine` could merge with this one.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub run: String,
    pub title: String,
    pub branch: Option<String>,
    pub clean: bool,
    pub conflicts: usize,
}

/// Accepted runs of other tasks on `run`'s repository and base, each with
/// whether it merges cleanly with `run`'s winner (no workspace is opened).
pub async fn candidates(
    api: &Api,
    home: &Home,
    admin: &AgentKey,
    run: &RunState,
) -> Vec<Candidate> {
    let Some(mine) = run.decided.as_ref().and_then(|d| d.head) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for other in RunState::all(home) {
        let Some(head) = other.decided.as_ref().and_then(|d| d.head) else {
            continue;
        };
        if other.id == run.id
            || other.discarded
            || other.repo != run.repo
            || other.base != run.base
            || other.task == run.task
        {
            continue;
        }
        if let Ok(m) = api
            .merge(&admin.signing_key, &run.base, &[head, mine], false)
            .await
        {
            out.push(Candidate {
                run: other.id.clone(),
                title: other.title.clone(),
                branch: other.decided.as_ref().and_then(|d| d.branch.clone()),
                clean: m.clean,
                conflicts: m.conflicts.len(),
            });
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct CombineOptions {
    /// The branch to create (default `aver/combine-<titles>`).
    pub branch: Option<String>,
}

/// One task's verdict on the combined snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct TaskVerdict {
    pub run: String,
    pub task: TaskId,
    pub title: String,
    pub receipt: ReceiptId,
    pub verdict: Verdict,
    /// Its number in its batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// The input run that carries it, when it is a prerequisite.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// `run` names a workspace directory made outside `aver run`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workspace: bool,
}

impl TaskVerdict {
    /// `#2`, `run 7KQ2X1` or `workspace calc-k3x9`.
    pub fn name(&self) -> String {
        name_of(self.number, &self.run, self.workspace)
    }

    fn task(&self) -> RunRef {
        RunRef {
            number: self.number,
            run: self.run.clone(),
            title: self.title.clone(),
            workspace: self.workspace,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Combined {
    /// Nothing merged; `labels[i]` names head `i` of each conflict.
    Conflicts {
        conflicts: Vec<Conflict>,
        labels: Vec<String>,
    },
    /// Merged, but not every task passes on the result; nothing exported.
    NotPass {
        snapshot: SnapshotId,
        verdicts: Vec<TaskVerdict>,
    },
    Exported {
        branch: String,
        commit: String,
        snapshot: SnapshotId,
        merged: Vec<String>,
        verdicts: Vec<TaskVerdict>,
    },
}

/// Finds the run `name` names: a run id, or the attempt id of its winner.
pub fn find_run(home: &Home, name: &str) -> Result<RunState, RunError> {
    if name.starts_with("att_") {
        return RunState::all(home)
            .into_iter()
            .find(|r| {
                r.decided
                    .as_ref()
                    .is_some_and(|d| d.winner.to_string() == name)
            })
            .ok_or_else(|| RunError::Invalid(format!("no accepted run has {name} as its winner")));
    }
    RunState::find(home, Some(name), None)
        .map_err(|e| RunError::Invalid(format!("run {name}: {e}")))
}

/// What `aver combine` is given for one task.
#[derive(Debug, Clone)]
pub enum Input {
    /// A run whose winner was accepted: its committed snapshot.
    Accepted(RunState),
    /// A run not accepted yet and its best attempt: the attempt's current
    /// snapshot, accepted only once the combine passes.
    Best(RunState, u32),
    /// The attempt of a workspace made outside `aver run` (`aver workspace
    /// new`, `claude --worktree`): its directory as it is now, or once it
    /// was accepted, the snapshot its last receipt verified. `repo` is
    /// where to export when its directory names none.
    Solo {
        attempt: AttemptId,
        repo: std::path::PathBuf,
    },
}

/// A task whose change a head carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contained {
    pub run: String,
    pub task: TaskId,
    pub title: String,
    /// Its number in its batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// The snapshot its run started from: its task is verified against it.
    #[serde(skip)]
    pub base: SnapshotId,
    /// The attempt whose change it is.
    pub attempt: AttemptId,
    /// The input run whose head brings it in, when it is that run's
    /// prerequisite rather than the input itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Its run was accepted at `REVIEW`: a person approved its changed
    /// test expectations, so a merge that is `REVIEW` for them passes.
    #[serde(skip)]
    pub approved_review: bool,
    /// `run` names a workspace directory made outside `aver run`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workspace: bool,
}

impl Contained {
    /// `#2`, `run 7KQ2X1` or `workspace calc-k3x9`.
    pub fn name(&self) -> String {
        name_of(self.number, &self.run, self.workspace)
    }
}

fn name_of(number: Option<u32>, run: &str, workspace: bool) -> String {
    match number {
        Some(n) => format!("#{n}"),
        None if workspace => format!("workspace {run}"),
        None => format!("run {run}"),
    }
}

/// Where a head to merge comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// A run's winner, or its best attempt `accept`, accepted once the
    /// combine passes.
    Run {
        run: Box<RunState>,
        accept: Option<u32>,
    },
    /// A workspace made outside `aver run`. The combine never accepts it:
    /// its directory stays as it is, for `aver accept` there.
    Solo(Solo),
}

/// A workspace made outside `aver run`, as a combine input.
#[derive(Debug, Clone)]
pub struct Solo {
    /// Its directory's name, or its attempt id once the directory is gone.
    pub label: String,
    pub title: String,
    pub repo: std::path::PathBuf,
}

/// One head to merge: a run's winner, its best attempt, or a solo
/// workspace.
#[derive(Debug, Clone)]
pub struct CombineInput {
    pub source: Source,
    pub head: SnapshotId,
    /// The base every run in its chain descends from.
    pub root: SnapshotId,
    /// `root`'s commit.
    pub base_commit: String,
    /// Every task the head carries: its prerequisites first (a stacked
    /// run's), its own task last.
    pub contains: Vec<Contained>,
    /// The `REVIEW` receipt of an attempt not accepted yet: only
    /// `--approve-review` lets the combine accept it.
    pub review: Option<ReceiptId>,
}

impl CombineInput {
    /// The run's id, or the solo workspace's label.
    pub fn id(&self) -> &str {
        match &self.source {
            Source::Run { run, .. } => &run.id,
            Source::Solo(s) => &s.label,
        }
    }

    pub fn number(&self) -> Option<u32> {
        match &self.source {
            Source::Run { run, .. } => run.batch.as_ref().map(|b| b.number),
            Source::Solo(_) => None,
        }
    }

    pub fn title(&self) -> &str {
        match &self.source {
            Source::Run { run, .. } => &run.title,
            Source::Solo(s) => &s.title,
        }
    }

    pub fn repo(&self) -> &std::path::Path {
        match &self.source {
            Source::Run { run, .. } => &run.repo,
            Source::Solo(s) => &s.repo,
        }
    }

    fn solo(&self) -> bool {
        matches!(self.source, Source::Solo(_))
    }

    /// The attempt to accept once the combine passes: a run's best
    /// attempt not accepted yet.
    fn to_accept(&self) -> Option<(&RunState, u32)> {
        match &self.source {
            Source::Run {
                run,
                accept: Some(n),
            } => Some((run, *n)),
            _ => None,
        }
    }

    pub fn run_ref(&self) -> RunRef {
        RunRef {
            number: self.number(),
            run: self.id().to_owned(),
            title: self.title().to_owned(),
            workspace: self.solo(),
        }
    }

    /// `#2`, `run 7KQ2X1` or `workspace calc-k3x9`.
    pub fn name(&self) -> String {
        name_of(self.number(), self.id(), self.solo())
    }

    /// `#2 ("Add double")`.
    pub fn label(&self) -> String {
        format!("{} ({:?})", self.name(), self.title())
    }

    fn order(&self) -> u32 {
        self.number().unwrap_or(u32::MAX)
    }
}

/// An input as it is now: ready to merge, or not passing.
#[derive(Debug, Clone)]
pub enum Resolved {
    Ready(Box<CombineInput>),
    /// Its best attempt's receipt for its current snapshot is not `PASS`
    /// or `REVIEW`.
    NotPass {
        task: RunRef,
        verdict: Verdict,
        receipt: ReceiptId,
    },
}

/// The root of a run's chain: the base of the first run in it.
pub fn root_of(run: &RunState) -> SnapshotId {
    run.stacked_on.as_ref().map_or(run.base, |s| s.root)
}

/// Whether `run` was accepted at `REVIEW`, a person's approval.
fn approved_review(run: &RunState) -> bool {
    run.decided.as_ref().and_then(|d| d.verdict) == Some(Verdict::Review)
}

/// The runs `run` was stacked on, directly.
fn prerequisites(run: &RunState) -> Vec<String> {
    match run.stacked_on.as_ref().map(|s| &s.onto) {
        Some(StackOnto::Run { run, .. }) => vec![run.clone()],
        Some(StackOnto::Combine { runs, .. }) => runs.clone(),
        None => Vec::new(),
    }
}

/// Every task `run`'s head carries, prerequisites first, through `load`.
fn chain(
    load: &dyn Fn(&str) -> Option<RunState>,
    run: &RunState,
    own: AttemptId,
) -> Result<Vec<Contained>, RunError> {
    fn walk(
        load: &dyn Fn(&str) -> Option<RunState>,
        of: &RunState,
        via: &str,
        seen: &mut BTreeSet<String>,
        out: &mut Vec<Contained>,
    ) -> Result<(), RunError> {
        for id in prerequisites(of) {
            if !seen.insert(id.clone()) {
                continue;
            }
            let pre = load(&id).ok_or_else(|| {
                RunError::Invalid(format!(
                    "run {} is stacked on run {id}, which combine cannot find; it verifies every task a head carries, so name that run too",
                    of.id
                ))
            })?;
            let winner = pre.decided.as_ref().map(|d| d.winner).ok_or_else(|| {
                RunError::Invalid(format!(
                    "run {} is stacked on run {id}, which has no accepted attempt",
                    of.id
                ))
            })?;
            walk(load, &pre, via, seen, out)?;
            out.push(Contained {
                run: pre.id.clone(),
                task: pre.task,
                title: pre.title.clone(),
                number: pre.batch.as_ref().map(|b| b.number),
                base: pre.base,
                attempt: winner,
                via: Some(via.to_owned()),
                approved_review: approved_review(&pre),
                workspace: false,
            });
        }
        Ok(())
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::from([run.id.clone()]);
    walk(load, run, &run.id, &mut seen, &mut out)?;
    out.push(Contained {
        run: run.id.clone(),
        task: run.task,
        title: run.title.clone(),
        number: run.batch.as_ref().map(|b| b.number),
        base: run.base,
        attempt: own,
        via: None,
        approved_review: approved_review(run),
        workspace: false,
    });
    Ok(out)
}

fn accepted_input(
    load: &dyn Fn(&str) -> Option<RunState>,
    run: RunState,
) -> Result<CombineInput, RunError> {
    let decided = run.decided.as_ref().ok_or_else(|| {
        RunError::Invalid(format!(
            "run {} has no accepted attempt; aver accept one first",
            run.id
        ))
    })?;
    let head = decided.head.ok_or_else(|| {
        RunError::Invalid(format!(
            "run {} was accepted before aver recorded its snapshot; accept it again with a newer aver",
            run.id
        ))
    })?;
    let contains = chain(load, &run, decided.winner)?;
    Ok(CombineInput {
        root: root_of(&run),
        base_commit: root_commit(load, &run),
        head,
        contains,
        review: None,
        source: Source::Run {
            run: Box::new(run),
            accept: None,
        },
    })
}

/// Resolves `input`: an accepted run's committed snapshot, or the current
/// snapshot of a run's best attempt with a receipt for exactly that
/// snapshot (verifying it once when the latest receipt is stale), and the
/// tasks its head carries.
pub async fn resolve(
    api: &Api,
    home: &Home,
    admin: &AgentKey,
    input: &Input,
) -> Result<Resolved, RunError> {
    let load = |id: &str| RunState::load(home, id).ok();
    let (run, index) = match input {
        Input::Accepted(run) => {
            return Ok(Resolved::Ready(Box::new(accepted_input(
                &load,
                run.clone(),
            )?)))
        }
        Input::Best(run, index) => (run, *index),
        Input::Solo { attempt, repo } => {
            return resolve_solo(api, home, admin, attempt, repo).await
        }
    };
    if run.decided.is_some() {
        return Ok(Resolved::Ready(Box::new(accepted_input(
            &load,
            run.clone(),
        )?)));
    }
    if run.discarded {
        return Err(RunError::Invalid(format!("run {} was discarded", run.id)));
    }
    if run.supervisor_alive() {
        return Err(RunError::Invalid(format!(
            "run {} is still running; combine it once it ends",
            run.id
        )));
    }
    let agent = run
        .agent(&index.to_string())
        .ok_or_else(|| RunError::Invalid(format!("run {} has no attempt {index}", run.id)))?;
    let key = compare::agent_key(home, agent).ok_or_else(|| {
        RunError::Invalid(format!(
            "the key of attempt {index} of run {} is gone",
            run.id
        ))
    })?;
    let current = api.snapshot(&key.signing_key, &agent.workspace).await?;
    if current == run.base {
        return Err(RunError::Invalid(format!(
            "attempt {index} of run {} changed nothing",
            run.id
        )));
    }
    let attempt = api.attempt(&admin.signing_key, &agent.attempt).await?;
    let latest = match attempt.receipts.last() {
        Some(id) => Some(api.receipt(id).await?),
        None => None,
    };
    let v = match latest {
        Some(v) if v.receipt.body.head_snapshot == current => v,
        _ => {
            api.verify(&key.signing_key, &run.task, &agent.workspace, false)
                .await?
        }
    };
    let verdict = v.receipt.body.verdict;
    if !passes(verdict) {
        return Ok(Resolved::NotPass {
            task: RunRef::of(run),
            verdict,
            receipt: v.receipt_id,
        });
    }
    let contains = chain(&load, run, agent.attempt)?;
    Ok(Resolved::Ready(Box::new(CombineInput {
        root: root_of(run),
        base_commit: root_commit(&load, run),
        head: current,
        contains,
        review: (verdict == Verdict::Review).then_some(v.receipt_id),
        source: Source::Run {
            run: Box::new(run.clone()),
            accept: Some(index),
        },
    })))
}

/// Finds the attempt `name` names for a workspace made outside `aver
/// run`: its directory's name, its attempt id, or the branch `aver accept`
/// exported from it (read from the `Aver-Attempt:` line of the branch's
/// commit in `repo`).
pub fn find_solo(
    api: &Api,
    home: &Home,
    repo: &std::path::Path,
    name: &str,
) -> Result<AttemptId, RunError> {
    let attempt = if let Ok(id) = name.parse::<AttemptId>() {
        id
    } else if name.starts_with("aver/") {
        let message = git_message(repo, name)
            .ok_or_else(|| RunError::Invalid(format!("no branch {name} in {}", repo.display())))?;
        let ids: Vec<&str> = message
            .lines()
            .filter_map(|l| l.strip_prefix("Aver-Attempt: "))
            .collect();
        let [id] = ids.as_slice() else {
            return Err(RunError::Invalid(format!(
                "branch {name} holds {} attempts; combine takes a branch aver accept exported from one workspace",
                ids.len()
            )));
        };
        id.trim()
            .parse()
            .map_err(|e| RunError::Invalid(format!("branch {name}: {e}")))?
    } else {
        let mats = api
            .services()
            .workspaces
            .disk()
            .materializations(false)
            .map_err(|e| RunError::Invalid(e.to_string()))?;
        let m = mats
            .iter()
            .find(|m| m.dir.file_name().is_some_and(|n| n == name) || m.dir == std::path::Path::new(name))
            .ok_or_else(|| {
                RunError::Invalid(format!(
                    "{name} is no run, task number, attempt, aver/ branch or workspace directory (aver status and aver workspace ls list them)"
                ))
            })?;
        let file = workspace_file(&m.dir)?;
        file.attempt.ok_or_else(|| {
            RunError::Invalid(format!(
                "workspace {name} belongs to no task, so there is nothing to verify it against"
            ))
        })?
    };
    if let Some(run) = RunState::all(home)
        .into_iter()
        .find(|r| r.agents.iter().any(|a| a.attempt == attempt))
    {
        return Err(RunError::Invalid(format!(
            "{name} is an attempt of run {}; name the run (or its task number) instead",
            run.id
        )));
    }
    Ok(attempt)
}

fn workspace_file(dir: &std::path::Path) -> Result<crate::workspace::WorkspaceFile, RunError> {
    crate::workspace::WorkspaceFile::find(dir)?
        .map(|(_, f)| f)
        .ok_or_else(|| RunError::Invalid(format!("{} has no .aver/workspace.toml", dir.display())))
}

fn git_message(repo: &std::path::Path, rev: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--format=%B", rev, "--"])
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Resolves a solo workspace's attempt: while it is open and its directory
/// live, the directory's current snapshot with a receipt for exactly that
/// snapshot (verified once when the latest is stale, as [`resolve`] does
/// for a run's best attempt); otherwise the snapshot its last receipt
/// verified.
async fn resolve_solo(
    api: &Api,
    home: &Home,
    admin: &AgentKey,
    id: &AttemptId,
    repo: &std::path::Path,
) -> Result<Resolved, RunError> {
    let attempt = api.attempt(&admin.signing_key, id).await?;
    let disk = api.services().workspaces.disk().clone();
    let mat = disk
        .materialization(&attempt.workspace)
        .map_err(|e| RunError::Invalid(e.to_string()))?;
    let label = mat
        .as_ref()
        .and_then(|m| m.dir.file_name())
        .map_or_else(|| id.to_string(), |n| n.to_string_lossy().into_owned());
    let body = api.task(&attempt.task).await?.body;
    let title = accept::solo_names(Some(&body), &attempt.workspace, &label, None, None).0;
    let live = mat
        .as_ref()
        .filter(|m| m.state == crate::disk::MatState::Live);
    let repo = live
        .and_then(|m| m.repo.clone())
        .or_else(|| accept::view_repo(home, &attempt.workspace))
        .unwrap_or_else(|| repo.to_owned());
    let open = attempt.state == crate::tasks::AttemptState::Open;
    let latest = match attempt.receipts.last() {
        Some(r) => Some(api.receipt(r).await?),
        None => None,
    };
    let (head, v) = match live.filter(|_| open) {
        Some(m) => {
            let file = workspace_file(&m.dir)?;
            let name = file
                .key
                .as_deref()
                .map(str::parse::<crate::agents::AgentName>)
                .transpose()
                .map_err(|e| RunError::Invalid(format!("{}: {e}", m.dir.display())))?;
            let key = crate::services::key_in(home, name.as_ref())
                .map_err(|e| RunError::Invalid(format!("workspace {label}: {e}")))?;
            let current = api.snapshot(&key.signing_key, &attempt.workspace).await?;
            if current == attempt.base {
                return Err(RunError::Invalid(format!(
                    "workspace {label} changed nothing"
                )));
            }
            let v = match latest {
                Some(v) if v.receipt.body.head_snapshot == current => v,
                _ => {
                    api.verify(&key.signing_key, &attempt.task, &attempt.workspace, false)
                        .await?
                }
            };
            (current, v)
        }
        None => {
            let v = latest.ok_or_else(|| {
                RunError::Invalid(format!(
                    "workspace {label} has no live directory and was never verified"
                ))
            })?;
            (v.receipt.body.head_snapshot, v)
        }
    };
    let verdict = v.receipt.body.verdict;
    let solo = Solo {
        label: label.clone(),
        title: title.clone(),
        repo,
    };
    if !passes(verdict) {
        return Ok(Resolved::NotPass {
            task: RunRef {
                number: None,
                run: label,
                title,
                workspace: true,
            },
            verdict,
            receipt: v.receipt_id,
        });
    }
    let base_commit = crate::git::imported_commit(&api.services().store, &attempt.base)
        .map_err(|e| RunError::Invalid(e.to_string()))?
        .map(|c| c.to_string())
        .unwrap_or_default();
    let review = verdict == Verdict::Review;
    Ok(Resolved::Ready(Box::new(CombineInput {
        head,
        root: attempt.base,
        base_commit,
        contains: vec![Contained {
            run: label,
            task: attempt.task,
            title,
            number: None,
            base: attempt.base,
            attempt: *id,
            via: None,
            approved_review: review && !open,
            workspace: true,
        }],
        review: (review && open).then_some(v.receipt_id),
        source: Source::Solo(solo),
    })))
}

/// `PASS`, or `REVIEW`: a pass whose changed test expectations a person
/// approves. Whether a `REVIEW` was approved is [`Combine::passes`]'s to
/// say.
fn passes(v: Verdict) -> bool {
    matches!(v, Verdict::Pass | Verdict::Review)
}

/// An input left out because another one's head already carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Absorbed {
    /// The run left out.
    pub run: String,
    /// The input whose head carries it.
    pub by: String,
}

/// For each of `inputs` (run id, the runs its head carries), the input
/// that absorbs it: one that carries it, or an earlier one for the same
/// run.
fn absorbers(inputs: &[(&str, Vec<&str>)]) -> Vec<Option<usize>> {
    inputs
        .iter()
        .enumerate()
        .map(|(i, (run, _))| {
            inputs.iter().enumerate().position(|(j, (other, carries))| {
                if other == run {
                    j < i
                } else {
                    carries.contains(run)
                }
            })
        })
        .collect()
}

/// Leaves out every input another input's head already carries (a
/// prerequisite named beside the task stacked on it, or a run named twice),
/// so each change is merged once, onto the shared root. The task stays in
/// the result through the head that carries it, and is verified there.
pub fn dedupe_chains(inputs: &mut Vec<CombineInput>) -> Vec<Absorbed> {
    let keys: Vec<(&str, Vec<&str>)> = inputs
        .iter()
        .map(|i| (i.id(), i.contains.iter().map(|c| c.run.as_str()).collect()))
        .collect();
    let by = absorbers(&keys);
    let absorbed: Vec<Absorbed> = by
        .iter()
        .enumerate()
        .filter_map(|(i, j)| {
            j.map(|j| Absorbed {
                run: inputs[i].id().to_owned(),
                by: inputs[j].id().to_owned(),
            })
        })
        .collect();
    let mut i = 0;
    inputs.retain(|_| {
        i += 1;
        by[i - 1].is_none()
    });
    absorbed
}

/// A run named in a report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    pub run: String,
    pub title: String,
    /// `run` names a workspace directory made outside `aver run`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workspace: bool,
}

impl RunRef {
    fn of(run: &RunState) -> Self {
        Self {
            number: run.batch.as_ref().map(|b| b.number),
            run: run.id.clone(),
            title: run.title.clone(),
            workspace: false,
        }
    }

    fn of_task(c: &Contained) -> Self {
        Self {
            number: c.number,
            run: c.run.clone(),
            title: c.title.clone(),
            workspace: c.workspace,
        }
    }

    /// `#2`, `run 7KQ2X1` or `workspace calc-k3x9`.
    pub fn name(&self) -> String {
        name_of(self.number, &self.run, self.workspace)
    }

    /// `#2 ("Add double")`.
    pub fn label(&self) -> String {
        format!("{} ({:?})", self.name(), self.title)
    }
}

/// Why a task was left out.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LeftWhy {
    /// Its change conflicts with `with`'s at `paths`.
    Conflict { with: RunRef, paths: Vec<String> },
    /// Task `task` (its own, or a prerequisite its head carries) is
    /// `verdict` on the merge, or, without a merge, on its own attempt.
    NotPass {
        task: RunRef,
        verdict: Verdict,
        receipt: ReceiptId,
    },
}

/// A task left out of the result, with the prerequisites its head carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Left {
    #[serde(flatten)]
    pub task: RunRef,
    pub why: LeftWhy,
    /// Prerequisites its head carries, left out with it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub carries: Vec<RunRef>,
}

/// What a combine came to.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub status: CombineStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotId>,
    /// The commit the chain of every run in it starts from.
    pub base_commit: String,
    /// The exported commit's parent: `base_commit`, or for one stacked
    /// head, the commit of the task under it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Paths changed by more than one head and merged line by line.
    pub merged: Vec<String>,
    /// Every task in the exported result.
    pub included: Vec<Contained>,
    pub excluded: Vec<Left>,
    /// The verdicts of the last merge tried.
    pub verdicts: Vec<TaskVerdict>,
    /// The conflicts of the last merge tried; `labels[i]` names its head
    /// `i`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<Conflict>,
    #[serde(skip)]
    pub labels: Vec<String>,
    /// Inputs another input's head carried, merged once through it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub absorbed: Vec<Absorbed>,
    /// Runs accepted once the result passed, and their branches.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<(RunRef, String)>,
    /// Tasks whose `REVIEW` the exported result holds on the strength of
    /// `--approve-review` alone (not an earlier `aver accept`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub approved_review: Vec<RunRef>,
}

/// How to combine.
pub struct Combine<'a> {
    pub api: &'a Api,
    /// Needed to accept runs that were not accepted yet and to write the
    /// batch's combine record.
    pub home: Option<&'a Home>,
    pub admin: &'a AgentKey,
    pub branch: Option<String>,
    /// Leave out what conflicts or fails instead of exporting nothing.
    pub partial: bool,
    /// The batch whose `combine.json` records the outcome.
    pub batch: Option<String>,
    /// Accept `REVIEW` (`--approve-review`): inputs whose attempt is
    /// `REVIEW` and a merge that is `REVIEW`. Without it only work a person
    /// already accepted at `REVIEW` may make the merge `REVIEW`, and an
    /// input not accepted yet must be `PASS`.
    pub approve_review: bool,
    /// Called with the directory of each input still to accept, after the
    /// merge is verified and before any accept: tests stand in for an edit
    /// that lands there.
    #[doc(hidden)]
    pub before_accept: Option<fn(&std::path::Path)>,
}

/// One merge of `inputs` and the verification of every task they carry.
enum Tried {
    Conflicts(Vec<Conflict>),
    Verified {
        ws: aver_core::WorkspaceId,
        snapshot: SnapshotId,
        merged: Vec<String>,
        verdicts: Vec<TaskVerdict>,
    },
}

impl Combine<'_> {
    /// Whether a task's verdict on a merge of `inputs` lets it be exported:
    /// `PASS`, or `REVIEW` when the user passed `--approve-review` or the
    /// merge carries a task a person accepted at `REVIEW` (a changed test
    /// expectation shows in every task's verify of the merge).
    fn passes(&self, v: Verdict, inputs: &[CombineInput]) -> bool {
        match v {
            Verdict::Pass => true,
            Verdict::Review => {
                self.approve_review
                    || inputs
                        .iter()
                        .flat_map(|i| &i.contains)
                        .any(|c| c.approved_review)
            }
            _ => false,
        }
    }

    async fn merge(
        &self,
        root: &SnapshotId,
        inputs: &[&CombineInput],
        open: bool,
    ) -> Result<super::api::MergeOutcome, RunError> {
        let mut heads: Vec<SnapshotId> = inputs.iter().map(|i| i.head).collect();
        // A merge takes two heads or more; one head merged with itself
        // is that head.
        if heads.len() == 1 {
            heads.push(heads[0]);
        }
        Ok(self
            .api
            .merge(&self.admin.signing_key, root, &heads, open)
            .await?)
    }

    async fn try_merge(
        &self,
        root: &SnapshotId,
        inputs: &[&CombineInput],
    ) -> Result<Tried, RunError> {
        let merged = self.merge(root, inputs, true).await?;
        if !merged.clean {
            return Ok(Tried::Conflicts(merged.conflicts));
        }
        let (Some(ws), Some(snapshot)) = (merged.workspace, merged.snapshot) else {
            return Err(RunError::Invalid(
                "the merge was clean but opened no workspace".into(),
            ));
        };
        let mut verdicts = Vec::new();
        for c in union(inputs) {
            let v = match self
                .api
                .verify_head(&self.admin.signing_key, &c.task, &snapshot, &c.base)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    let _ = self.api.discard(&self.admin.signing_key, &ws).await;
                    return Err(e.into());
                }
            };
            verdicts.push(TaskVerdict {
                run: c.run.clone(),
                task: c.task,
                title: c.title.clone(),
                receipt: v.receipt_id,
                verdict: v.receipt.body.verdict,
                number: c.number,
                via: c.via.clone(),
                workspace: c.workspace,
            });
        }
        crate::fault::point("combine.merged")?;
        Ok(Tried::Verified {
            ws,
            snapshot,
            merged: merged.merged,
            verdicts,
        })
    }

    /// Merges `inputs` and, when every task they carry passes on the
    /// result, accepts those not accepted yet and exports it. With
    /// `partial`, a merge of everything that does not pass is followed by
    /// [`Combine::greedy`]. Every outcome is written to the batch's combine
    /// record.
    pub async fn run(&self, resolved: Vec<Resolved>) -> Result<Report, RunError> {
        let mut candidates = Vec::new();
        let mut excluded = Vec::new();
        for r in resolved {
            match r {
                Resolved::Ready(i) if !self.approve_review && i.review.is_some() => {
                    excluded.push(Left {
                        task: i.run_ref(),
                        why: LeftWhy::NotPass {
                            task: i.run_ref(),
                            verdict: Verdict::Review,
                            receipt: i.review.expect("matched"),
                        },
                        carries: Vec::new(),
                    })
                }
                Resolved::Ready(i) => candidates.push(*i),
                Resolved::NotPass {
                    task,
                    verdict,
                    receipt,
                } => excluded.push(Left {
                    task: task.clone(),
                    why: LeftWhy::NotPass {
                        task,
                        verdict,
                        receipt,
                    },
                    carries: Vec::new(),
                }),
            }
        }
        candidates.sort_by_key(CombineInput::order);
        let mut report = Report {
            status: CombineStatus::NotPass,
            branch: None,
            commit: None,
            snapshot: None,
            base_commit: String::new(),
            parent: None,
            merged: Vec::new(),
            included: Vec::new(),
            excluded,
            verdicts: Vec::new(),
            conflicts: Vec::new(),
            labels: Vec::new(),
            absorbed: Vec::new(),
            accepted: Vec::new(),
            approved_review: Vec::new(),
        };
        let Some(first) = candidates.first() else {
            return self.finish(report);
        };
        let root = first.root;
        report.base_commit = base_commit_of(first);
        for c in &candidates[1..] {
            if !same_path(c.repo(), first.repo()) || c.root != root {
                return Err(RunError::Invalid(format!(
                    "{} and {} started from different commits ({} and {}); combine needs a shared base",
                    first.name(),
                    c.name(),
                    report.base_commit,
                    base_commit_of(c)
                )));
            }
        }
        if !report.excluded.is_empty() && !self.partial {
            return self.finish(report);
        }
        let mut inputs = candidates.clone();
        report.absorbed = dedupe_chains(&mut inputs);
        check_tasks(&inputs)?;
        let refs: Vec<&CombineInput> = inputs.iter().collect();
        if !self.partial && union(&refs).len() < 2 {
            return Err(RunError::Invalid(
                "combine takes two or more tasks (or one stacked on another)".into(),
            ));
        }
        report.labels = refs.iter().map(|i| i.label()).collect();
        match self.try_merge(&root, &refs).await? {
            Tried::Conflicts(conflicts) => {
                if self.partial {
                    return self.greedy(report, candidates).await;
                }
                for ((i, with), paths) in conflict_pairs(&conflicts) {
                    report.excluded.push(left(
                        &inputs[i],
                        LeftWhy::Conflict {
                            with: inputs[with].run_ref(),
                            paths,
                        },
                    ));
                }
                report.conflicts = conflicts;
                report.status = CombineStatus::Conflicts;
                self.finish(report)
            }
            Tried::Verified {
                ws,
                snapshot,
                merged,
                verdicts,
            } => {
                let failing: Vec<&TaskVerdict> = verdicts
                    .iter()
                    .filter(|v| !self.passes(v.verdict, &inputs))
                    .collect();
                if failing.is_empty() {
                    report.verdicts = verdicts;
                    report.merged = merged;
                    crate::fault::point("combine.verified")?;
                    return self.export(report, &inputs, &ws, &snapshot).await;
                }
                let _ = self.api.discard(&self.admin.signing_key, &ws).await;
                if self.partial {
                    return self.greedy(report, candidates).await;
                }
                let mut seen = BTreeSet::new();
                for v in &failing {
                    let i = inputs
                        .iter()
                        .position(|i| i.contains.iter().any(|c| c.run == v.run))
                        .expect("every verdict is of a contained task");
                    if seen.insert(i) {
                        report.excluded.push(left(&inputs[i], not_pass(v)));
                    }
                }
                report.verdicts = verdicts;
                report.snapshot = Some(snapshot);
                report.status = CombineStatus::NotPass;
                self.finish(report)
            }
        }
    }

    /// `--partial` once the merge of everything did not pass: inputs taken
    /// in number order, each kept only when it merges cleanly with the ones
    /// kept so far and every task of that merge passes on it, so an input
    /// is left out only for what it does with the kept ones, never for
    /// one already left out. A dependent carries its prerequisites and
    /// replaces them; an input a kept head carries is in through it. Each
    /// step is a merge and a verify of every task in it (runs of an
    /// identical tree come from the run cache), and the last passing merge
    /// is exported, even of one input.
    async fn greedy(
        &self,
        mut report: Report,
        candidates: Vec<CombineInput>,
    ) -> Result<Report, RunError> {
        let root = candidates[0].root;
        let carries =
            |by: &CombineInput, id: &str| by.id() != id && by.contains.iter().any(|c| c.run == id);
        let mut kept: Vec<CombineInput> = Vec::new();
        let mut absorbed: Vec<Absorbed> = Vec::new();
        let mut best: Option<Tried> = None;
        for input in candidates {
            if let Some(by) = kept.iter().find(|k| carries(k, input.id())) {
                absorbed.push(Absorbed {
                    run: input.id().to_owned(),
                    by: by.id().to_owned(),
                });
                continue;
            }
            if kept.iter().any(|k| k.id() == input.id()) {
                continue;
            }
            let mut trial: Vec<CombineInput> = kept
                .iter()
                .filter(|k| !carries(&input, k.id()))
                .cloned()
                .collect();
            trial.push(input.clone());
            check_tasks(&trial)?;
            let refs: Vec<&CombineInput> = trial.iter().collect();
            match self.try_merge(&root, &refs).await? {
                Tried::Conflicts(conflicts) => {
                    let me = refs.len() - 1;
                    let (with, paths) = conflict_pairs(&conflicts)
                        .into_iter()
                        .find_map(|((a, b), paths)| {
                            (a == me)
                                .then_some((b, paths.clone()))
                                .or_else(|| (b == me).then_some((a, paths)))
                        })
                        .unwrap_or_else(|| (0, conflicts.iter().map(|c| c.path.clone()).collect()));
                    report.excluded.push(left(
                        &input,
                        LeftWhy::Conflict {
                            with: trial[with].run_ref(),
                            paths,
                        },
                    ));
                }
                Tried::Verified {
                    ws,
                    snapshot,
                    merged,
                    verdicts,
                } => {
                    let failing: Vec<&TaskVerdict> = verdicts
                        .iter()
                        .filter(|v| !self.passes(v.verdict, &trial))
                        .collect();
                    // Its own task's verdict when that fails; any failing
                    // test fails every task's verify of the merge.
                    let blame = failing
                        .iter()
                        .find(|v| input.contains.iter().any(|c| c.run == v.run))
                        .or(failing.first());
                    if let Some(v) = blame {
                        let why = not_pass(v);
                        let _ = self.api.discard(&self.admin.signing_key, &ws).await;
                        report.excluded.push(left(&input, why));
                        continue;
                    }
                    if let Some(Tried::Verified { ws, .. }) = best.take() {
                        let _ = self.api.discard(&self.admin.signing_key, &ws).await;
                    }
                    for k in kept.iter().filter(|k| carries(&input, k.id())) {
                        absorbed.push(Absorbed {
                            run: k.id().to_owned(),
                            by: input.id().to_owned(),
                        });
                    }
                    for a in &mut absorbed {
                        if carries(&input, &a.by) {
                            a.by = input.id().to_owned();
                        }
                    }
                    kept = trial;
                    best = Some(Tried::Verified {
                        ws,
                        snapshot,
                        merged,
                        verdicts,
                    });
                }
            }
        }
        report.absorbed = absorbed;
        let Some(Tried::Verified {
            ws,
            snapshot,
            merged,
            verdicts,
        }) = best
        else {
            report.status = if report
                .excluded
                .iter()
                .any(|l| matches!(l.why, LeftWhy::Conflict { .. }))
            {
                CombineStatus::Conflicts
            } else {
                CombineStatus::NotPass
            };
            return self.finish(report);
        };
        report.labels = kept.iter().map(CombineInput::label).collect();
        report.verdicts = verdicts;
        report.merged = merged;
        crate::fault::point("combine.verified")?;
        self.export(report, &kept, &ws, &snapshot).await
    }

    /// The tasks whose `REVIEW` only `--approve-review` approves: inputs
    /// not accepted yet that are `REVIEW`, or else, when no task the merge
    /// carries was accepted at `REVIEW`, those `REVIEW` on the merge.
    fn approved_by_flag(&self, inputs: &[CombineInput], verdicts: &[TaskVerdict]) -> Vec<RunRef> {
        if !self.approve_review {
            return Vec::new();
        }
        let pending: Vec<RunRef> = inputs
            .iter()
            .filter(|i| i.review.is_some())
            .map(CombineInput::run_ref)
            .collect();
        let prior = inputs
            .iter()
            .flat_map(|i| &i.contains)
            .any(|c| c.approved_review);
        if !pending.is_empty() || prior {
            return pending;
        }
        verdicts
            .iter()
            .filter(|v| v.verdict == Verdict::Review)
            .map(TaskVerdict::task)
            .collect()
    }

    /// Refuses `input` (to be accepted as attempt `index`) when its
    /// directory is no longer the head that was merged.
    async fn still_merged(
        &self,
        home: &Home,
        input: &CombineInput,
        run: &RunState,
        index: u32,
    ) -> Result<(), RunError> {
        let agent = run
            .agent(&index.to_string())
            .ok_or_else(|| RunError::Invalid(format!("run {} has no attempt {index}", run.id)))?;
        let key = compare::agent_key(home, agent).ok_or_else(|| {
            RunError::Invalid(format!(
                "the key of attempt {index} of run {} is gone",
                run.id
            ))
        })?;
        if self
            .api
            .snapshot(&key.signing_key, &agent.workspace)
            .await?
            != input.head
        {
            return Err(RunError::Invalid(format!(
                "{} changed after it was merged and verified, so the combine no longer holds it; nothing was accepted or exported. Run the combine again",
                input.label()
            )));
        }
        Ok(())
    }

    /// Accepts the inputs not accepted yet, commits the merged workspace
    /// and exports it.
    async fn export(
        &self,
        mut report: Report,
        inputs: &[CombineInput],
        ws: &aver_core::WorkspaceId,
        merged: &SnapshotId,
    ) -> Result<Report, RunError> {
        let first = &inputs[0];
        let repo = first.repo();
        let refs: Vec<&CombineInput> = inputs.iter().collect();
        let contained = union(&refs);
        report.approved_review = self.approved_by_flag(inputs, &report.verdicts);
        let titles: Vec<&str> = contained.iter().map(|c| c.title.as_str()).collect();
        let runs: Vec<String> = contained.iter().map(|c| c.run.clone()).collect();
        let earlier = self.home.and_then(|h| Pending::load(h, &runs));
        let branch = self
            .branch
            .clone()
            .or_else(|| earlier.as_ref().map(|p| p.branch.clone()))
            .unwrap_or_else(|| {
                default_branch(&format!("combine {}", titles.join(" and ")), first.id())
            });
        let discard = || async {
            let _ = self.api.discard(&self.admin.signing_key, ws).await;
        };
        // Every branch is checked before anything is accepted, so an accept
        // cannot fail halfway for a name. A branch an interrupted combine
        // (or accept) of the same runs wrote at the same snapshot is
        // adopted.
        let mut names = vec![(branch.clone(), *merged)];
        let pending: Vec<(&CombineInput, &RunState, u32)> = inputs
            .iter()
            .filter_map(|i| i.to_accept().map(|(run, n)| (i, run, n)))
            .collect();
        for (i, run, _) in &pending {
            let name = self
                .home
                .and_then(|h| super::state::Accepting::load(h, &run.id))
                .map_or_else(|| branch_for(run), |j| j.branch);
            names.push((name, i.head));
        }
        for (name, at) in &names {
            if let Err(e) = check_branch(repo, name, Some(at)) {
                discard().await;
                return Err(e);
            }
        }
        let identity = match crate::git::user_identity(repo) {
            Ok(id) => id,
            Err(e) => {
                discard().await;
                return Err(RunError::Invalid(e));
            }
        };
        // Every input still to accept must still be the head that was
        // merged and verified, checked for all before the first accept, and
        // each accept takes only that head: no task is accepted at a
        // snapshot other than the one the branch holds.
        let home = match (self.home, pending.first()) {
            (Some(h), _) => Some(h),
            (None, None) => None,
            (None, Some((_, run, _))) => {
                discard().await;
                return Err(RunError::Invalid(format!(
                    "run {} is not accepted, and accepting it needs the home",
                    run.id
                )));
            }
        };
        for (input, run, index) in &pending {
            let home = home.expect("checked above");
            if let Some(hook) = self.before_accept {
                if let Some(a) = run.agent(&index.to_string()) {
                    hook(&a.dir);
                }
            }
            if let Err(e) = self.still_merged(home, input, run, *index).await {
                discard().await;
                return Err(e);
            }
        }
        let mut plan = Pending {
            runs: runs.clone(),
            repo: repo.to_owned(),
            branch: branch.clone(),
            snapshot: *merged,
            planned: names[1..]
                .iter()
                .zip(pending.iter())
                .map(|((b, _), (_, run, _))| (run.id.clone(), b.clone()))
                .collect(),
            step: "planned".into(),
        };
        if let Some(home) = self.home {
            plan.save(home, None)?;
        }
        for (input, run, index) in pending {
            let home = home.expect("checked above");
            let mut run = run.clone();
            let a = accept::accept(
                self.api,
                home,
                self.admin,
                &mut run,
                &index.to_string(),
                &AcceptOptions {
                    expect_head: Some(input.head),
                    approve_review: self.approve_review,
                    ..Default::default()
                },
            )
            .await;
            match a {
                Ok(a) => report.accepted.push((RunRef::of(&run), a.branch)),
                Err(e) => {
                    discard().await;
                    return Err(e);
                }
            }
            if let Some(home) = self.home {
                plan.save(home, Some("accepted"))?;
            }
            crate::fault::point("combine.accepted")?;
        }
        // Every receipt of the merge verified `merged`; the export refuses
        // any other snapshot under it.
        let receipt = report
            .verdicts
            .first()
            .map(|v| v.receipt)
            .ok_or_else(|| RunError::Invalid("the merge has no receipt".into()))?;
        if let [only] = inputs {
            // One head, stacked on the tasks it carries: the merge is that
            // head, already a lineage of one commit per task on the base,
            // which is exported as it is.
            discard().await;
            // Accepting committed the head: its committed snapshot is the
            // one with the task's commit.
            let head = match only.to_accept() {
                Some((run, _)) => self
                    .home
                    .and_then(|h| RunState::load(h, &run.id).ok())
                    .and_then(|r| r.decided.and_then(|d| d.head))
                    .unwrap_or(only.head),
                None => only.head,
            };
            let commit = self
                .api
                .export_reviewed(&head, &receipt, repo, &branch, Some(&identity))
                .await?;
            crate::fault::point("combine.exported")?;
            report.parent = git_parent(repo, &commit);
            return self.exported(report, repo, branch, commit, head, contained);
        }
        let mut message = format!("Combine: {}\n\n", titles.join("; "));
        let _ = writeln!(
            message,
            "Merged by aver combine from the work of {} tasks, and verified against each task:",
            contained.len()
        );
        for v in &report.verdicts {
            let _ = writeln!(message, "- {}: {}", v.title, v.verdict.as_str());
        }
        if !report.approved_review.is_empty() {
            let names: Vec<String> = report.approved_review.iter().map(RunRef::label).collect();
            let _ = writeln!(
                message,
                "REVIEW approved with --approve-review: {}",
                names.join(", ")
            );
        }
        if !report.excluded.is_empty() {
            message.push_str("\nLeft out:\n");
            for l in &report.excluded {
                let _ = writeln!(message, "- {}: {}", l.task.title, why_text(&l.why));
            }
        }
        message.push('\n');
        for (c, v) in contained.iter().zip(&report.verdicts) {
            let _ = write!(
                message,
                "Aver-Task: {}\nAver-Attempt: {}\nAver-Receipt: {}\n",
                c.task, c.attempt, v.receipt
            );
        }
        let committed = self
            .api
            .commit(&self.admin.signing_key, ws, &message)
            .await?;
        if let Some(home) = self.home {
            plan.save(home, Some("committed"))?;
        }
        crate::fault::point("combine.committed")?;
        let commit = self
            .api
            .export_reviewed(
                &committed.snapshot,
                &receipt,
                repo,
                &branch,
                Some(&identity),
            )
            .await?;
        crate::fault::point("combine.exported")?;
        report.parent = git_parent(repo, &commit);
        self.exported(report, repo, branch, commit, committed.snapshot, contained)
    }

    /// Records the export, by commit, so `aver rerun --onto <branch>`
    /// builds on this snapshot and never on whatever the branch holds later.
    fn exported(
        &self,
        mut report: Report,
        repo: &std::path::Path,
        branch: String,
        commit: String,
        snapshot: SnapshotId,
        contained: Vec<&Contained>,
    ) -> Result<Report, RunError> {
        if let Some(home) = self.home {
            record::save_exported(
                home,
                &record::ExportedCombine {
                    commit: commit.clone(),
                    snapshot,
                    branch: branch.clone(),
                    repo: repo.to_owned(),
                    runs: contained.iter().map(|c| c.run.clone()).collect(),
                    at: aver_core::Timestamp::now(),
                },
            )?;
            Pending::remove(
                home,
                &contained.iter().map(|c| c.run.clone()).collect::<Vec<_>>(),
            );
        }
        report.status = if report.excluded.is_empty() {
            CombineStatus::Exported
        } else {
            CombineStatus::Partial
        };
        report.branch = Some(branch);
        report.commit = Some(commit);
        report.snapshot = Some(snapshot);
        report.included = contained.into_iter().cloned().collect();
        self.finish(report)
    }

    /// Writes the batch's combine record.
    fn finish(&self, report: Report) -> Result<Report, RunError> {
        if let (Some(home), Some(batch)) = (self.home, &self.batch) {
            record::save(home, batch, &record_of(&report))?;
        }
        Ok(report)
    }
}

/// `combines/pending-<runs>.json`: a combine past its verify, written
/// before it accepts anything and removed once its export is recorded. A
/// combine of the same runs run again after a crash takes its names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    /// Every run whose task the result holds, in order.
    pub runs: Vec<String>,
    pub repo: std::path::PathBuf,
    pub branch: String,
    /// The merged snapshot every task was verified on.
    pub snapshot: SnapshotId,
    /// The inputs to accept and the branch each is accepted as.
    pub planned: Vec<(String, String)>,
    /// The last step done: `planned`, `accepted` (after each accept),
    /// `committed`.
    pub step: String,
}

impl Pending {
    fn path(home: &Home, runs: &[String]) -> std::path::PathBuf {
        home.root()
            .join("combines")
            .join(format!("pending-{}.json", runs.join("-")))
    }

    pub fn load(home: &Home, runs: &[String]) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(Self::path(home, runs)).ok()?).ok()
    }

    fn save(&mut self, home: &Home, step: Option<&str>) -> Result<(), RunError> {
        if let Some(step) = step {
            self.step = step.to_owned();
        }
        let path = Self::path(home, &self.runs);
        std::fs::create_dir_all(path.parent().expect("combines/ is a directory"))?;
        let bytes = serde_json::to_vec_pretty(self).expect("plans serialize");
        crate::atomic::write(&path, &bytes, crate::atomic::Mode::Keep)?;
        Ok(())
    }

    fn remove(home: &Home, runs: &[String]) {
        let _ = std::fs::remove_file(Self::path(home, runs));
    }
}

fn git_parent(repo: &std::path::Path, commit: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("{commit}^")])
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn base_commit_of(input: &CombineInput) -> String {
    input.base_commit.clone()
}

/// The commit the first run of `run`'s chain started from.
fn root_commit(load: &dyn Fn(&str) -> Option<RunState>, run: &RunState) -> String {
    let mut at = run.clone();
    for _ in 0..64 {
        match prerequisites(&at).first().and_then(|id| load(id)) {
            Some(pre) => at = pre,
            None => break,
        }
    }
    at.base_commit
}

/// Every task `inputs` carry, each once (two runs stacked on one
/// prerequisite both carry it), in order.
fn union<'a>(inputs: &[&'a CombineInput]) -> Vec<&'a Contained> {
    let mut seen = BTreeSet::new();
    inputs
        .iter()
        .flat_map(|i| &i.contains)
        .filter(|c| seen.insert(c.run.as_str()))
        .collect()
}

/// Refuses two inputs on one task.
fn check_tasks(inputs: &[CombineInput]) -> Result<(), RunError> {
    let mut seen: BTreeMap<TaskId, &str> = BTreeMap::new();
    for c in inputs.iter().flat_map(|i| &i.contains) {
        if let Some(other) = seen.insert(c.task, &c.run) {
            if other != c.run {
                return Err(RunError::Invalid(format!(
                    "runs {other} and {} share task {}; combine merges different tasks",
                    c.run, c.task
                )));
            }
        }
    }
    Ok(())
}

/// For each conflict, every head involved but the first, paired with the
/// first: `(head, first) -> paths`. In a line conflict, the heads involved
/// are those with a side in a conflicting hunk, not every head that changed
/// the file.
fn conflict_pairs(conflicts: &[Conflict]) -> BTreeMap<(usize, usize), Vec<String>> {
    let mut out: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
    for c in conflicts {
        let groups: Vec<Vec<usize>> = if c.hunks.is_empty() {
            vec![c.heads.clone()]
        } else {
            c.hunks
                .iter()
                .map(|h| h.sides.iter().map(|s| s.head).collect())
                .collect()
        };
        for mut heads in groups {
            heads.sort_unstable();
            heads.dedup();
            let Some((&first, rest)) = heads.split_first() else {
                continue;
            };
            for &h in rest {
                let paths = out.entry((h, first)).or_default();
                if !paths.contains(&c.path) {
                    paths.push(c.path.clone());
                }
            }
        }
    }
    out
}

fn left(input: &CombineInput, why: LeftWhy) -> Left {
    Left {
        task: input.run_ref(),
        why,
        carries: input
            .contains
            .iter()
            .filter(|c| c.via.is_some())
            .map(RunRef::of_task)
            .collect(),
    }
}

fn not_pass(v: &TaskVerdict) -> LeftWhy {
    LeftWhy::NotPass {
        task: v.task(),
        verdict: v.verdict,
        receipt: v.receipt,
    }
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    let canonical = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    a == b || canonical(a) == canonical(b)
}

/// `conflicts with #1 at README.md`, `FAIL_OWN on the merge`.
pub fn why_text(why: &LeftWhy) -> String {
    match why {
        LeftWhy::Conflict { with, paths } => {
            format!("conflicts with {} at {}", with.label(), paths.join(", "))
        }
        LeftWhy::NotPass {
            task,
            verdict: Verdict::Review,
            receipt,
        } => format!(
            "{} is REVIEW (receipt {receipt}): it changes what an existing test expects, which only a person approves (aver diff, then aver accept it, or combine again with --approve-review)",
            task.label(),
        ),
        LeftWhy::NotPass {
            task,
            verdict,
            receipt,
        } => format!(
            "{} is {} (receipt {receipt})",
            task.label(),
            verdict.as_str()
        ),
    }
}

/// The batch's record of a report: task numbers only.
pub fn record_of(report: &Report) -> CombineRecord {
    let mut included: Vec<u32> = report.included.iter().filter_map(|c| c.number).collect();
    included.sort_unstable();
    included.dedup();
    let mut excluded = Vec::new();
    for l in &report.excluded {
        let why = match &l.why {
            LeftWhy::Conflict { with, paths } => ExcludedWhy::Conflict {
                with: with.number.unwrap_or(0),
                paths: paths.clone(),
            },
            LeftWhy::NotPass {
                verdict, receipt, ..
            } => ExcludedWhy::NotPass {
                verdict: *verdict,
                receipt: *receipt,
            },
        };
        for t in std::iter::once(&l.task).chain(&l.carries) {
            if let Some(number) = t.number {
                if !included.contains(&number) {
                    excluded.push(Excluded {
                        number,
                        run: t.run.clone(),
                        why: why.clone(),
                    });
                }
            }
        }
    }
    CombineRecord {
        at: aver_core::Timestamp::now(),
        status: report.status,
        included,
        excluded,
        branch: report.branch.clone(),
    }
}

/// Merges the accepted winners of `runs` and, when every task passes on
/// the result, exports it as one branch. A stacked run's prerequisites
/// must be among `runs`; [`Combine::run`] finds them in the home.
pub async fn combine(
    api: &Api,
    admin: &AgentKey,
    runs: &[RunState],
    opts: &CombineOptions,
) -> Result<Combined, RunError> {
    if runs.len() < 2 && runs.iter().all(|r| r.stacked_on.is_none()) {
        return Err(RunError::Invalid("combine takes two or more runs".into()));
    }
    let load = |id: &str| runs.iter().find(|r| r.id == id).cloned();
    let resolved = runs
        .iter()
        .map(|r| accepted_input(&load, r.clone()).map(|i| Resolved::Ready(Box::new(i))))
        .collect::<Result<Vec<_>, _>>()?;
    let report = Combine {
        api,
        home: None,
        admin,
        branch: opts.branch.clone(),
        partial: false,
        batch: None,
        approve_review: false,
        before_accept: None,
    }
    .run(resolved)
    .await?;
    Ok(report.into())
}

impl From<Report> for Combined {
    /// A report of a combine that is not `--partial`.
    fn from(report: Report) -> Self {
        match report.status {
            CombineStatus::Conflicts => Combined::Conflicts {
                conflicts: report.conflicts,
                labels: report.labels,
            },
            CombineStatus::NotPass => Combined::NotPass {
                snapshot: report
                    .snapshot
                    .expect("a merge that does not pass has a snapshot"),
                verdicts: report.verdicts,
            },
            CombineStatus::Exported | CombineStatus::Partial => Combined::Exported {
                branch: report.branch.expect("exported"),
                commit: report.commit.expect("exported"),
                snapshot: report.snapshot.expect("exported"),
                merged: report.merged,
                verdicts: report.verdicts,
            },
        }
    }
}

/// Conflicts for a human: each file, why, and for line conflicts the base
/// lines and what each run did there, then each run's lines, fenced as
/// repository data under [`FENCE_NOTE`].
pub fn conflicts_text(conflicts: &[Conflict], labels: &[String]) -> String {
    let mut out = String::new();
    if conflicts.iter().any(|c| {
        c.hunks
            .iter()
            .any(|h| h.sides.iter().any(|s| !s.text.is_empty()))
    }) {
        let _ = writeln!(out, "{FENCE_NOTE}");
    }
    let label = |i: &usize| {
        labels
            .get(*i)
            .cloned()
            .unwrap_or_else(|| format!("head {i}"))
    };
    for c in conflicts {
        let who: Vec<String> = c.heads.iter().map(label).collect();
        let why = match c.kind {
            ConflictKind::Content => "both changed the same or adjacent lines",
            ConflictKind::DeleteModify => "one deleted it, another changed it",
            ConflictKind::AddAdd => "each added it, with different contents",
            ConflictKind::Kind => "its kind changed (file, executable or symlink)",
            ConflictKind::Binary => "not text, so it cannot be merged by line",
        };
        let _ = writeln!(out, "  {}: {why} ({})", one_line(&c.path), who.join(", "));
        for h in &c.hunks {
            let sides: Vec<String> = h
                .sides
                .iter()
                .map(|s| {
                    let who = label(&s.head);
                    if s.base.start == s.base.end {
                        format!("{who} inserted its {}", s.lines.human())
                    } else if s.lines.start == s.lines.end {
                        format!("{who} deleted {}", s.base.human())
                    } else {
                        format!(
                            "{who} replaced {} with its {}",
                            s.base.human(),
                            s.lines.human()
                        )
                    }
                })
                .collect();
            let _ = writeln!(out, "    at base {}: {}", h.base.human(), sides.join("; "));
            for s in h.sides.iter().filter(|s| s.lines.start < s.lines.end) {
                let total = (s.lines.end - s.lines.start) as usize;
                let mut body: String = s
                    .text
                    .iter()
                    .map(|l| format!("{}\n", one_line(l)))
                    .collect();
                if total > s.text.len() {
                    let _ = writeln!(body, "… {} more lines", total - s.text.len());
                }
                let _ = write!(out, "    {}'s lines:\n{}", label(&s.head), fence(&body));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(c: char) -> SnapshotId {
        // The last character holds only the hash's last bit: `a`.
        format!("snap_{}a", c.to_string().repeat(51))
            .parse()
            .unwrap()
    }

    fn input(id: &str, number: u32, carries: &[&str]) -> CombineInput {
        let task: TaskId = format!("task_{}", "a".repeat(52)).parse().unwrap();
        let run: RunState = serde_json::from_value(serde_json::json!({
            "schema": "aver.runstate/v0",
            "id": id,
            "repo": "/r",
            "base_commit": "0000",
            "base": snap('a'),
            "task": task,
            "title": format!("task {number}"),
            "supervisor": { "pid": 1, "started_at": aver_core::Timestamp::now() },
            "budget": { "timeout_ms": 1, "idle_ms": 1, "max_verifies": 1 },
            "agents": [],
            "batch": { "id": "B", "number": number },
        }))
        .unwrap();
        let contained = |run: &str, via: Option<&str>| Contained {
            run: run.into(),
            task,
            title: run.into(),
            number: None,
            base: snap('a'),
            attempt: AttemptId::generate(),
            via: via.map(Into::into),
            approved_review: false,
            workspace: false,
        };
        let mut contains: Vec<Contained> = carries.iter().map(|c| contained(c, Some(id))).collect();
        contains.push(contained(id, None));
        CombineInput {
            source: Source::Run {
                run: Box::new(run),
                accept: None,
            },
            head: snap('b'),
            root: snap('a'),
            base_commit: "0000".into(),
            contains,
            review: None,
        }
    }

    fn ids(inputs: &[CombineInput]) -> Vec<&str> {
        inputs.iter().map(CombineInput::id).collect()
    }

    #[test]
    fn a_prerequisite_named_beside_its_dependent_is_merged_once_through_it() {
        // #2 is stacked on #1; #3 is not stacked.
        let mut inputs = vec![
            input("R1", 1, &[]),
            input("R2", 2, &["R1"]),
            input("R3", 3, &[]),
        ];
        let absorbed = dedupe_chains(&mut inputs);
        assert_eq!(ids(&inputs), ["R2", "R3"]);
        assert_eq!(
            absorbed,
            [Absorbed {
                run: "R1".into(),
                by: "R2".into()
            }]
        );
        // #1 stays in the result: the head that carries it lists it.
        assert!(inputs[0].contains.iter().any(|c| c.run == "R1"));

        // A chain #1 <- #2 <- #3 named whole is one head; a run named
        // twice is merged once.
        let mut chain = vec![
            input("R3", 3, &["R1", "R2"]),
            input("R1", 1, &[]),
            input("R2", 2, &["R1"]),
            input("R3", 3, &["R1", "R2"]),
        ];
        dedupe_chains(&mut chain);
        assert_eq!(ids(&chain), ["R3"]);

        // Two tasks stacked on the same prerequisite both stay.
        let mut diamond = vec![input("R2", 2, &["R1"]), input("R3", 3, &["R1"])];
        assert!(dedupe_chains(&mut diamond).is_empty());
        assert_eq!(ids(&diamond), ["R2", "R3"]);
    }

    #[test]
    fn conflicts_pair_each_later_head_with_the_first() {
        let c = |path: &str, heads: Vec<usize>| Conflict {
            path: path.into(),
            kind: ConflictKind::Content,
            heads,
            hunks: Vec::new(),
        };
        let mut d = c("d", vec![0, 1, 2]);
        d.hunks = vec![crate::workspace::merge::ConflictHunk {
            base: crate::workspace::merge::Lines { start: 2, end: 3 },
            sides: [0, 1]
                .map(|head| crate::workspace::merge::Side {
                    head,
                    base: crate::workspace::merge::Lines { start: 2, end: 3 },
                    lines: crate::workspace::merge::Lines { start: 2, end: 3 },
                    text: Vec::new(),
                })
                .to_vec(),
        }];
        let pairs = conflict_pairs(&[
            c("a", vec![0, 2]),
            c("b", vec![2, 0]),
            c("c", vec![1, 2, 3]),
            d,
        ]);
        assert_eq!(pairs[&(2, 0)], ["a", "b"]);
        assert_eq!(pairs[&(2, 1)], ["c"]);
        assert_eq!(pairs[&(3, 1)], ["c"]);
        // Head 2 changed d elsewhere: only the heads in the hunk conflict.
        assert_eq!(pairs[&(1, 0)], ["d"]);
        assert_eq!(pairs.len(), 4);
    }
}
