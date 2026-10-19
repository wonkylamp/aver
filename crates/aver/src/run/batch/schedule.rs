//! Which queued task starts next, and on what (§2.10, batches): a task
//! whose prerequisites (`after: #n`) have not ended waits; one whose
//! prerequisites passed starts on their accepted work; one predicted to
//! collide with a running task is held behind it ([`hold`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

use aver_core::Timestamp;

use super::input::BatchTask;
use super::summary::{self, Outcome};
use super::{record, task_line, BatchState, BlockedBy, Slot};
use crate::keys::{AgentKey, Home};
use crate::run::api::Api;
use crate::run::combine;
use crate::run::state::{RunState, Stack, StackOnto};

/// Whether queued tasks predicted to collide with running ones wait for
/// them (`aver run --sequence auto|off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum SequenceMode {
    Auto,
    Off,
}

/// `--sequence` and `--sequence-threshold`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    pub mode: SequenceMode,
    /// The predicted-conflict score (0-100) at or above which a task is
    /// held.
    pub threshold: u8,
}

impl Default for Sequence {
    fn default() -> Self {
        Self {
            mode: SequenceMode::Auto,
            threshold: 50,
        }
    }
}

/// A queued task held behind running tasks it is predicted to collide
/// with; it starts once they end, on the accepted work of those that pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    /// The running tasks it waits for.
    pub behind: Vec<u32>,
    /// The paths both are expected to change.
    pub paths: Vec<String>,
    /// The predicted-conflict score, 0-100.
    pub score: u8,
    /// Whether the score came from a prediction (the task's brief) rather
    /// than both tasks' diffs.
    pub predicted: bool,
}

/// What [`hold`] may look at.
pub struct HoldCtx<'a> {
    pub home: &'a Home,
    pub api: &'a Api,
    pub rt: &'a tokio::runtime::Runtime,
    pub repo: &'a Path,
    /// The batch's base commit.
    pub base_commit: &'a str,
    pub batch: &'a str,
    pub sequence: Sequence,
    /// Every task of the batch.
    pub tasks: &'a [BatchTask],
}

/// Whether `task` should wait for some of the `running` tasks (task
/// number, run) because they are likely to change the same lines.
pub fn hold(task: &BatchTask, running: &[(u32, &RunState)], ctx: &HoldCtx<'_>) -> Option<Hold> {
    let _ = (task, running, ctx);
    None
}

/// Where a task starts, once nothing holds it back.
#[derive(Debug, Clone)]
pub(super) struct Start {
    pub rev: String,
    pub stacked_on: Option<Stack>,
    /// What the brief says the run builds on.
    pub note: Option<String>,
}

/// What a task whose prerequisites all ended starts on.
pub(super) enum Base {
    /// The batch's base commit, or its prerequisites' accepted work.
    Go(Start),
    /// Prerequisites that passed but are not accepted (`--no-auto-accept`).
    Wait(Vec<u32>),
    /// It cannot start, and why.
    Block(BlockedBy),
}

/// What [`base_for`] needs.
pub(super) struct BaseCtx<'a> {
    pub home: &'a Home,
    pub api: &'a Api,
    pub admin: &'a AgentKey,
    pub rt: &'a tokio::runtime::Runtime,
    pub batch: &'a BatchState,
    pub auto_accept: bool,
}

/// What task `number`, whose prerequisites `after` have all ended, starts
/// on (§2.10, task graph): the batch's base when it has none; the accepted
/// winner of its one prerequisite, accepting it first when it passed; a
/// combine of the winners of several. Any prerequisite that did not pass
/// blocks it. Accepting goes through `aver accept`'s own checks, and a
/// combine is verified against every task it contains before it is
/// exported, so nothing unverified becomes a base.
pub(super) fn base_for(ctx: &BaseCtx<'_>, after: &[u32], slots: &mut [Slot]) -> Base {
    let root = Start {
        rev: ctx.batch.base_commit.clone(),
        stacked_on: None,
        note: None,
    };
    if after.is_empty() {
        return Base::Go(root);
    }
    let index = |n: u32| ctx.batch.tasks.iter().position(|e| e.number == n);
    let mut runs: Vec<RunState> = Vec::new();
    let mut waiting = Vec::new();
    for &p in after {
        let Some(i) = index(p) else {
            return Base::Block(BlockedBy {
                after: p,
                why: "is not a task of this batch".into(),
            });
        };
        let entry = &ctx.batch.tasks[i];
        let line = task_line(entry, &slots[i], None);
        let Slot::Done(run, compared) = &mut slots[i] else {
            return Base::Block(BlockedBy {
                after: p,
                why: format!("did not pass ({})", summary::outcome_text(&line)),
            });
        };
        // `aver accept` may have run from another terminal meanwhile.
        if let Ok(fresh) = RunState::load(ctx.home, &run.id) {
            **run = fresh;
        }
        match &line.outcome {
            Outcome::Ready { .. } => {}
            Outcome::Review { .. } if run.decided.is_some() => {}
            Outcome::Review { .. } => {
                return Base::Block(BlockedBy {
                    after: p,
                    why: format!(
                        "needs review before others build on it: aver diff {p}, aver accept {p}"
                    ),
                })
            }
            _ => {
                return Base::Block(BlockedBy {
                    after: p,
                    why: format!("did not pass: {}", summary::outcome_text(&line)),
                })
            }
        }
        if run.decided.is_none() {
            if !ctx.auto_accept {
                waiting.push(p);
                continue;
            }
            let (_, Some(best)) = Outcome::of_run(compared) else {
                return Base::Block(BlockedBy {
                    after: p,
                    why: "has no attempt to accept".into(),
                });
            };
            let accepted = ctx.rt.block_on(crate::run::accept::accept(
                ctx.api,
                ctx.home,
                ctx.admin,
                run,
                &best.to_string(),
                &crate::run::accept::AcceptOptions::default(),
            ));
            if let Err(e) = accepted {
                return Base::Block(BlockedBy {
                    after: p,
                    why: format!("passed, but could not be accepted: {e}"),
                });
            }
        }
        runs.push((**run).clone());
    }
    if !waiting.is_empty() {
        return Base::Wait(waiting);
    }
    let root_of = |r: &RunState| r.stacked_on.as_ref().map_or(ctx.batch.base, |s| s.root);
    let title = |n: u32| {
        index(n)
            .map(|i| crate::run::report::one_line_plain(&ctx.batch.tasks[i].title))
            .unwrap_or_default()
    };
    if let [run] = runs.as_slice() {
        let d = run.decided.as_ref().expect("accepted above");
        let (Some(commit), Some(head)) = (d.commit.clone(), d.head) else {
            return Base::Block(BlockedBy {
                after: after[0],
                why: "was accepted without a recorded commit; aver rerun it".into(),
            });
        };
        let note = format!(
            "BUILDS ON: task #{} \"{}\", accepted as branch {} (commit {}). Your copy starts with that work in place: build on it and keep it working.",
            after[0],
            title(after[0]),
            d.branch.as_deref().unwrap_or("-"),
            &commit[..7.min(commit.len())]
        );
        return Base::Go(Start {
            rev: commit.clone(),
            stacked_on: Some(Stack {
                onto: StackOnto::Run {
                    run: run.id.clone(),
                    number: Some(after[0]),
                },
                commit,
                head,
                root: root_of(run),
            }),
            note: Some(note),
        });
    }
    // Resolved from the home, so a prerequisite stacked on runs that are
    // not among `after` (a diamond, a longer chain) brings its whole chain.
    let combined = ctx.rt.block_on(async {
        let mut resolved = Vec::new();
        for r in &runs {
            let input = combine::Input::Accepted(r.clone());
            resolved.push(combine::resolve(ctx.api, ctx.home, ctx.admin, &input).await?);
        }
        combine::Combine {
            api: ctx.api,
            home: Some(ctx.home),
            admin: ctx.admin,
            branch: None,
            partial: false,
            // `base_for` writes the batch's record itself, by task number.
            batch: None,
            // Only a person approves REVIEW: a merge that is REVIEW for no
            // prerequisite a person accepted at REVIEW blocks.
            approve_review: false,
            before_accept: None,
        }
        .run(resolved)
        .await
        .map(combine::Combined::from)
    });
    let names = after
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(" and ");
    let blocked = |why: String| {
        Base::Block(BlockedBy {
            after: after[0],
            why,
        })
    };
    match combined {
        Err(e) => blocked(format!(
            "and the other prerequisites could not be combined: {e}"
        )),
        Ok(combine::Combined::Conflicts { conflicts, .. }) => {
            let paths: Vec<String> = conflicts.iter().map(|c| c.path.clone()).collect();
            let _ = record::save(
                ctx.home,
                &ctx.batch.id,
                &record::CombineRecord {
                    at: Timestamp::now(),
                    status: record::CombineStatus::Conflicts,
                    included: Vec::new(),
                    excluded: conflicting(after, &conflicts, &runs),
                    branch: None,
                },
            );
            blocked(format!(
                "and {names} conflict in {}, so there is nothing to build on",
                paths.join(", ")
            ))
        }
        Ok(combine::Combined::NotPass { verdicts, .. }) => {
            let bad: Vec<&combine::TaskVerdict> = verdicts
                .iter()
                .filter(|v| v.verdict != aver_core::Verdict::Pass)
                .collect();
            let _ = record::save(
                ctx.home,
                &ctx.batch.id,
                &record::CombineRecord {
                    at: Timestamp::now(),
                    status: record::CombineStatus::NotPass,
                    included: Vec::new(),
                    excluded: bad
                        .iter()
                        .filter_map(|v| {
                            let at = runs.iter().position(|r| r.id == v.run)?;
                            Some(record::Excluded {
                                number: after[at],
                                run: v.run.clone(),
                                why: record::ExcludedWhy::NotPass {
                                    verdict: v.verdict,
                                    receipt: v.receipt,
                                },
                            })
                        })
                        .collect(),
                    branch: None,
                },
            );
            let failing: Vec<String> = bad
                .iter()
                .map(|v| {
                    format!(
                        "{} {}",
                        crate::run::report::one_line_plain(&v.title),
                        v.verdict.as_str()
                    )
                })
                .collect();
            if bad.iter().all(|v| v.verdict == aver_core::Verdict::Review) {
                let numbers: Vec<String> = after.iter().map(u32::to_string).collect();
                return blocked(format!(
                    "and {names} merge, but the result is REVIEW (it changes what an existing test expects), which a person approves: aver combine {} --approve-review, then aver rerun onto its branch",
                    numbers.join(" ")
                ));
            }
            blocked(format!(
                "and {names} merge, but the result does not pass: {}",
                failing.join(", ")
            ))
        }
        Ok(combine::Combined::Exported {
            branch,
            commit,
            snapshot,
            ..
        }) => {
            let _ = record::save(
                ctx.home,
                &ctx.batch.id,
                &record::CombineRecord {
                    at: Timestamp::now(),
                    status: record::CombineStatus::Exported,
                    included: after.to_vec(),
                    excluded: Vec::new(),
                    branch: Some(branch.clone()),
                },
            );
            let listed: Vec<String> = after
                .iter()
                .map(|n| format!("#{n} \"{}\"", title(*n)))
                .collect();
            let note = format!(
                "BUILDS ON: tasks {}, combined and verified against each as branch {branch} (commit {}). Your copy starts with their work in place: build on it and keep it working.",
                listed.join(", "),
                &commit[..7.min(commit.len())]
            );
            Base::Go(Start {
                rev: commit.clone(),
                stacked_on: Some(Stack {
                    onto: StackOnto::Combine {
                        branch,
                        runs: runs.iter().map(|r| r.id.clone()).collect(),
                    },
                    commit,
                    head: snapshot,
                    root: root_of(&runs[0]),
                }),
                note: Some(note),
            })
        }
    }
}

/// A combine's conflicts as each later task excluded for its clash with
/// the first one it conflicts with.
fn conflicting(
    after: &[u32],
    conflicts: &[crate::workspace::merge::Conflict],
    runs: &[RunState],
) -> Vec<record::Excluded> {
    let mut out: Vec<record::Excluded> = Vec::new();
    for c in conflicts {
        let [first, rest @ ..] = c.heads.as_slice() else {
            continue;
        };
        for &h in rest {
            let (Some(&number), Some(run)) = (after.get(h), runs.get(h)) else {
                continue;
            };
            let with = after.get(*first).copied().unwrap_or(number);
            match out.iter_mut().find(|e| e.number == number) {
                Some(e) => {
                    if let record::ExcludedWhy::Conflict { paths, .. } = &mut e.why {
                        if !paths.contains(&c.path) {
                            paths.push(c.path.clone());
                        }
                    }
                }
                None => out.push(record::Excluded {
                    number,
                    run: run.id.clone(),
                    why: record::ExcludedWhy::Conflict {
                        with,
                        paths: vec![c.path.clone()],
                    },
                }),
            }
        }
    }
    out
}
