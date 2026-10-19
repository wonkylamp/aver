//! The verify gate and the ranking of parallel attempts (ARCHITECTURE.md
//! §2.10). Pure functions: `aver run`'s supervisor and Claude Code's Stop
//! hook decide with [`gate`] whether an agent is done, and the comparison
//! table and `aver accept` order attempts with [`rank`].

use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

use crate::{AttemptId, Verdict};

/// What happens to an agent that stopped (or tried to).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDecision {
    /// Its latest receipt is `PASS`, or `REVIEW` (only changed test
    /// expectations are left, which a human reviews at accept): done.
    Done,
    /// Not done and verifies remain: resume it with the report.
    Continue,
    /// It gave up (`aver giveup`): end it and keep the reason.
    GiveUp,
    /// Not done and the verify budget is spent: end it.
    BudgetExhausted,
}

/// Decides an agent's fate from its latest verdict (`None` without a
/// receipt), how many receipts its attempt holds (derived, never a stored
/// counter) and the budget. `PASS` and `REVIEW` are done: nothing an agent
/// can do decides whether a changed test expectation was right. A `PASS` or
/// `REVIEW` that judged the previous version of a
/// file the directory changed (a withheld file, §2.3.1) is not done; the
/// caller passes it as `Unproven`.
pub fn gate(
    latest: Option<Verdict>,
    verifies: u32,
    max_verifies: u32,
    gave_up: bool,
) -> GateDecision {
    if gave_up {
        GateDecision::GiveUp
    } else if matches!(latest, Some(Verdict::Pass | Verdict::Review)) {
        GateDecision::Done
    } else if verifies >= max_verifies {
        GateDecision::BudgetExhausted
    } else {
        GateDecision::Continue
    }
}

/// One attempt as the ranking sees it, from its latest receipt and diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub attempt: AttemptId,
    /// The latest receipt's verdict; `None` without a receipt.
    pub verdict: Option<Verdict>,
    pub criteria: u32,
    pub unproven: u32,
    /// Entries with cause `NEW`, `FLAKY` and `CHANGED`.
    pub new: u32,
    pub flaky: u32,
    #[serde(default)]
    pub changed: u32,
    pub changed_files: u32,
    /// Lines added plus lines removed against the base.
    pub changed_lines: u32,
    pub wall_ms: u64,
    pub gave_up: bool,
    /// The workspace equals its base.
    pub empty: bool,
    /// Distinct entries that prove the change: with criteria, entries that
    /// prove one (transplant `fail` or `absent`, head-tests `pass`, no
    /// cause); without, the tests that show the change.
    #[serde(default)]
    pub proofs: u32,
    /// Tests of other top attempts that come out `NEW` on this attempt's
    /// code (the cross-check, §2.10).
    #[serde(default)]
    pub cross_failed: u32,
}

/// Rank classes, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankClass {
    Pass,
    Unproven,
    Review,
    Blocked,
    FailOwn,
    NoReceipt,
    NoChange,
}

impl RankClass {
    pub fn of(c: &Candidate) -> Self {
        if c.empty {
            return Self::NoChange;
        }
        match c.verdict {
            Some(Verdict::Pass) => Self::Pass,
            Some(Verdict::Review) => Self::Review,
            Some(Verdict::Unproven) => Self::Unproven,
            Some(Verdict::BlockedNotOwn) => Self::Blocked,
            Some(Verdict::FailOwn) => Self::FailOwn,
            None => Self::NoReceipt,
        }
    }

    /// A class whose attempt a person might accept as it is.
    pub fn acceptable(self) -> bool {
        matches!(self, Self::Pass | Self::Unproven | Self::Review)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Review => "REVIEW",
            Self::Unproven => "UNPROVEN",
            Self::Blocked => "BLOCKED",
            Self::FailOwn => "FAIL_OWN",
            Self::NoReceipt => "no receipt",
            Self::NoChange => "no change",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ranked {
    pub attempt: AttemptId,
    /// 1 for the best attempt; every place is distinct.
    pub place: u32,
    pub class: RankClass,
    /// One line on why the attempt sits where it does.
    pub why: String,
    /// Set on place 1 when place 2 is equal on evidence: what decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tie: Option<Tie>,
}

/// Places 1 and 2 are equal on evidence (class, `CHANGED` entries, unproven
/// criteria, cross-check failures, proofs, giving up); the rest of the key
/// decided between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tie {
    /// The attempt at place 2.
    pub with: AttemptId,
    pub by: TieBreak,
}

/// The key that separated a [`Tie`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TieBreak {
    DiffSize,
    WallTime,
    AttemptId,
}

impl TieBreak {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DiffSize => "diff size",
            Self::WallTime => "wall time",
            Self::AttemptId => "attempt id",
        }
    }
}

/// Orders attempts: by class (`PASS` > `UNPROVEN` > `REVIEW` > `BLOCKED` >
/// `FAIL_OWN` > no receipt > no change, where an empty diff is always no
/// change), then fewer `CHANGED` entries, fewer unproven criteria, fewer of
/// the other top attempts' tests failing on its code (the cross-check), more
/// proofs, not given up, fewer changed lines, shorter wall time, and the
/// attempt id. Total and deterministic: the input order never matters.
/// `UNPROVEN` ranks above `REVIEW` so that an attempt that left the existing
/// tests and harness alone beats one that changed what they expect, however
/// small its diff: without tagged tests, a test-gaming attempt is often the
/// smallest. More proofs rank above a smaller diff because the fuller fix
/// is usually the larger one. `FLAKY` counts are draws of the base's
/// flakiness, so they stay out of the key and appear only in `why`.
pub fn rank(candidates: &[Candidate]) -> Vec<Ranked> {
    let mut sorted: Vec<&Candidate> = candidates.iter().collect();
    sorted.sort_by_key(|c| (evidence(c), c.changed_lines, c.wall_ms, c.attempt));
    let tie = match sorted.as_slice() {
        [a, b, ..] if evidence(a) == evidence(b) && RankClass::of(a).acceptable() => Some(Tie {
            with: b.attempt,
            by: if a.changed_lines != b.changed_lines {
                TieBreak::DiffSize
            } else if a.wall_ms != b.wall_ms {
                TieBreak::WallTime
            } else {
                TieBreak::AttemptId
            },
        }),
        _ => None,
    };
    sorted
        .into_iter()
        .enumerate()
        .map(|(i, c)| Ranked {
            attempt: c.attempt,
            place: i as u32 + 1,
            class: RankClass::of(c),
            why: why(c),
            tie: if i == 0 { tie.clone() } else { None },
        })
        .collect()
}

/// The part of the rank key that is evidence about the change: attempts
/// equal on it are tied, and diff size, time or the id only breaks the tie.
fn evidence(c: &Candidate) -> (RankClass, u32, u32, u32, Reverse<u32>, bool) {
    (
        RankClass::of(c),
        c.changed,
        c.unproven,
        c.cross_failed,
        Reverse(c.proofs),
        c.gave_up,
    )
}

fn why(c: &Candidate) -> String {
    let class = RankClass::of(c);
    let mut parts = vec![class.as_str().to_owned()];
    if c.verdict.is_some() && !c.empty {
        if c.criteria > 0 {
            parts.push(format!(
                "{}/{} criteria proven",
                c.criteria - c.unproven.min(c.criteria),
                c.criteria
            ));
        }
        if c.proofs > 0 {
            parts.push(format!(
                "{} {}",
                c.proofs,
                if c.proofs == 1 { "proof" } else { "proofs" }
            ));
        }
        parts.push(format!("{} new", c.new));
        if c.cross_failed > 0 {
            parts.push(format!("fails {} of the others' tests", c.cross_failed));
        }
        if c.changed > 0 {
            parts.push(format!("{} changed tests or harness", c.changed));
        }
        if c.flaky > 0 {
            parts.push(format!("{} flaky", c.flaky));
        }
    }
    if !c.empty {
        parts.push(format!(
            "{} {}, {} lines",
            c.changed_files,
            if c.changed_files == 1 {
                "file"
            } else {
                "files"
            },
            c.changed_lines
        ));
    }
    if c.gave_up {
        parts.push("gave up".into());
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERDICTS: [Option<Verdict>; 6] = [
        None,
        Some(Verdict::Pass),
        Some(Verdict::Review),
        Some(Verdict::Unproven),
        Some(Verdict::BlockedNotOwn),
        Some(Verdict::FailOwn),
    ];

    fn ids(n: usize) -> Vec<AttemptId> {
        let mut ids: Vec<AttemptId> = (0..n).map(|_| AttemptId::generate()).collect();
        ids.sort();
        ids
    }

    /// Every candidate over a small space, each with a distinct id.
    fn space() -> Vec<Candidate> {
        let mut out = Vec::new();
        for verdict in VERDICTS {
            for empty in [false, true] {
                for unproven in [0, 1] {
                    for flaky in [0, 1] {
                        for gave_up in [false, true] {
                            for changed_lines in [3, 40] {
                                for wall_ms in [1_000, 9_000] {
                                    out.push(Candidate {
                                        attempt: AttemptId::generate(),
                                        verdict,
                                        criteria: 2,
                                        unproven,
                                        new: 0,
                                        flaky,
                                        changed: u32::from(verdict == Some(Verdict::Review))
                                            + u32::from(wall_ms > 1_000 && unproven == 1),
                                        changed_files: u32::from(!empty),
                                        changed_lines: if empty { 0 } else { changed_lines },
                                        wall_ms,
                                        gave_up,
                                        empty,
                                        proofs: u32::from(changed_lines > 3)
                                            + u32::from(flaky == 1),
                                        cross_failed: u32::from(gave_up && wall_ms > 1_000),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    #[test]
    fn gate_is_exhaustive_over_small_inputs() {
        for latest in VERDICTS {
            for verifies in 0..4 {
                for max in 1..4 {
                    for gave_up in [false, true] {
                        let got = gate(latest, verifies, max, gave_up);
                        let want = if gave_up {
                            GateDecision::GiveUp
                        } else if matches!(latest, Some(Verdict::Pass | Verdict::Review)) {
                            GateDecision::Done
                        } else if verifies >= max {
                            GateDecision::BudgetExhausted
                        } else {
                            GateDecision::Continue
                        };
                        assert_eq!(got, want, "{latest:?} {verifies}/{max} {gave_up}");
                    }
                }
            }
        }
        assert_eq!(gate(Some(Verdict::Pass), 9, 1, false), GateDecision::Done);
    }

    #[test]
    fn pass_never_ranks_below_unproven_and_empty_is_no_change() {
        let all = space();
        // Every pair, ranked together.
        for a in &all {
            for b in &all {
                if a.attempt == b.attempt {
                    continue;
                }
                let ranked = rank(&[a.clone(), b.clone()]);
                let place = |c: &Candidate| {
                    ranked
                        .iter()
                        .find(|r| r.attempt == c.attempt)
                        .unwrap()
                        .place
                };
                let (ca, cb) = (RankClass::of(a), RankClass::of(b));
                if ca == RankClass::Pass && cb == RankClass::Unproven {
                    assert!(place(a) < place(b), "{a:?} vs {b:?}");
                }
                if ca == RankClass::Unproven && cb == RankClass::Review {
                    assert!(place(a) < place(b), "{a:?} vs {b:?}");
                }
                if ca == cb && a.changed < b.changed {
                    assert!(place(a) < place(b), "fewer CHANGED first: {a:?} vs {b:?}");
                }
                if ca < cb {
                    assert!(place(a) < place(b), "class order: {a:?} vs {b:?}");
                }
            }
        }
        for c in &all {
            if c.empty {
                assert_eq!(rank(std::slice::from_ref(c))[0].class, RankClass::NoChange);
            }
        }
    }

    #[test]
    fn rank_is_total_and_ignores_input_order() {
        let all = space();
        let forward = rank(&all);
        let mut reversed: Vec<Candidate> = all.iter().rev().cloned().collect();
        reversed.rotate_left(17);
        assert_eq!(forward, rank(&reversed));
        let places: Vec<u32> = forward.iter().map(|r| r.place).collect();
        assert_eq!(places, (1..=all.len() as u32).collect::<Vec<_>>());
    }

    #[test]
    fn within_a_class_smaller_and_faster_wins_then_the_id() {
        let [a, b, c] = ids(3).try_into().unwrap();
        let base = Candidate {
            attempt: a,
            verdict: Some(Verdict::Pass),
            criteria: 1,
            unproven: 0,
            new: 0,
            flaky: 0,
            changed: 0,
            changed_files: 1,
            changed_lines: 10,
            wall_ms: 5_000,
            gave_up: false,
            empty: false,
            proofs: 0,
            cross_failed: 0,
        };
        let smaller = Candidate {
            attempt: b,
            changed_lines: 4,
            ..base.clone()
        };
        let tie = Candidate {
            attempt: c,
            ..base.clone()
        };
        let ranked = rank(&[tie.clone(), base.clone(), smaller.clone()]);
        let order: Vec<AttemptId> = ranked.iter().map(|r| r.attempt).collect();
        assert_eq!(order, vec![b, a, c]);
        assert_eq!(
            ranked[0].why,
            "PASS, 1/1 criteria proven, 0 new, 1 file, 4 lines"
        );
        assert_eq!(
            ranked[0].tie,
            Some(Tie {
                with: a,
                by: TieBreak::DiffSize
            })
        );
        assert!(ranked[1..].iter().all(|r| r.tie.is_none()));
    }

    /// The hard-set noisy-ledger run (bench-data/eval/sonnet-hard-analysis.md
    /// §1): both attempts `PASS 1/1`, 0 NEW, 4 FLAKY. Diff size alone picked
    /// #2 (34 lines, 1 proving test), which failed a hidden test; #1 (57
    /// lines, 2 proving tests) passed all nine.
    #[test]
    fn noisy_ledger_flips_on_proofs_and_flaky_counts_do_not_matter() {
        let [one, two] = ids(2).try_into().unwrap();
        let attempt = |attempt, proofs, flaky, changed_lines| Candidate {
            attempt,
            verdict: Some(Verdict::Pass),
            criteria: 1,
            unproven: 0,
            new: 0,
            flaky,
            changed: 0,
            changed_files: 2,
            changed_lines,
            wall_ms: 300_000,
            gave_up: false,
            empty: false,
            proofs,
            cross_failed: 0,
        };
        for (flaky1, flaky2) in [(4, 4), (9, 0), (0, 9)] {
            let ranked = rank(&[attempt(two, 1, flaky2, 34), attempt(one, 2, flaky1, 57)]);
            assert_eq!(ranked[0].attempt, one, "flaky {flaky1}/{flaky2}");
            assert_eq!(ranked[0].tie, None);
        }
        let ranked = rank(&[attempt(two, 1, 4, 34), attempt(one, 2, 4, 57)]);
        assert_eq!(
            ranked[0].why,
            "PASS, 1/1 criteria proven, 2 proofs, 0 new, 4 flaky, 2 files, 57 lines"
        );
        // Equal proofs: a tie on evidence, which diff size decides and says.
        let ranked = rank(&[attempt(two, 2, 4, 34), attempt(one, 2, 0, 57)]);
        assert_eq!(ranked[0].attempt, two);
        assert_eq!(
            ranked[0].tie,
            Some(Tie {
                with: one,
                by: TieBreak::DiffSize
            })
        );
    }

    #[test]
    fn failing_the_others_tests_outranks_proofs_and_size() {
        let [one, two] = ids(2).try_into().unwrap();
        let base = Candidate {
            attempt: one,
            verdict: Some(Verdict::Pass),
            criteria: 1,
            unproven: 0,
            new: 0,
            flaky: 0,
            changed: 0,
            changed_files: 1,
            changed_lines: 50,
            wall_ms: 1_000,
            gave_up: false,
            empty: false,
            proofs: 1,
            cross_failed: 0,
        };
        let smaller_but_fails = Candidate {
            attempt: two,
            changed_lines: 5,
            proofs: 3,
            cross_failed: 2,
            ..base.clone()
        };
        let ranked = rank(&[smaller_but_fails, base]);
        assert_eq!(ranked[0].attempt, one);
        assert_eq!(ranked[0].tie, None);
        assert!(
            ranked[1].why.contains("fails 2 of the others' tests"),
            "{}",
            ranked[1].why
        );
    }

    #[test]
    fn no_tie_is_flagged_below_the_acceptable_classes() {
        let [one, two] = ids(2).try_into().unwrap();
        let fail = |attempt| Candidate {
            attempt,
            verdict: Some(Verdict::FailOwn),
            criteria: 1,
            unproven: 1,
            new: 1,
            flaky: 0,
            changed: 0,
            changed_files: 1,
            changed_lines: 5,
            wall_ms: 1_000,
            gave_up: false,
            empty: false,
            proofs: 0,
            cross_failed: 0,
        };
        assert_eq!(rank(&[fail(one), fail(two)])[0].tie, None);
    }
}
