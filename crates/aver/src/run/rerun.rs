//! `aver rerun <n> [--onto <m>]` (§2.11): a task again, on a fresh run with
//! the prompt, criteria and protected paths of its Task object, either on
//! the base its last run had or on top of other work: another task's
//! accepted winner (accepted first when it passed and nobody accepted it
//! yet), a run, or a branch Aver made (an accepted task's, or a combine's).
//! A stacked run records what it builds on (`stacked_on`), so `aver
//! combine` verifies the tasks under it too. In a batch, the new run
//! replaces the task's run and the old one moves to its history.

use std::io;
use std::path::PathBuf;
use std::process::Command;

use super::accept::{self, AcceptOptions};
use super::agent::Template;
use super::api::Api;
use super::batch::{self, record, BatchState};
use super::combine::{self, root_of, LeftWhy};
use super::state::{self, BatchRef, Budget, RunState, Stack, StackOnto};
use super::supervisor::{RunError, RunSpec};
use crate::keys::{AgentKey, Home};

/// What `--onto` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Onto {
    /// A task of the batch, by number.
    Task(u32),
    /// A run id, or the attempt id of its winner.
    Run(String),
    /// A branch Aver made: an accepted task's, or a combine's.
    Branch(String),
}

/// A task number: `2` or `#2`.
pub fn task_number(s: &str) -> Option<u32> {
    s.strip_prefix('#').unwrap_or(s).parse().ok()
}

impl Onto {
    /// A task number, a run (an id whose run file exists, or `att_…`),
    /// else a branch.
    pub fn parse(home: &Home, s: &str) -> Self {
        if let Some(n) = task_number(s) {
            return Self::Task(n);
        }
        if s.starts_with("att_") || RunState::load(home, &s.to_uppercase()).is_ok() {
            return Self::Run(s.to_owned());
        }
        Self::Branch(s.to_owned())
    }
}

/// How to run the agents of the new run.
#[derive(Debug, Clone)]
pub struct RerunOptions {
    /// One template per agent.
    pub agents: Vec<Template>,
    /// Default: the old run's.
    pub budget: Option<Budget>,
    pub deps: bool,
    pub git_view: bool,
    pub aver_bin: PathBuf,
    pub ports: u16,
    /// The new run's id (default: a fresh one).
    pub id: Option<String>,
    /// Accept a `REVIEW` task `--onto` names (`--approve-review`); without
    /// it only `PASS` work is accepted to build on.
    pub approve_review: bool,
}

/// What the new run starts on, for the human.
#[derive(Debug, Clone)]
pub struct Base {
    /// `#1 ("Add negate")`, `aver/combine-…`.
    pub label: String,
    /// The branch holding it.
    pub branch: Option<String>,
    pub commit: String,
    /// The task `--onto` named was accepted by this rerun, as `branch`.
    pub accepted: bool,
    /// What it accepted was `REVIEW`, approved by `--approve-review`.
    pub approved_review: bool,
}

/// A rerun ready to start.
#[derive(Debug, Clone)]
pub struct Rerun {
    pub spec: RunSpec,
    /// The task's last run; `None` for a task of a batch that never started.
    pub old: Option<RunState>,
    /// The batch and number the task has there.
    pub batch: Option<BatchRef>,
    pub onto: Option<Base>,
}

/// What a rerun repeats: the task's last run, or, for a task of a batch
/// that never started (waiting, blocked), what the batch saved of it
/// ([`batch::TaskFile`]) and the batch's base.
#[derive(Debug, Clone)]
pub struct Prior {
    pub run: Option<RunState>,
    /// The last run's id, or the one the batch chose for the task.
    pub id: String,
    /// The last run's task; a task that never started has none yet.
    pub task: Option<aver_core::TaskId>,
    pub repo: PathBuf,
    pub base_commit: String,
    /// The base snapshot of the chain the task started on.
    pub root: aver_core::SnapshotId,
    pub stacked_on: Option<Stack>,
    pub budget: Budget,
    pub batch: Option<BatchRef>,
    /// The tasks it is after in its batch.
    pub after: Vec<u32>,
    /// The agents' template names.
    pub agents: Vec<String>,
    /// The saved task, when it never started.
    pub saved: Option<batch::SavedTask>,
}

impl Prior {
    fn of_run(run: RunState, after: Vec<u32>) -> Self {
        Self {
            id: run.id.clone(),
            task: Some(run.task),
            repo: run.repo.clone(),
            base_commit: run.base_commit.clone(),
            root: root_of(&run),
            stacked_on: run.stacked_on.clone(),
            budget: run.budget.clone(),
            batch: run.batch.clone(),
            after,
            agents: run.agents.iter().map(|a| a.template.clone()).collect(),
            saved: None,
            run: Some(run),
        }
    }
}

/// What task `target` (its number in `batch`, or a run id) last was.
pub fn prior(home: &Home, batch: Option<&BatchState>, target: &str) -> Result<Prior, RunError> {
    match (batch, task_number(target)) {
        (Some(b), Some(n)) => {
            let entry = b
                .entry(n)
                .ok_or_else(|| RunError::Invalid(format!("batch {} has no task #{n}", b.id)))?;
            if let Ok(run) = RunState::load(home, &entry.run) {
                return Ok(Prior::of_run(run, entry.after.clone()));
            }
            let saved = batch::TaskFile::load(home, &b.id).ok();
            let Some((file, task)) = saved
                .as_ref()
                .and_then(|f| f.task(n).map(|t| (f, t.clone())))
            else {
                return Err(RunError::Invalid(format!(
                    "task #{n} of batch {} never started, and an older aver ran the batch without saving its tasks; aver run it as a task of its own",
                    b.id
                )));
            };
            Ok(Prior {
                run: None,
                id: entry.run.clone(),
                task: None,
                repo: b.repo.clone(),
                base_commit: b.base_commit.clone(),
                root: b.base,
                stacked_on: None,
                budget: file.budget.clone(),
                batch: Some(BatchRef {
                    id: b.id.clone(),
                    number: n,
                    issue: task.issue,
                }),
                after: entry.after.clone(),
                agents: file.agents.clone(),
                saved: Some(task),
            })
        }
        (None, Some(n)) => Err(RunError::Invalid(format!(
            "no batch to find task #{n} in; name a run id, or the batch with --batch"
        ))),
        (_, None) => combine::find_run(home, target).map(|r| Prior::of_run(r, Vec::new())),
    }
}

/// An accepted run as a base: its winner's commit and snapshot. A run
/// nobody accepted is accepted first when its best attempt is `PASS`, or
/// `REVIEW` with `approve_review`.
async fn onto_run(
    api: &Api,
    home: &Home,
    admin: &AgentKey,
    mut run: RunState,
    approve_review: bool,
) -> Result<(Stack, Base), RunError> {
    let label = match run.batch.as_ref() {
        Some(b) => format!("#{} ({:?})", b.number, run.title),
        None => format!("run {} ({:?})", run.id, run.title),
    };
    let mut accepted = false;
    let mut approved_review = false;
    if run.decided.is_none() {
        if run.supervisor_alive() {
            return Err(RunError::Invalid(format!(
                "{label} is still running; rerun onto it once it ends"
            )));
        }
        let best = batch::best_attempt(api, home, &admin.signing_key, &run)
            .await
            .ok_or_else(|| RunError::Invalid(format!("{label} has no attempts")))?;
        // Accepting refuses anything but PASS, and REVIEW unless
        // --approve-review says a person approved it: only verified work
        // becomes a base.
        let done = accept::accept(
            api,
            home,
            admin,
            &mut run,
            &best.to_string(),
            &AcceptOptions {
                approve_review,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| match run.batch.as_ref() {
            _ if !e.to_string().contains(" is REVIEW: ") => {
                RunError::Invalid(format!("{label} cannot be built on: {e}"))
            }
            Some(b) => RunError::Invalid(format!(
                "{label} needs review before others build on it: {e}. Review it (aver diff {n}, then aver accept {n}), or pass --approve-review to accept it here",
                n = b.number
            )),
            None => RunError::Invalid(format!(
                "{label} needs review before others build on it: {e}. Review it (aver diff {best} --run {r}, then aver accept {best} --run {r}), or pass --approve-review to accept it here",
                r = run.id
            )),
        })?;
        accepted = true;
        approved_review = done.verdict == aver_core::Verdict::Review;
        crate::fault::point("rerun.onto_accepted")?;
    }
    let decided = run.decided.as_ref().expect("accepted above");
    let (Some(commit), Some(head)) = (decided.commit.clone(), decided.head) else {
        return Err(RunError::Invalid(format!(
            "{label} was accepted before aver recorded its commit and snapshot; accept it again with a newer aver"
        )));
    };
    let stack = Stack {
        onto: StackOnto::Run {
            run: run.id.clone(),
            number: run.batch.as_ref().map(|b| b.number),
        },
        commit: commit.clone(),
        head,
        root: root_of(&run),
    };
    Ok((
        stack,
        Base {
            label,
            branch: decided.branch.clone(),
            commit,
            accepted,
            approved_review,
        },
    ))
}

fn git(repo: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A branch Aver made as a base: an accepted task's (its run, at the
/// commit it was accepted as), or a combine's, at the commit and snapshot
/// the combine verified and exported (`record::exported`). A branch whose
/// tip is neither, a combine branch amended or moved since, is refused:
/// its tip is not verified work.
async fn onto_branch(
    api: &Api,
    home: &Home,
    admin: &AgentKey,
    repo: &std::path::Path,
    branch: &str,
    approve_review: bool,
) -> Result<(Stack, Base), RunError> {
    let runs = RunState::all(home);
    if let Some(run) = runs.iter().find(|r| {
        r.repo == repo && r.decided.as_ref().and_then(|d| d.branch.as_deref()) == Some(branch)
    }) {
        return onto_run(api, home, admin, run.clone(), approve_review).await;
    }
    let commit = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )
    .ok_or_else(|| RunError::Invalid(format!("no branch {branch} in {}", repo.display())))?;
    let short = |c: &str| c[..12.min(c.len())].to_owned();
    let Some(combined) = record::exported(home, &commit).filter(|c| c.repo == repo) else {
        if let Some(c) = record::exported_to(home, repo, branch) {
            return Err(RunError::Invalid(format!(
                "{branch} is at {}, not at {}, the commit aver combine verified and exported there: it was amended or moved since, so its tip is not verified work. Build on the combine as it was (git branch -f {branch} {}), or combine again",
                short(&commit),
                short(&c.commit),
                c.commit
            )));
        }
        return Err(RunError::Invalid(format!(
            "{branch} is not a branch aver made from runs in this home (its tip {} is no commit aver accept or aver combine exported); --onto takes a task number, a run, or a branch aver accept or aver combine wrote",
            short(&commit)
        )));
    };
    let mut on: Vec<&RunState> = Vec::new();
    for id in &combined.runs {
        let Some(r) = runs.iter().find(|r| &r.id == id && r.decided.is_some()) else {
            return Err(RunError::Invalid(format!(
                "{branch} holds run {id}, which is no longer an accepted run in this home"
            )));
        };
        on.push(r);
    }
    let Some(first) = on.first() else {
        return Err(RunError::Invalid(format!("{branch} records no runs")));
    };
    let root = root_of(first);
    if on.iter().any(|r| root_of(r) != root) {
        return Err(RunError::Invalid(format!(
            "{branch} holds runs that started from different commits"
        )));
    }
    let stack = Stack {
        onto: StackOnto::Combine {
            branch: branch.to_owned(),
            runs: on.iter().map(|r| r.id.clone()).collect(),
        },
        commit: commit.clone(),
        head: combined.snapshot,
        root,
    };
    Ok((
        stack,
        Base {
            label: branch.to_owned(),
            branch: Some(branch.to_owned()),
            commit,
            accepted: false,
            approved_review: false,
        },
    ))
}

/// The runs a stack builds on, through every level.
fn under(home: &Home, stack: &Stack) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = match &stack.onto {
        StackOnto::Run { run, .. } => vec![run.clone()],
        StackOnto::Combine { runs, .. } => runs.clone(),
    };
    while let Some(id) = todo.pop() {
        if out.contains(&id) {
            continue;
        }
        if let Some(s) = RunState::load(home, &id).ok().and_then(|r| r.stacked_on) {
            match s.onto {
                StackOnto::Run { run, .. } => todo.push(run),
                StackOnto::Combine { runs, .. } => todo.extend(runs),
            }
        }
        out.push(id);
    }
    out
}

/// Refuses, before anything is accepted, to build `old`'s task on its own
/// run, on work that builds on it, or on work from another base.
fn refuse_onto(home: &Home, old: &Prior, run: &RunState, name: &str) -> Result<(), RunError> {
    let mut chain = vec![run.id.clone()];
    if let Some(s) = &run.stacked_on {
        chain.extend(under(home, s));
    }
    let same_batch_task = matches!(
        (&run.batch, &old.batch),
        (Some(a), Some(b)) if a.id == b.id && a.number == b.number
    );
    if chain.contains(&old.id) || Some(run.task) == old.task || same_batch_task {
        return Err(RunError::Invalid(format!(
            "run {} already holds {name}'s work; --onto names another task's",
            run.id
        )));
    }
    if root_of(run) != old.root {
        return Err(RunError::Invalid(format!(
            "run {} and {name} started from different commits; --onto needs work on the same base",
            run.id
        )));
    }
    Ok(())
}

/// The new run of task `target` (its number in `batch`, or a run id),
/// on `onto` or on its last run's base.
pub async fn spec_for(
    api: &Api,
    home: &Home,
    admin: &AgentKey,
    batch: Option<&BatchState>,
    target: &str,
    onto: Option<&Onto>,
    opts: RerunOptions,
) -> Result<Rerun, RunError> {
    if let Some(b) = batch.filter(|b| b.alive()) {
        return Err(RunError::Invalid(format!(
            "batch {} is still running (pid {}); rerun its tasks once it ends, or stop it with ctrl-c",
            b.id, b.supervisor.pid
        )));
    }
    let old = prior(home, batch, target)?;
    if let Some(run) = old.run.as_ref().filter(|r| r.supervisor_alive()) {
        return Err(RunError::Invalid(format!(
            "run {} is still running (pid {}); rerun it once it ends",
            run.id, run.supervisor.pid
        )));
    }
    let name = match old.batch.as_ref() {
        Some(b) => format!("#{}", b.number),
        None => format!("run {}", old.id),
    };
    let (title, prompt, criteria, protected) = match (&old.saved, old.task) {
        (Some(t), _) => (
            t.title.clone(),
            t.prompt.clone(),
            t.criteria.clone(),
            t.protected.clone(),
        ),
        (None, Some(id)) => {
            let task = api.task(&id).await?;
            let body = task.body;
            (
                body.title.as_str().to_owned(),
                body.intent.as_str().to_owned(),
                body.criteria
                    .iter()
                    .map(|c| (c.id.to_string(), c.text.clone()))
                    .collect(),
                body.protected_paths.iter().cloned().collect(),
            )
        }
        (None, None) => unreachable!("a prior is a run or a saved task"),
    };
    if onto.is_none() && old.run.is_none() && !old.after.is_empty() {
        let after: Vec<String> = old.after.iter().map(|n| format!("#{n}")).collect();
        let how = match old.after.as_slice() {
            [m] => format!("aver rerun {} --onto {m}", target.trim_start_matches('#')),
            _ => format!(
                "aver combine {}, then aver rerun {} --onto <the combine's branch>",
                after.join(" ").replace('#', ""),
                target.trim_start_matches('#')
            ),
        };
        return Err(RunError::Invalid(format!(
            "{name} never started, and it builds on {}; say what it starts on: {how}",
            after.join(" and ")
        )));
    }
    let (stack, base) = match onto {
        None => (old.stacked_on.clone(), None),
        Some(o) => {
            let (stack, base) = match o {
                Onto::Task(m) => {
                    let b = batch.ok_or_else(|| {
                        RunError::Invalid(format!(
                            "--onto {m} names a task of a batch, and {name} is not in one; name a run or a branch"
                        ))
                    })?;
                    let entry = b.entry(*m).ok_or_else(|| {
                        RunError::Invalid(format!("batch {} has no task #{m}", b.id))
                    })?;
                    let run = RunState::load(home, &entry.run).map_err(|_| {
                        RunError::Invalid(format!(
                            "task #{m} never started, so there is nothing to build on"
                        ))
                    })?;
                    refuse_onto(home, &old, &run, &name)?;
                    onto_run(api, home, admin, run, opts.approve_review).await?
                }
                Onto::Run(r) => {
                    let run = combine::find_run(home, r)?;
                    refuse_onto(home, &old, &run, &name)?;
                    onto_run(api, home, admin, run, opts.approve_review).await?
                }
                Onto::Branch(br) => {
                    onto_branch(api, home, admin, &old.repo, br, opts.approve_review).await?
                }
            };
            if under(home, &stack).contains(&old.id)
                || matches!(&stack.onto, StackOnto::Run { run, .. } if run == &old.id)
            {
                return Err(RunError::Invalid(format!(
                    "{} already builds on {name}; rerun that task instead",
                    base.label
                )));
            }
            if stack.root != old.root {
                return Err(RunError::Invalid(format!(
                    "{} and {name} started from different commits; --onto needs work on the same base",
                    base.label
                )));
            }
            (Some(stack), Some(base))
        }
    };
    let rev = match &stack {
        Some(s) => s.commit.clone(),
        None => old.base_commit.clone(),
    };
    let context = base.as_ref().map(|b| {
        let mut text = format!(
            "This task starts on {}{}: that work is already in your directory, verified and accepted. Build on it; do not redo or undo it.",
            b.label,
            b.branch
                .as_deref()
                .map(|br| format!(", accepted as {br}"))
                .unwrap_or_default()
        );
        let conflict = old
            .batch
            .as_ref()
            .and_then(|r| record::last(home, &r.id).map(|rec| (r.number, rec)))
            .and_then(|(n, rec)| {
                rec.excluded.into_iter().find_map(|e| match e.why {
                    record::ExcludedWhy::Conflict { with, paths } if e.number == n => {
                        Some((with, paths))
                    }
                    _ => None,
                })
            });
        if let Some((with, paths)) = conflict {
            text.push_str(&format!(
                " Its earlier run conflicted with #{with} at {}; change those places on top of what is there now.",
                paths.join(", ")
            ));
        }
        text
    });
    let siblings = batch
        .map(|b| {
            b.tasks
                .iter()
                .filter(|t| Some(t.number) != old.batch.as_ref().map(|r| r.number))
                .map(|t| format!("#{} {}", t.number, t.title))
                .collect()
        })
        .unwrap_or_default();
    let spec = RunSpec {
        repo: old.repo.clone(),
        rev,
        prompt,
        title: Some(title),
        criteria,
        protected,
        agents: opts.agents,
        budget: opts.budget.unwrap_or_else(|| old.budget.clone()),
        deps: opts.deps,
        git_view: opts.git_view,
        aver_bin: opts.aver_bin,
        ports: opts.ports,
        leases: Vec::new(),
        id: Some(opts.id.unwrap_or_else(state::new_run_id)),
        batch: old.batch.clone(),
        siblings,
        stacked_on: stack,
        context,
    };
    Ok(Rerun {
        spec,
        batch: old.batch.clone(),
        old: old.run,
        onto: base,
    })
}

/// Records the new run as the task's run in its batch, the old one moving
/// to its history (a task that never started has none), under the batch's
/// lock. Outside a batch, nothing.
pub fn record(home: &Home, rerun: &Rerun) -> io::Result<()> {
    let (Some(b), Some(new)) = (&rerun.batch, &rerun.spec.id) else {
        return Ok(());
    };
    BatchState::update(home, &b.id, |state| {
        if let Some(e) = state.tasks.iter_mut().find(|t| t.number == b.number) {
            if e.run != *new {
                let old = std::mem::replace(&mut e.run, new.clone());
                if rerun.old.is_some() {
                    e.history.push(old);
                }
            }
            e.error = None;
            e.blocked = None;
            e.held = None;
        }
    })?;
    Ok(())
}

/// Records why the new run could not be prepared.
pub fn record_error(home: &Home, rerun: &Rerun, error: &str) {
    if let Some(b) = &rerun.batch {
        let _ = BatchState::update(home, &b.id, |state| {
            if let Some(e) = state.tasks.iter_mut().find(|t| t.number == b.number) {
                e.error = Some(error.to_owned());
            }
        });
    }
}

/// The `aver rerun` lines a combine's left-out tasks call for: each on the
/// task it conflicted with, or on the combine's branch.
pub fn hints(report: &combine::Report) -> Vec<String> {
    let mut out = Vec::new();
    for l in report.excluded.iter().filter(|l| !l.task.workspace) {
        let onto = match (&report.branch, &l.why) {
            (Some(branch), _) => branch.clone(),
            (None, LeftWhy::Conflict { with, .. }) => {
                with.number.map_or(with.run.clone(), |n| n.to_string())
            }
            (None, LeftWhy::NotPass { .. }) => continue,
        };
        let me = l.task.number.map_or(l.task.run.clone(), |n| n.to_string());
        out.push(format!("aver rerun {me} --onto {onto}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onto_reads_a_number_a_run_or_a_branch() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path().to_owned());
        assert_eq!(Onto::parse(&home, "2"), Onto::Task(2));
        assert_eq!(Onto::parse(&home, "#3"), Onto::Task(3));
        assert_eq!(
            Onto::parse(&home, "aver/combine-a-and-b"),
            Onto::Branch("aver/combine-a-and-b".into())
        );
        assert_eq!(Onto::parse(&home, "att_x"), Onto::Run("att_x".into()));
    }
}
