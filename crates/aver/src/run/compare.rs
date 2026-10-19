//! The comparison of a run's attempts (§2.10): each attempt's latest
//! receipt and diff, ranked by [`aver_core::rank::rank`], rendered as the
//! table `aver run` ends with and `aver status` prints.

use std::fmt::Write;

use aver_core::rank::{self, Candidate, Ranked};
use aver_core::{Cause, ReceiptBody, ReceiptId, SigningKey, SnapshotId, Transplant, Verdict};

use super::api::Api;
use super::cross::CrossReport;
use super::state::{AgentRun, AgentState, RunState};
use super::view::elapsed;
use crate::diagnostics::Diagnostics;
use crate::keys::{AgentKey, Home};
use crate::style;

/// What the comparison knows about one attempt.
#[derive(Debug, Clone)]
pub struct Compared {
    pub agent: AgentRun,
    pub receipt: Option<ReceiptId>,
    pub verdict: Option<Verdict>,
    pub criteria: u32,
    /// `(criterion, reason)` for each unproven criterion.
    pub unproven: Vec<(String, String)>,
    /// Changed source files no check covers (§1.9.3), which hold the
    /// verdict at `UNPROVEN`.
    pub uncovered: u64,
    pub new: u32,
    /// The ids of the `NEW` entries: what the change broke.
    pub new_entries: Vec<String>,
    pub flaky: u32,
    pub preexisting: u32,
    pub changed: u32,
    /// The tests that prove the change ([`proofs`]).
    pub proofs: u32,
    /// The latest receipt's head snapshot.
    pub head: Option<SnapshotId>,
    /// The test files the head added or modified (repository paths).
    pub test_files: Vec<String>,
    /// Those of them the base lacks.
    pub added_tests: Vec<String>,
    /// The cross-check: per other agent, its tests that fail on this
    /// attempt's code (repository content).
    pub cross: Vec<(u32, Vec<String>)>,
    pub cross_failed: u32,
    /// `None` when the diff could not be read (the workspace is gone).
    pub diff: Option<(u32, u32, u32)>,
    pub verifies: u32,
}

impl Compared {
    fn new(agent: &AgentRun) -> Self {
        Self {
            agent: agent.clone(),
            receipt: None,
            verdict: None,
            criteria: 0,
            unproven: Vec::new(),
            uncovered: 0,
            new: 0,
            new_entries: Vec::new(),
            flaky: 0,
            preexisting: 0,
            changed: 0,
            proofs: 0,
            head: None,
            test_files: Vec::new(),
            added_tests: Vec::new(),
            cross: Vec::new(),
            cross_failed: 0,
            diff: None,
            verifies: 0,
        }
    }

    pub fn candidate(&self) -> Candidate {
        let (files, added, removed) = self.diff.unwrap_or((0, 0, 0));
        Candidate {
            attempt: self.agent.attempt,
            verdict: self.verdict,
            criteria: self.criteria,
            unproven: self.unproven.len() as u32,
            new: self.new,
            flaky: self.flaky,
            changed: self.changed,
            changed_files: files,
            changed_lines: added + removed,
            wall_ms: self.agent.wall_ms(),
            gave_up: self.agent.gave_up.is_some(),
            empty: self.diff.is_some_and(|(f, _, _)| f == 0),
            proofs: self.proofs,
            cross_failed: self.cross_failed,
        }
    }
}

/// The tests that prove a change: with criteria, the distinct entries that
/// prove one, failed (or did not load) on the transplant, passed in the
/// head-tests run and have no cause; without criteria, the tests that show
/// the change (diagnostics' evidence).
pub fn proofs(body: &ReceiptBody, diagnostics: Option<&Diagnostics>) -> u32 {
    if body.criteria.as_slice().is_empty() {
        return diagnostics
            .and_then(|d| d.evidence.as_ref())
            .map_or(0, |e| e.shows_change.len() as u64 + e.shows_change_omitted)
            as u32;
    }
    body.checks
        .iter()
        .filter(|c| {
            !c.proves.is_empty()
                && c.cause.is_none()
                && matches!(c.transplant, Some(Transplant::Fail | Transplant::Absent))
                && c.head_tests == Some(Transplant::Pass)
        })
        .count() as u32
}

/// The key an agent of a run signs with.
pub fn agent_key(home: &Home, agent: &AgentRun) -> Option<AgentKey> {
    let name = agent.key.parse().ok()?;
    AgentKey::read(&home.key_path(&name)).ok()
}

/// Reads every attempt's latest receipt and diff, and
/// the run's cross-check when it has one.
pub async fn gather(api: &Api, home: &Home, admin: &SigningKey, run: &RunState) -> Vec<Compared> {
    let mut out = Vec::new();
    for agent in &run.agents {
        let mut c = Compared::new(agent);
        if let Ok(attempt) = api.attempt(admin, &agent.attempt).await {
            c.verifies = attempt.receipts.len() as u32;
            if let Some(id) = attempt.receipts.last() {
                if let Ok(v) = api.receipt(id).await {
                    let body = &v.receipt.body;
                    c.receipt = Some(*id);
                    c.verdict = Some(body.verdict);
                    c.criteria = body.criteria.as_slice().len() as u32;
                    c.uncovered = body.uncovered.as_ref().map_or(0, |u| u.total());
                    c.head = Some(body.head_snapshot);
                    c.proofs = proofs(body, v.diagnostics.as_ref());
                    let (added, modified) = test_files(body, v.diagnostics.as_ref());
                    c.test_files = added.iter().chain(&modified).cloned().collect();
                    c.added_tests = added;
                    for u in body.unproven.iter() {
                        let reason = v
                            .diagnostics
                            .as_ref()
                            .and_then(|d| d.unproven.iter().find(|x| x.criterion == *u))
                            .map(|d| d.reason.as_str().to_owned())
                            .unwrap_or_else(|| "unproven".into());
                        c.unproven.push((u.to_string(), reason));
                    }
                    for check in body.checks.iter() {
                        match check.cause {
                            Some(Cause::New) => {
                                c.new += 1;
                                c.new_entries.push(check.id.to_string());
                            }
                            Some(Cause::Flaky) => c.flaky += 1,
                            Some(Cause::Preexisting) => c.preexisting += 1,
                            Some(Cause::Changed) => c.changed += 1,
                            _ => {}
                        }
                    }
                }
            }
        }
        if let Some(key) = agent_key(home, agent) {
            if let Ok(files) = api.diff(&key.signing_key, &agent.workspace).await {
                let (mut added, mut removed) = (0, 0);
                for f in &files {
                    let (a, r) = f.lines();
                    added += a;
                    removed += r;
                }
                c.diff = Some((files.len() as u32, added, removed));
            }
        }
        out.push(c);
    }
    if let Some(report) = CrossReport::read(home, &run.id) {
        apply_cross(&mut out, &report);
    }
    out
}

/// The test files a head added and those it modified, from the
/// diagnostics' longer lists when there are some, else the receipt's.
fn test_files(body: &ReceiptBody, diagnostics: Option<&Diagnostics>) -> (Vec<String>, Vec<String>) {
    if let Some(t) = diagnostics.and_then(|d| d.test_changes.as_ref()) {
        return (t.files_added.items.clone(), t.files_modified.items.clone());
    }
    body.test_changes
        .as_ref()
        .map_or_else(Default::default, |t| {
            let strings = |l: &aver_core::Listed<aver_core::Path>| {
                l.items.iter().map(|p| p.to_string()).collect::<Vec<_>>()
            };
            (strings(&t.files_added), strings(&t.files_modified))
        })
}

/// Puts a cross-check's failures on the attempts whose code they ran on.
pub fn apply_cross(rows: &mut [Compared], report: &CrossReport) {
    for c in rows {
        c.cross = report.against(c.agent.index);
        c.cross_failed = report.failed_on(c.agent.index);
    }
}

/// The attempts in rank order.
pub fn ranked(rows: &[Compared]) -> Vec<(Ranked, &Compared)> {
    let candidates: Vec<Candidate> = rows.iter().map(Compared::candidate).collect();
    rank::rank(&candidates)
        .into_iter()
        .map(|r| {
            let row = rows
                .iter()
                .find(|c| c.agent.attempt == r.attempt)
                .expect("ranked attempts come from the rows");
            (r, row)
        })
        .collect()
}

/// What decides the order, printed under the table.
pub const ORDER_NOTE: &str = "Order: verdict, then fewer changed tests, unproven criteria and others' tests failed, then more proofs; diff size only breaks ties.";

/// `text` padded with spaces to `width` visible characters, after `paint`
/// (which may add escape codes) is applied.
fn padded(text: &str, width: usize, paint: impl Fn(&str) -> String) -> String {
    let pad = width.saturating_sub(text.chars().count());
    format!("{}{}", paint(text), " ".repeat(pad))
}

/// The comparison table, best first, with each unproven criterion's reason
/// and the cross-check's failures under its row, and the tie and the order
/// it follows at the end.
pub fn table(rows: &[Compared]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "  #  agent   verdict          crit  proofs  new  preexisting  diff          time     verifies  cost"
    );
    let ranked = ranked(rows);
    for (r, c) in &ranked {
        let verdict = match (c.verdict, r.class) {
            (_, rank::RankClass::NoChange) => "no change".to_owned(),
            (Some(v), _) => v.as_str().to_owned(),
            (None, _) => "-".to_owned(),
        };
        let crit = if c.criteria == 0 {
            "-".to_owned()
        } else {
            format!("{}/{}", c.criteria - c.unproven.len() as u32, c.criteria)
        };
        let proofs = if c.verdict.is_some() {
            c.proofs.to_string()
        } else {
            "-".to_owned()
        };
        let (diff, diff_len) = match c.diff {
            Some((f, a, d)) => {
                let plain = format!("{f}f +{a} -{d}");
                let painted = format!(
                    "{f}f {} {}",
                    style::added(&format!("+{a}")),
                    style::removed(&format!("-{d}"))
                );
                (painted, plain.chars().count())
            }
            None => ("-".into(), 1),
        };
        let mut verifies = c.verifies.to_string();
        match c.agent.state {
            AgentState::OutOfVerifies => verifies.push_str(" budget"),
            AgentState::GaveUp => verifies.push_str(" gave up"),
            AgentState::Idle => verifies.push_str(" idle"),
            AgentState::Timeout => verifies.push_str(" timeout"),
            AgentState::Killed => verifies.push_str(" killed"),
            AgentState::Failed => verifies.push_str(" failed"),
            _ => {}
        }
        let shown = if r.class == rank::RankClass::NoChange {
            None
        } else {
            c.verdict
        };
        let _ = writeln!(
            out,
            "  {:<2} {:<7} {} {:<5} {:<7} {:<4} {:<12} {}{} {:<8} {:<9} {}",
            c.agent.index,
            c.agent.template,
            padded(&verdict, 16, |t| style::verdict(shown, t)),
            crit,
            proofs,
            c.new,
            c.preexisting,
            diff,
            " ".repeat(13usize.saturating_sub(diff_len)),
            elapsed(c.agent.wall_ms()),
            verifies,
            c.agent.usage.short(),
        );
        for (id, reason) in &c.unproven {
            let _ = writeln!(out, "     {id}: {reason}");
        }
        if c.uncovered > 0 {
            let _ = writeln!(
                out,
                "     {}: {}",
                crate::diagnostics::UNCOVERED_NOTE,
                c.uncovered
            );
        }
        for (of, tests) in &c.cross {
            let _ = writeln!(
                out,
                "     fails {} of #{of}'s {}:",
                tests.len(),
                if tests.len() == 1 { "test" } else { "tests" }
            );
            for line in super::report::fence(&tests.join("\n")).lines() {
                let _ = writeln!(out, "       {line}");
            }
        }
        if let Some(why) = &c.agent.gave_up {
            let _ = writeln!(out, "     gave up: {}", super::report::one_line_plain(why));
        }
    }
    if let Some((first, _)) = ranked.first() {
        if let Some(tie) = &first.tie {
            let number = |attempt| {
                ranked
                    .iter()
                    .find(|(r, _)| r.attempt == attempt)
                    .map_or(0, |(_, c)| c.agent.index)
            };
            let (a, b) = (number(first.attempt), number(tie.with));
            let _ = writeln!(
                out,
                "  Tie on evidence between #{a} and #{b}, decided by {}: review with `aver diff {a} {b}`",
                tie.by.as_str()
            );
        }
    }
    if ranked.len() > 1 {
        let _ = writeln!(out, "  {ORDER_NOTE}");
    }
    out
}

/// The next commands, naming the best attempt and the runner-up. Accepting
/// is offered only for a best attempt `aver accept` takes: a `PASS` that
/// changed something, or a `REVIEW` with a person's approval.
pub fn hints(rows: &[Compared]) -> String {
    let ranked = ranked(rows);
    let Some((_, best)) = ranked.first() else {
        return String::new();
    };
    let a = best.agent.index;
    let changed = best.diff.is_some_and(|d| d.0 > 0);
    let accept = match best.verdict {
        Some(Verdict::Pass) if changed => format!("aver accept {a} [--pr]   "),
        Some(Verdict::Review) if changed => format!("aver accept {a} --approve-review   "),
        _ => String::new(),
    };
    match ranked.get(1) {
        None => format!("  {accept}aver diff {a}   aver discard\n"),
        Some((_, next)) => format!(
            "  {accept}aver diff {a} {} [--tool <cmd>]   aver discard\n",
            next.agent.index
        ),
    }
}
