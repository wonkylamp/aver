//! Rerun and verdict rules (ARCHITECTURE.md §1.9.3) as pure functions. An
//! entry's cause is [`crate::judge`]'s.
//!
//! A side's runs are the endings of one check (or one test) on base or head,
//! in the order they ran. An infra run (a resource cap or a sandbox failure)
//! is retried once, so two consecutive infra endings end the side.

use serde::{Deserialize, Serialize};

use crate::id::BlobId;
use crate::judge;
use crate::receipt::{
    BaseStatus, Cause, HeadStatus, ProofRule, ReceiptCheck, Transplant, Verdict, JUDGING_CHECK,
};
use crate::run::Ending;
use crate::scalar::CriterionId;
use crate::task::Criteria;

/// Runs a failing side needs before its result counts as a stable failure.
pub const MAX_RUNS: usize = 3;

/// The classification of one receipt entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classification {
    pub base: BaseStatus,
    pub head: HeadStatus,
    pub cause: Option<Cause>,
}

/// Whether a side needs another run under the rerun policy: each side runs
/// once, a failing side reruns until it has `max_runs` results or disagrees
/// with itself, and an infra run is retried once. The transplant's
/// `max_runs` is 1: its proving tests fail there by design, so a rerun
/// would only repeat an expected failure.
pub fn needs_rerun(runs: &[Ending], max_runs: usize) -> bool {
    match runs {
        [] => true,
        [.., a, b] if a.is_infra() && b.is_infra() => false,
        [.., last] if last.is_infra() => true,
        _ => {
            let side = Side::of(runs);
            side.agreement == Some(Agreement::Fail) && side.results < max_runs
        }
    }
}

/// How a failing run failed, as far as a rerun could change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Tests failed in a report; a rerun may pass them.
    Tests,
    /// The build stopped at a compiler error, which a rerun builds again.
    Build,
    /// The command failed as a whole: its exit code and output.
    Command { exit_code: i32, output: BlobId },
}

/// Whether a base side's failure is settled before [`MAX_RUNS`] results:
/// `failures` are its failing runs in order, and the last stopped at a
/// compiler error, or the last two failed as a whole with the same exit
/// code and the same output. A rerun of either repeats it; per-test
/// failures keep the full policy, since that is where flakiness shows.
pub fn failure_settled(failures: &[Failure]) -> bool {
    match failures {
        [.., Failure::Build] => true,
        [.., a @ Failure::Command { .. }, b] => a == b,
        _ => false,
    }
}

/// Whether the base side of an entry needs more runs before a flaky head can
/// be judged (§1.9.3): the head disagreed with itself, the base passed every
/// run, and the base has fewer than [`MAX_RUNS`] results. A base that
/// passes [`MAX_RUNS`] times is stable, so the head's failures are the
/// change's (`NEW`); one that fails on a rerun is flaky itself (`FLAKY`).
pub fn base_needs_confirmation(base_runs: &[Ending], head_runs: &[Ending]) -> bool {
    let base = Side::of(base_runs);
    Side::of(head_runs).agreement == Some(Agreement::Flaky)
        && base.agreement == Some(Agreement::Pass)
        && !base.ended_infra
        && base.results < MAX_RUNS
}

/// Whether `entry` proves the criteria in its `proves` under `rule`: it
/// passes on head with no cause and, in v2, [`judge::proves`] over what the
/// receipt recorded of its transplant and head-tests runs. A receipt does
/// not say why a transplant failed, so a weak proof is weighed by the
/// verifier, which has the failure messages (§1.9.4).
pub fn proves(entry: &ReceiptCheck, rule: ProofRule) -> bool {
    let transplant = match entry.transplant {
        Some(Transplant::Fail | Transplant::Absent) => judge::Transplant::FailOnAssertion,
        Some(Transplant::Pass) => judge::Transplant::Pass,
        Some(Transplant::Infra) => judge::Transplant::Infra,
        None => judge::Transplant::NotRun,
    };
    let own = match entry.head_tests {
        Some(Transplant::Pass) => judge::Own::Pass,
        Some(Transplant::Fail) => judge::Own::Fail,
        Some(Transplant::Absent) => judge::Own::Absent,
        Some(Transplant::Infra) => judge::Own::Infra,
        None => judge::Own::NotRun,
    };
    entry.head == HeadStatus::Pass
        && match rule {
            ProofRule::HeadPasses => entry.cause.is_none(),
            ProofRule::Transplant => judge::proves(entry.cause, own, transplant),
        }
}

/// Whether the receipt has a judging-file entry.
pub fn judging_changed(entries: &[ReceiptCheck]) -> bool {
    entries.iter().any(|e| {
        e.id.as_str()
            .split_once("::")
            .is_some_and(|(check, _)| check == JUDGING_CHECK)
    })
}

/// Whether `entry` is a command-level entry: a whole check judged by its
/// exit code, with no per-test breakdown. Its id is the check id alone;
/// per-test, exit, judging and protected-path entries all contain `::`.
pub fn is_command_level(entry: &ReceiptCheck) -> bool {
    !entry.id.as_str().contains("::")
}

/// Whether `entry` leaves the change's effect unknown (v2): a command-level
/// entry that fails on both sides, where one red check hides whatever else
/// the change did to it, or an entry that was flaky on the base and failed
/// on every head run, where the head never once passed. Either way nothing
/// says the change is safe, so the verdict cannot be `PASS`.
pub fn inconclusive(entry: &ReceiptCheck, rule: ProofRule) -> bool {
    rule == ProofRule::Transplant
        && match entry.cause {
            Some(Cause::Preexisting) => is_command_level(entry),
            Some(Cause::Flaky) => entry.head == HeadStatus::Fail,
            _ => false,
        }
}

/// Computes the unproven criteria (in criteria order) and the verdict.
///
/// A criterion is proven when an entry that proves it passes [`proves`] and,
/// in v2, no judging file changed. Verdict, first match wins: any `NEW` is
/// `FAIL_OWN`; any `ENV`, or in v2 any [`inconclusive`] entry, is
/// `BLOCKED_NOT_OWN`; nothing unproven is `REVIEW` when an entry is
/// `CHANGED` (v2 only) and `PASS` otherwise; an unproven criterion whose
/// proving entries are all `FLAKY` is `BLOCKED_NOT_OWN`, unless (v2) a
/// judging file changed; otherwise `UNPROVEN`. So `UNPROVEN` outranks `REVIEW`: a human is asked to review
/// changed test expectations only once nothing else is left to do.
pub fn verdict(
    entries: &[ReceiptCheck],
    criteria: &Criteria,
    rule: ProofRule,
) -> (Vec<CriterionId>, Verdict) {
    verdict_with_coverage(entries, criteria, rule, false)
}

/// [`verdict`] for a v2 receipt that may list `uncovered` changes (§1.9.3):
/// source files the head changed that no check could exercise. With any,
/// nothing measured part of the change, so the receipt is `UNPROVEN` where
/// it would be `PASS` or `REVIEW`; `FAIL_OWN` and `BLOCKED_NOT_OWN` stand,
/// and so does `UNPROVEN` for an unproven criterion.
pub fn verdict_with_coverage(
    entries: &[ReceiptCheck],
    criteria: &Criteria,
    rule: ProofRule,
    uncovered: bool,
) -> (Vec<CriterionId>, Verdict) {
    let judging = rule == ProofRule::Transplant && judging_changed(entries);
    let unproven: Vec<CriterionId> = criteria
        .iter()
        .map(|c| &c.id)
        .filter(|id| judging || !provers(entries, id).any(|e| proves(e, rule)))
        .cloned()
        .collect();
    let any_cause = |cause: Cause| entries.iter().any(|e| e.cause == Some(cause));
    // With a judging change nothing proves, flaky or not: the reason is the
    // harness edit, so such a receipt is UNPROVEN, not BLOCKED_NOT_OWN.
    let blocked_by_flaky = !judging
        && unproven.iter().any(|id| {
            let mut provers = provers(entries, id).peekable();
            provers.peek().is_some() && provers.all(|e| e.cause == Some(Cause::Flaky))
        });
    let verdict = if any_cause(Cause::New) {
        Verdict::FailOwn
    } else if any_cause(Cause::Env) || entries.iter().any(|e| inconclusive(e, rule)) {
        Verdict::BlockedNotOwn
    } else if unproven.is_empty() && !uncovered && any_cause(Cause::Changed) {
        Verdict::Review
    } else if unproven.is_empty() && !uncovered {
        Verdict::Pass
    } else if blocked_by_flaky {
        Verdict::BlockedNotOwn
    } else {
        Verdict::Unproven
    };
    (unproven, verdict)
}

/// What the runs cannot show whatever they found, so a verdict that would
/// be `PASS` is not (§1.9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cap {
    /// No check that counts tests ran one on the original code (a type
    /// check alone, or exit codes alone): nothing ran the change. `PASS`
    /// and `REVIEW` become `UNPROVEN`.
    NoRuntimeTests,
    /// The contract the checks ran is the change's own `aver.toml`, which
    /// a person must approve: `PASS` becomes `REVIEW`.
    ProposedContract,
}

impl Cap {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoRuntimeTests => "no_runtime_tests",
            Self::ProposedContract => "proposed_contract",
        }
    }
}

/// `verdict` capped by what the runs cannot show, and the cap that
/// applied. Never raises a verdict: `UNPROVEN` outranks `REVIEW`, so no
/// runtime tests is weighed first.
pub fn cap(verdict: Verdict, runtime_tests: bool, proposed: bool) -> (Verdict, Option<Cap>) {
    match verdict {
        Verdict::Pass | Verdict::Review if !runtime_tests => {
            (Verdict::Unproven, Some(Cap::NoRuntimeTests))
        }
        Verdict::Pass if proposed => (Verdict::Review, Some(Cap::ProposedContract)),
        v => (v, None),
    }
}

/// A changed region of a source file, in the head's lines.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HunkAt {
    pub path: crate::scalar::Path,
    pub lines: crate::pushback::LineRange,
}

/// What reverting each source hunk of a change found (§1.9.4, hunk
/// necessity), for a verdict that would otherwise be `PASS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AblationOutcome {
    /// Every hunk reverted made a proving test fail (or the run fail).
    AllKilled,
    /// These hunks, reverted, left every proving test passing.
    SurvivedHunk(Vec<HunkAt>),
    /// The phase did not run: the verdict was not `PASS`, there was no
    /// proving test, a proving test could not be selected by name, or the
    /// change had too many hunks.
    NotRun,
}

/// `verdict` after hunk necessity: a surviving hunk turns `PASS` into
/// `UNPROVEN`. Never raises a verdict and never touches one that is not
/// `PASS`.
pub fn ablated(verdict: Verdict, outcome: &AblationOutcome) -> Verdict {
    match (verdict, outcome) {
        (Verdict::Pass, AblationOutcome::SurvivedHunk(hunks)) if !hunks.is_empty() => {
            Verdict::Unproven
        }
        (v, _) => v,
    }
}

/// The pushback for each surviving hunk, in the order given.
pub fn ablation_pushbacks(outcome: &AblationOutcome) -> Vec<crate::pushback::Pushback> {
    match outcome {
        AblationOutcome::SurvivedHunk(hunks) => hunks
            .iter()
            .map(|h| crate::pushback::Pushback::UntestedHunk {
                path: h.path.clone(),
                lines: h.lines,
            })
            .collect(),
        AblationOutcome::AllKilled | AblationOutcome::NotRun => Vec::new(),
    }
}

/// Why a criterion is unproven (§1.9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnprovenReason {
    /// No entry proves it.
    NotClaimed,
    /// No entry proves it, but a head test file carries its tag.
    ClaimedTestNotFound,
    /// No entry proves it, and no check that ran reports test names (each
    /// is judged by its exit code), so no tag could be found.
    NoTestNames,
    /// An entry that proves it passes on head and in the head-tests run, and
    /// on the transplant too.
    PassesWithoutChange,
    /// Its proving entries fail on head or in the head-tests run.
    FailsAtHead,
    /// The head changed a judging file, so nothing proves anything.
    JudgingFilesChanged,
    /// Every proving entry is `FLAKY` or `ENV`.
    Blocked,
    /// Its proving entries pass, but each failed on the original code only
    /// because something it uses is missing there (a weak proof, §1.9.4).
    /// Only the verifier knows this, from the runs' failure messages.
    WeakProof,
    /// Not a criterion's reason: a source hunk of a change that would be
    /// `PASS` that no proving test needs, since reverting it alone left
    /// every proving test passing (§1.9.4, hunk necessity). Only the
    /// verifier knows this, from the ablation runs.
    UntestedHunk,
}

impl UnprovenReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotClaimed => "not_claimed",
            Self::ClaimedTestNotFound => "claimed_test_not_found",
            Self::NoTestNames => "no_test_names",
            Self::PassesWithoutChange => "passes_without_change",
            Self::FailsAtHead => "fails_at_head",
            Self::JudgingFilesChanged => "judging_files_changed",
            Self::Blocked => "blocked",
            Self::WeakProof => "weak_proof",
            Self::UntestedHunk => "untested_hunk",
        }
    }
}

/// Why criterion `id`, which [`verdict`] left unproven, is unproven, first
/// match wins: a judging file changed (v2); no entry proves it; one passes
/// on head and in the head-tests run with no cause (so it passed on the
/// transplant too); all are `FLAKY` or `ENV`; otherwise they fail on head or
/// in the head-tests run. Never returns
/// [`UnprovenReason::ClaimedTestNotFound`], which needs the head's files,
/// or [`UnprovenReason::NoTestNames`], which needs the checks' reports.
pub fn unproven_reason(
    entries: &[ReceiptCheck],
    id: &CriterionId,
    rule: ProofRule,
) -> UnprovenReason {
    if rule == ProofRule::Transplant && judging_changed(entries) {
        return UnprovenReason::JudgingFilesChanged;
    }
    let provers: Vec<&ReceiptCheck> = provers(entries, id).collect();
    if provers.is_empty() {
        UnprovenReason::NotClaimed
    } else if provers.iter().any(|e| {
        e.head == HeadStatus::Pass
            && e.cause.is_none()
            && matches!(e.head_tests, None | Some(Transplant::Pass))
    }) {
        UnprovenReason::PassesWithoutChange
    } else if provers
        .iter()
        .all(|e| matches!(e.cause, Some(Cause::Flaky | Cause::Env)))
    {
        UnprovenReason::Blocked
    } else {
        UnprovenReason::FailsAtHead
    }
}

fn provers<'a>(
    entries: &'a [ReceiptCheck],
    id: &'a CriterionId,
) -> impl Iterator<Item = &'a ReceiptCheck> + 'a {
    entries
        .iter()
        .filter(move |e| e.proves.get(id.as_str()).is_some())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agreement {
    Pass,
    Fail,
    Flaky,
}

struct Side {
    agreement: Option<Agreement>,
    results: usize,
    ended_infra: bool,
}

impl Side {
    fn of(runs: &[Ending]) -> Self {
        let passes = runs.iter().filter(|o| **o == Ending::Pass).count();
        let fails = runs.iter().filter(|o| **o == Ending::Fail).count();
        let agreement = match (passes > 0, fails > 0) {
            (true, true) => Some(Agreement::Flaky),
            (true, false) => Some(Agreement::Pass),
            (false, true) => Some(Agreement::Fail),
            (false, false) => None,
        };
        Self {
            agreement,
            results: passes + fails,
            ended_infra: runs.last().is_some_and(|r| r.is_infra()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pushback::{LineRange, Pushback};

    const VERDICTS: [Verdict; 5] = [
        Verdict::Pass,
        Verdict::FailOwn,
        Verdict::BlockedNotOwn,
        Verdict::Unproven,
        Verdict::Review,
    ];

    fn hunk(path: &str, start: u32, end: u32) -> HunkAt {
        HunkAt {
            path: path.parse().unwrap(),
            lines: LineRange { start, end },
        }
    }

    fn outcomes() -> Vec<AblationOutcome> {
        let mut out = vec![AblationOutcome::AllKilled, AblationOutcome::NotRun];
        let pool = [
            hunk("src/a.rs", 3, 5),
            hunk("lib/b.py", 1, 1),
            hunk("c.go", 9, 40),
        ];
        for mask in 1u8..8 {
            out.push(AblationOutcome::SurvivedHunk(
                pool.iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, h)| h.clone())
                    .collect(),
            ));
        }
        out
    }

    /// Exhaustive over every verdict and every outcome over a small pool
    /// of hunks (the domain is finite, so every case is checked rather
    /// than sampled): a surviving hunk never leaves `PASS`, it lowers only
    /// `PASS` and only to `UNPROVEN`, and nothing else moves any verdict.
    #[test]
    fn a_surviving_hunk_never_passes_and_nothing_else_moves_a_verdict() {
        for verdict in VERDICTS {
            for outcome in outcomes() {
                let after = ablated(verdict, &outcome);
                match (&outcome, verdict) {
                    (AblationOutcome::SurvivedHunk(_), Verdict::Pass) => {
                        assert_eq!(after, Verdict::Unproven, "{outcome:?}");
                    }
                    _ => assert_eq!(after, verdict, "{outcome:?} {verdict:?}"),
                }
                assert!(
                    !matches!(outcome, AblationOutcome::SurvivedHunk(_)) || after != Verdict::Pass,
                    "{outcome:?} {verdict:?}"
                );
                let pushbacks = ablation_pushbacks(&outcome);
                match &outcome {
                    AblationOutcome::SurvivedHunk(h) => assert_eq!(pushbacks.len(), h.len()),
                    _ => assert!(pushbacks.is_empty()),
                }
            }
        }
    }

    #[test]
    fn the_pushback_names_the_hunk_in_one_sentence() {
        let line = Pushback::UntestedHunk {
            path: "src/calc.js".parse().unwrap(),
            lines: LineRange { start: 7, end: 9 },
        }
        .sentence()
        .to_line();
        assert_eq!(
            line,
            "Reverting your change at `src/calc.js:7-9` did not cause any proving test to fail; add an assertion that requires it or remove the unneeded code."
        );
    }
}
