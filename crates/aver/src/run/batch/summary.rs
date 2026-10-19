//! The batch summary (§2.10, batches): one line per task with what it came
//! to, the counts on top, the files two tasks both changed, and the next
//! commands. `aver run` prints it live and at the end; `aver status`
//! prints it again from the run files and receipts.

use std::fmt::Write as _;

use aver_core::rank::RankClass;
use aver_core::Verdict;
use serde::Serialize;

use super::super::compare::{self, Compared};
use super::super::report::one_line_plain;
use super::super::state::{AgentState, RunState, StackOnto};
use super::super::view::{elapsed, fit, fit_words};
use super::record::{CombineRecord, CombineStatus, ExcludedWhy};
use super::{BatchBudget, Spent};
use crate::coord::Overlap;

/// What one task came to, or where it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Queued,
    Running {
        state: AgentState,
        action: String,
        /// The verdict of its latest verify so far.
        verdict: Option<Verdict>,
    },
    /// `PASS`: ready to accept. The lines it added and removed, unless its
    /// directory is gone (accepted or discarded).
    Ready {
        added: Option<u32>,
        removed: Option<u32>,
    },
    /// `FAIL_OWN`: the ids of the entries it broke.
    Broke {
        entries: Vec<String>,
    },
    /// `REVIEW`: it changed this many test expectations.
    Review {
        changed: u32,
    },
    /// `(criterion, reason)` for each unproven criterion.
    Unproven {
        unproven: Vec<(String, String)>,
        /// Changed source files no check covers.
        uncovered: u64,
    },
    /// `BLOCKED_NOT_OWN`.
    Blocked,
    NoChange,
    /// The agents stopped without a receipt.
    NoResult {
        state: AgentState,
    },
    /// The run could not be prepared.
    Failed {
        error: String,
    },
    /// The batch stopped before the task started.
    NotStarted,
    /// Its prerequisites (`after: #n`) have not all ended, or (with
    /// `--no-auto-accept`) passed but are not accepted yet.
    Waiting {
        after: Vec<u32>,
    },
    /// Prerequisite `after` did not pass (or its work could not be
    /// stacked), so the task never started.
    BlockedBy {
        after: u32,
        why: String,
        /// Every task it builds on (`after: #n`), `after` among them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        prerequisites: Vec<u32>,
    },
    /// Held behind running tasks it is predicted to collide with.
    Held {
        behind: Vec<u32>,
        paths: Vec<String>,
    },
    /// The batch's budget (`--batch-timeout`, `--batch-verifies`,
    /// `--batch-usd`) ran out before the task started or finished.
    OverBudget {
        why: String,
    },
}

/// The header's categories, in order.
const CATEGORIES: &[&str] = &[
    "ready",
    "broke something",
    "needs review",
    "unproven",
    "blocked",
    "no change",
    "no result",
    "failed",
    "blocked by a prerequisite",
    "over budget",
    "running",
    "held",
    "waiting",
    "queued",
    "not started",
];

impl Outcome {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::Broke { .. } => "broke something",
            Self::Review { .. } => "needs review",
            Self::Unproven { .. } => "unproven",
            Self::Blocked => "blocked",
            Self::NoChange => "no change",
            Self::NoResult { .. } => "no result",
            Self::Failed { .. } => "failed",
            Self::Running { .. } => "running",
            Self::Queued => "queued",
            Self::NotStarted => "not started",
            Self::Waiting { .. } => "waiting",
            Self::BlockedBy { .. } => "blocked by a prerequisite",
            Self::Held { .. } => "held",
            Self::OverBudget { .. } => "over budget",
        }
    }

    /// The verdict column: the verdict, or where the task is.
    pub fn label(&self) -> String {
        match self {
            Self::Ready { .. } => "PASS".into(),
            Self::Broke { .. } => "FAIL_OWN".into(),
            Self::Review { .. } => "REVIEW".into(),
            Self::Unproven { .. } => "UNPROVEN".into(),
            Self::Blocked => "BLOCKED_NOT_OWN".into(),
            Self::NoChange => "no change".into(),
            Self::NoResult { .. } => "no result".into(),
            Self::Failed { .. } => "failed".into(),
            Self::Running { state, .. } => state.as_str().into(),
            Self::Queued => "queued".into(),
            Self::NotStarted => "-".into(),
            Self::Waiting { .. } => "waiting".into(),
            Self::BlockedBy { .. } => "blocked".into(),
            Self::Held { .. } => "held".into(),
            Self::OverBudget { .. } => "budget".into(),
        }
    }

    /// The verdict a final outcome stands for, for its colour.
    pub fn verdict(&self) -> Option<Verdict> {
        match self {
            Self::Ready { .. } => Some(Verdict::Pass),
            Self::Broke { .. } => Some(Verdict::FailOwn),
            Self::Review { .. } => Some(Verdict::Review),
            Self::Unproven { .. } => Some(Verdict::Unproven),
            Self::Blocked => Some(Verdict::BlockedNotOwn),
            _ => None,
        }
    }

    /// Whether the task ran to an outcome: something to diff and accept.
    pub fn has_result(&self) -> bool {
        self.is_final()
            && !matches!(
                self,
                Self::NotStarted | Self::BlockedBy { .. } | Self::OverBudget { .. }
            )
    }

    /// Whether the task will change no more in this batch.
    pub fn is_final(&self) -> bool {
        !matches!(
            self,
            Self::Queued | Self::Running { .. } | Self::Waiting { .. } | Self::Held { .. }
        )
    }

    /// The outcome of a finished run: what its best attempt came to.
    /// Returns the best attempt's index too.
    pub fn of_run(compared: &[Compared]) -> (Self, Option<u32>) {
        let Some((r, c)) = compare::ranked(compared).into_iter().next() else {
            return (
                Self::NoResult {
                    state: AgentState::Failed,
                },
                None,
            );
        };
        let outcome = match (r.class, c.verdict) {
            (RankClass::NoChange, _) => Self::NoChange,
            (_, Some(Verdict::Pass)) => Self::Ready {
                added: c.diff.map(|d| d.1),
                removed: c.diff.map(|d| d.2),
            },
            (_, Some(Verdict::FailOwn)) => Self::Broke {
                entries: c.new_entries.clone(),
            },
            (_, Some(Verdict::Review)) => Self::Review { changed: c.changed },
            (_, Some(Verdict::Unproven)) => Self::Unproven {
                unproven: c.unproven.clone(),
                uncovered: c.uncovered,
            },
            (_, Some(Verdict::BlockedNotOwn)) => Self::Blocked,
            (_, None) => Self::NoResult {
                state: c.agent.state,
            },
        };
        (outcome, Some(c.agent.index))
    }

    /// A run in progress, from its agents' states, last actions and latest
    /// verdicts: the busiest agent's state and action, the best verdict.
    pub fn running<'a>(
        agents: impl IntoIterator<Item = (AgentState, &'a str, Option<Verdict>)>,
    ) -> Self {
        let busy = |s: AgentState| match s {
            AgentState::Verifying | AgentState::Checking => 3,
            AgentState::Working => 2,
            AgentState::Preparing => 1,
            _ => 0,
        };
        let good = |v: Verdict| match v {
            Verdict::Pass => 4,
            Verdict::Review => 3,
            Verdict::Unproven => 2,
            Verdict::BlockedNotOwn => 1,
            Verdict::FailOwn => 0,
        };
        let mut state = AgentState::Preparing;
        let mut action = String::new();
        let mut verdict: Option<Verdict> = None;
        let mut first = true;
        for (s, a, v) in agents {
            if first || busy(s) > busy(state) {
                state = s;
                action = a.to_owned();
                first = false;
            }
            if let Some(v) = v {
                if verdict.is_none_or(|b| good(v) > good(b)) {
                    verdict = Some(v);
                }
            }
        }
        Self::Running {
            state,
            action,
            verdict,
        }
    }
}

/// An accepted task's branch and pull request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Accepted {
    pub branch: Option<String>,
    pub pr: Option<String>,
}

/// One task's line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskSummary {
    pub number: u32,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<u64>,
    /// The task's run id.
    pub run: String,
    /// The best attempt's index in the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(flatten)]
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted: Option<Accepted>,
    /// What its run builds on, when not the batch's base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stacked_on: Option<StackOnto>,
}

impl TaskSummary {
    /// The line of a task whose run is `run` (`None` before it started).
    pub fn of(
        number: u32,
        title: &str,
        issue: Option<u64>,
        run_id: &str,
        run: Option<&RunState>,
        compared: Option<&[Compared]>,
        batch_alive: bool,
    ) -> Self {
        let (outcome, attempt) = match (run, compared) {
            (None, _) if batch_alive => (Outcome::Queued, None),
            (None, _) => (Outcome::NotStarted, None),
            (Some(r), _) if r.supervisor_alive() => (
                Outcome::running(
                    r.agents
                        .iter()
                        .map(|a| (a.state, a.last_action.as_deref().unwrap_or(""), None)),
                ),
                None,
            ),
            (Some(_), Some(c)) => Outcome::of_run(c),
            (Some(r), None) => (
                Outcome::NoResult {
                    state: r.agents.first().map_or(AgentState::Failed, |a| a.state),
                },
                None,
            ),
        };
        let accepted = run.and_then(|r| r.decided.as_ref()).map(|d| Accepted {
            branch: d.branch.clone(),
            pr: d.pr.clone(),
        });
        let attempt = attempt.or(run.and_then(|r| r.decided.as_ref()).map(|d| d.index));
        Self {
            number,
            title: title.to_owned(),
            issue,
            run: run_id.to_owned(),
            attempt,
            outcome,
            accepted,
            stacked_on: run
                .and_then(|r| r.stacked_on.as_ref())
                .map(|s| s.onto.clone()),
        }
    }
}

/// Two tasks of the batch changed the same file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Collision {
    /// The two tasks' numbers, lower first.
    pub tasks: [u32; 2],
    pub path: String,
    pub overlap: Overlap,
}

/// The whole batch.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub batch: String,
    pub tasks: Vec<TaskSummary>,
    pub collisions: Vec<Collision>,
    /// The batch's limits and what it spent, when it has limits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetStatus>,
    /// The batch's last combine (`batches/<id>/combine.json`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_combine: Option<CombineRecord>,
}

/// A batch's limits against what it spent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BudgetStatus {
    pub budget: BatchBudget,
    pub spent: Spent,
    /// The batch's wall time so far (or in all, once it ended).
    pub wall_ms: u64,
    /// Which limit ran out, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub over: Option<String>,
}

impl BudgetStatus {
    /// `$25.00 · 60 verifies · 2h00m · 2 retries on failed, no-result, timeout`
    pub fn limits(&self) -> String {
        let b = &self.budget;
        let mut parts = Vec::new();
        if let Some(usd) = b.usd {
            parts.push(format!("${usd:.2}"));
        }
        if let Some(v) = b.verifies {
            parts.push(plural(v, "verify", "verifies"));
        }
        if let Some(ms) = b.wall_ms {
            parts.push(elapsed(ms));
        }
        if b.retries > 0 {
            let on: Vec<&str> = b.retry_on.iter().map(|r| r.as_str()).collect();
            parts.push(format!(
                "{} on {}",
                plural(b.retries, "retry", "retries"),
                on.join(", ")
            ));
        }
        parts.join(" · ")
    }

    /// `spent $12.40/$25.00 · 41/60 verifies · 1h02m/2h00m`
    pub fn line(&self) -> String {
        let (b, s) = (&self.budget, &self.spent);
        let mut parts = vec![match b.usd {
            Some(usd) => format!("spent ${:.2}/${usd:.2}", s.usd),
            None => format!("spent ${:.2}", s.usd),
        }];
        parts.push(match b.verifies {
            Some(v) => format!("{}/{v} verifies", s.verifies),
            None => plural(s.verifies, "verify", "verifies"),
        });
        parts.push(match b.wall_ms {
            Some(ms) => format!("{}/{}", elapsed(self.wall_ms), elapsed(ms)),
            None => elapsed(self.wall_ms),
        });
        if let Some(why) = &self.over {
            parts.push(format!("over budget: {why}"));
        }
        parts.join(" · ")
    }
}

/// An entry id as a person reads it: the test's own name (the last part of
/// its suite chain), or the path of a protected or judging entry.
fn entry_name(id: &str) -> String {
    let name = match id.split_once("::") {
        Some((_, test)) => test.rsplit(" > ").next().unwrap_or(test),
        None => id,
    };
    one_line_plain(name).chars().take(60).collect()
}

/// How to start task `n` on the work of `on`: onto the one task, or onto
/// a combine of several, so none of them is left out.
fn rerun_onto(n: u32, on: &[u32]) -> String {
    match on {
        [one] => format!("`aver rerun {n} --onto {one}`"),
        _ => {
            let all: Vec<String> = on.iter().map(u32::to_string).collect();
            format!(
                "`aver combine {}`, then `aver rerun {n} --onto <the combine's branch>`",
                all.join(" ")
            )
        }
    }
}

/// `#1, #3`
fn numbers(ns: &[u32]) -> String {
    ns.iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Summary {
    fn count(&self, category: &str) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.outcome.category() == category)
            .count()
    }

    /// `8 tasks · 5 ready · 2 broke something · 1 needs review`
    pub fn counts(&self) -> String {
        let mut parts = vec![format!(
            "{} {}",
            self.tasks.len(),
            if self.tasks.len() == 1 {
                "task"
            } else {
                "tasks"
            }
        )];
        for c in CATEGORIES {
            let n = self.count(c);
            if n > 0 {
                parts.push(format!("{n} {c}"));
            }
        }
        parts.join(" · ")
    }
}

/// A task's verdict and what it means, as its line in the table says.
pub fn outcome_text(t: &TaskSummary) -> String {
    format!("{} {}", t.outcome.label(), detail(t))
        .trim_end()
        .to_owned()
}

/// What a task's line says after its verdict.
fn detail(t: &TaskSummary) -> String {
    let accepted = t.accepted.as_ref().map(|a| match (&a.pr, &a.branch) {
        (Some(pr), _) => format!("PR {pr}"),
        (None, Some(b)) => format!("accepted as {b}"),
        (None, None) => "accepted".to_owned(),
    });
    match &t.outcome {
        Outcome::Ready { added, removed } => {
            let next = accepted.unwrap_or_else(|| format!("aver accept {}", t.number));
            match (added, removed) {
                (Some(a), Some(r)) => format!("{:<8} {next}", format!("+{a} -{r}")),
                _ => next,
            }
        }
        Outcome::Broke { entries } => {
            let first = entries
                .first()
                .map(|e| format!("broke {:?}", entry_name(e)))
                .unwrap_or_else(|| "broke something".into());
            match entries.len() {
                0 | 1 => first,
                n => format!("{first} and {} more", n - 1),
            }
        }
        Outcome::Review { changed } => accepted.unwrap_or_else(|| {
            format!(
                "changed {}: aver diff {}",
                plural(*changed, "test expectation", "test expectations"),
                t.number
            )
        }),
        Outcome::Unproven {
            unproven,
            uncovered,
        } => {
            let first = unproven
                .first()
                .map(|(id, why)| format!("not proven: {id} ({why})"));
            let first = match (first, *uncovered) {
                (Some(first), _) => first,
                (None, 0) => "not proven".into(),
                (None, n) => format!(
                    "{}: {}",
                    crate::diagnostics::UNCOVERED_NOTE,
                    plural(n as u32, "file", "files")
                ),
            };
            match unproven.len() {
                0 | 1 => first,
                n => format!("{first} and {} more", n - 1),
            }
        }
        Outcome::Blocked => format!(
            "flaky tests or infrastructure, not the change: aver status {}",
            t.run
        ),
        Outcome::NoChange => "the agents changed nothing".into(),
        Outcome::NoResult { state } => format!("{}, no receipt", state.as_str()),
        Outcome::Failed { error } => one_line_plain(error),
        Outcome::Running {
            action, verdict, ..
        } => {
            let action = one_line_plain(action);
            match verdict {
                Some(v) => format!("{action} (last verify {})", v.as_str()),
                None => action,
            }
        }
        Outcome::Queued => String::new(),
        Outcome::NotStarted => "the batch stopped before it started".into(),
        Outcome::Waiting { after } => format!("after {}", numbers(after)),
        Outcome::BlockedBy { after, why, .. } => {
            format!("not started: #{after} {}", one_line_plain(why))
        }
        Outcome::Held { behind, paths } => {
            let paths: Vec<String> = paths.iter().map(|p| one_line_plain(p)).collect();
            match paths.as_slice() {
                [] => format!("behind {}", numbers(behind)),
                _ => format!(
                    "behind {}: both change {}",
                    numbers(behind),
                    paths.join(", ")
                ),
            }
        }
        Outcome::OverBudget { why } => one_line_plain(why),
    }
}

impl Summary {
    /// One line per task, then the collisions.
    pub fn table(&self, width: usize) -> String {
        let num_w = self
            .tasks
            .iter()
            .map(|t| t.number.to_string().len() + 1)
            .max()
            .unwrap_or(2);
        // Titles take what the terminal leaves after the number, the
        // verdict and about 40 columns of what it came to.
        let title_w = self
            .tasks
            .iter()
            .map(|t| one_line_plain(&t.title).chars().count())
            .max()
            .unwrap_or(10)
            .clamp(10, width.saturating_sub(num_w + 10 + 40).max(10));
        let mut out = String::new();
        for t in &self.tasks {
            let title = fit_words(&one_line_plain(&t.title), title_w);
            let label = t.outcome.label();
            let line = format!(
                "{:<num_w$} {title:<title_w$} {label:<8} {}",
                format!("#{}", t.number),
                detail(t)
            );
            let line = fit(line.trim_end(), width).trim_end().to_owned();
            // Colour the verdict after cutting, so the cut counts only
            // visible characters.
            let at = num_w + 1 + title_w + 1;
            let line = match t.outcome.verdict() {
                Some(v) if line.get(at..).is_some_and(|rest| rest.starts_with(&label)) => {
                    format!(
                        "{}{}{}",
                        &line[..at],
                        crate::style::verdict(Some(v), &label),
                        &line[at + label.len()..]
                    )
                }
                _ => line,
            };
            let _ = writeln!(out, "{line}");
        }
        for c in &self.collisions {
            let how = match c.overlap {
                Overlap::Lines => "overlapping lines",
                Overlap::File => "the same file",
                Overlap::Disjoint => "different lines",
            };
            let _ = writeln!(
                out,
                "{}",
                fit(
                    &format!(
                        "overlap: #{} and #{} both change {} ({how})",
                        c.tasks[0],
                        c.tasks[1],
                        one_line_plain(&c.path)
                    ),
                    width
                )
                .trim_end()
            );
        }
        out
    }

    /// The next commands, for a finished batch.
    pub fn hints(&self) -> String {
        let open = |t: &&TaskSummary| t.accepted.is_none();
        let ready: Vec<&TaskSummary> = self
            .tasks
            .iter()
            .filter(|t| matches!(t.outcome, Outcome::Ready { .. }))
            .collect();
        let mut out = String::new();
        let colliding: Vec<&Collision> = self
            .collisions
            .iter()
            .filter(|c| c.tasks.iter().all(|n| ready.iter().any(|t| t.number == *n)))
            .collect();
        // The last combine, when it already took in every ready task: running
        // it again gives the same result.
        let tried = self.last_combine.as_ref().filter(|r| {
            !ready.is_empty()
                && ready.iter().all(|t| {
                    r.included.contains(&t.number)
                        || r.excluded.iter().any(|e| e.number == t.number)
                })
        });
        let failed =
            tried.filter(|r| matches!(r.status, CombineStatus::Conflicts | CombineStatus::NotPass));
        if let Some(r) = failed {
            let _ = writeln!(
                out,
                "The last combine of these tasks {}; running it again gives the same result.",
                if r.status == CombineStatus::Conflicts {
                    "conflicted"
                } else {
                    "did not pass"
                }
            );
            for e in &r.excluded {
                let _ = match &e.why {
                    ExcludedWhy::Conflict { with, paths } => writeln!(
                        out,
                        "#{} conflicts with #{with} in {}: `aver rerun {} --onto {with}` redoes it on #{with}'s work",
                        e.number,
                        paths.iter().map(|p| one_line_plain(p)).collect::<Vec<_>>().join(", "),
                        e.number
                    ),
                    ExcludedWhy::NotPass { verdict, .. } => writeln!(
                        out,
                        "#{} is {} on the merge: `aver rerun {} --onto <m>` redoes it on another task's work",
                        e.number,
                        verdict.as_str(),
                        e.number
                    ),
                };
            }
        } else if ready.len() >= 2 && !colliding.is_empty() && tried.is_none() {
            let _ = writeln!(
                out,
                "Ready tasks change the same files; `aver combine --all-pass` merges them and verifies the result against every task, or names the lines that conflict."
            );
        }
        for t in &self.tasks {
            let _ = match &t.outcome {
                Outcome::Waiting { after } => writeln!(
                    out,
                    "#{} waits for {}: {}then {}",
                    t.number,
                    numbers(after),
                    after
                        .iter()
                        .map(|p| format!("`aver accept {p}`, "))
                        .collect::<String>(),
                    rerun_onto(t.number, after)
                ),
                Outcome::BlockedBy {
                    after,
                    prerequisites,
                    ..
                } => {
                    let on = if prerequisites.is_empty() {
                        std::slice::from_ref(after)
                    } else {
                        prerequisites.as_slice()
                    };
                    writeln!(
                        out,
                        "#{} did not start: `aver rerun {after}` redoes #{after}, then {}",
                        t.number,
                        rerun_onto(t.number, on)
                    )
                }
                _ => Ok(()),
            };
        }
        if self
            .tasks
            .iter()
            .any(|t| matches!(t.outcome, Outcome::OverBudget { .. }))
        {
            let _ = writeln!(
                out,
                "The batch budget ran out; `aver rerun <n>` runs a task again, in its kept directory's place."
            );
        }
        let mut next = Vec::new();
        if ready.iter().any(open) {
            next.push("aver accept --all-pass [--pr]".to_owned());
        }
        match (failed, tried) {
            (Some(_), _) => next.push("aver combine --partial".to_owned()),
            (None, Some(_)) => {}
            (None, None) if ready.len() >= 2 => next.push("aver combine --all-pass".to_owned()),
            (None, None) => {}
        }
        if self
            .tasks
            .iter()
            .any(|t| t.outcome.has_result() && t.accepted.is_none())
        {
            next.push("aver diff <n>".to_owned());
            next.push("aver accept <n>".to_owned());
            next.push("aver discard".to_owned());
        }
        if !next.is_empty() {
            let _ = writeln!(out, "Next: {}", next.join("   "));
        }
        out
    }

    /// The counts, the budget, the table and the hints.
    pub fn render(&self, width: usize) -> String {
        let budget = self
            .budget
            .as_ref()
            .map(|b| format!("{}\n", b.line()))
            .unwrap_or_default();
        format!(
            "{}\n{budget}{}{}",
            self.counts(),
            self.table(width),
            self.hints()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(number: u32, title: &str, outcome: Outcome) -> TaskSummary {
        TaskSummary {
            number,
            title: title.into(),
            issue: None,
            run: format!("RUN00{number}"),
            attempt: Some(1),
            outcome,
            accepted: None,
            stacked_on: None,
        }
    }

    #[test]
    fn the_summary_reads_like_a_triage_list() {
        let s = Summary {
            batch: "B4TCH1".into(),
            tasks: vec![
                task(
                    1,
                    "fix ISO week parsing",
                    Outcome::Ready {
                        added: Some(12),
                        removed: Some(3),
                    },
                ),
                task(
                    4,
                    "cache user settings",
                    Outcome::Broke {
                        entries: vec![
                            "unit::test/settings.test.js > settings persist across reload".into(),
                        ],
                    },
                ),
                task(6, "rename config keys", Outcome::Review { changed: 2 }),
            ],
            collisions: Vec::new(),
            budget: None,
            last_combine: None,
        };
        assert_eq!(
            s.render(100),
            "3 tasks · 1 ready · 1 broke something · 1 needs review\n\
             #1 fix ISO week parsing PASS     +12 -3   aver accept 1\n\
             #4 cache user settings  FAIL_OWN broke \"settings persist across reload\"\n\
             #6 rename config keys   REVIEW   changed 2 test expectations: aver diff 6\n\
             Next: aver accept --all-pass [--pr]   aver diff <n>   aver accept <n>   aver discard\n"
        );
    }

    #[test]
    fn long_titles_use_the_width_and_are_cut_on_words() {
        let title = "make the importer match payees that differ only in case and spacing between bank downloads";
        let s = Summary {
            batch: "B4TCH1".into(),
            tasks: vec![task(
                1,
                title,
                Outcome::Ready {
                    added: Some(1),
                    removed: Some(0),
                },
            )],
            collisions: Vec::new(),
            budget: None,
            last_combine: None,
        };
        // Wide: the whole title, beyond the old 30 columns.
        assert!(s.table(160).contains(title), "{}", s.table(160));
        // Narrow: cut between words, the line within the width.
        let narrow = s.table(90);
        let line = narrow.lines().next().unwrap();
        assert!(line.chars().count() <= 90, "{line}");
        assert!(
            line.contains("make the importer match payees that…"),
            "{line}"
        );
        assert!(line.contains("PASS"), "{line}");
    }
}
