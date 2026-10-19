//! Evidence (ARCHITECTURE.md §1.9.7): how many tests each side ran, and the
//! blockers that keep a verdict from counting at all. Zero evidence is never
//! PASS: a check that ran no tests, fewer than its floor, or only failing
//! ones on the base blocks the verdict.

use serde::{Deserialize, Serialize};

use crate::contract::{Approval, ContractCheck, Reading, ReportSpec, TextFormat};
use crate::run::{TestResult, TestStatus};
use crate::scalar::{CheckId, Path, Text};
use crate::set::Set;

/// Most bytes of a test target's name a blocker keeps.
pub const MAX_TARGET_BYTES: usize = 256;

/// The tests one side of one check reported. A skipped test did not run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SideCount {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}

impl SideCount {
    pub fn of<'a>(tests: impl IntoIterator<Item = &'a TestResult>) -> Self {
        tests.into_iter().fold(Self::default(), |mut count, test| {
            let n = match test.status {
                TestStatus::Pass => &mut count.passed,
                TestStatus::Fail => &mut count.failed,
                TestStatus::Skip => &mut count.skipped,
            };
            *n = n.saturating_add(1);
            count
        })
    }

    /// The tests that ran: passed or failed.
    pub fn ran(&self) -> u32 {
        self.passed.saturating_add(self.failed)
    }
}

/// The trees a check runs on in one verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The original code and its tests.
    Base,
    /// The change with the base's tests.
    JudgedHead,
    /// The change with its own tests.
    HeadTests,
    /// The change's tests on the original code.
    Transplant,
}

/// A setup command that failed before a side's tests.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "step", content = "check", rename_all = "lowercase")]
pub enum SetupStep {
    /// `deps.install`.
    Install,
    /// The `prepare` command of a check.
    Prepare(CheckId),
}

/// What a side's run says beyond its counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SideFault {
    /// The runner stopped on its own error before or while reporting tests
    /// (see [`pytest_runner_error`]).
    RunnerError { exit_code: i32 },
    /// A test target ended early, a panic or abort cutting off tests it
    /// declares (see [`truncated`]).
    Truncated {
        target: Text<MAX_TARGET_BYTES>,
        missing: u32,
    },
    /// The check's `prepare` command failed.
    PrepareFailed,
}

/// One side of one check: its counts and faults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SideEvidence {
    pub count: SideCount,
    pub faults: Vec<SideFault>,
}

/// One affected check's base and judged head.
#[derive(Debug, Clone, Copy)]
pub struct CheckEvidence<'a> {
    pub check: &'a ContractCheck,
    pub base: &'a SideEvidence,
    /// The judged head: the base's tests on the change.
    pub head: &'a SideEvidence,
}

/// Why a verdict cannot count. Any blocker makes it BLOCKED (exit 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Blocker {
    /// No test ran on a side of a counted check.
    NoTests { check: CheckId, side: Side },
    /// An exit-only check whose contract the base did not approve.
    UnapprovedExit { check: CheckId },
    /// The base ran fewer tests than the contract's floor.
    BelowFloor {
        check: CheckId,
        ran: u32,
        floor: u32,
    },
    /// Every test that ran on the base failed.
    AllRed { check: CheckId, failed: u32 },
    /// The runner stopped on its own error.
    RunnerError {
        check: CheckId,
        side: Side,
        exit_code: i32,
    },
    /// A test target ended early with declared tests unreported.
    Truncated {
        check: CheckId,
        side: Side,
        target: Text<MAX_TARGET_BYTES>,
        missing: u32,
    },
    /// An install or prepare command failed.
    SetupFailed { step: SetupStep, side: Side },
    /// A kept directory holds a non-editable copy of a changed source file,
    /// so the tests would run the copy, not the change.
    StaleCopy { keep: Path, tracked: Path },
    /// The CI checkout's tracked files differ from the commit under test.
    DirtyCheckout { paths: Set<Path> },
}

/// How a check's result counts as evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counting {
    /// Test by test, with a floor on how many tests the base must run.
    Tests { min_tests: u32 },
    /// The exit code alone, which counts only when `approved`.
    Exit { approved: bool },
    /// No counts: a report that lists failures only (a type check's
    /// errors), so a clean run lists nothing, or one a side did not write.
    /// Only the sides' faults block.
    Uncounted,
}

impl Counting {
    /// How `check` counts under `approval`. Approval gates exit-only checks
    /// alone: a report's tests are evidence whoever wrote the command.
    pub fn of(check: &ContractCheck, approval: &Approval) -> Self {
        match &check.reading {
            Reading::Tests {
                report:
                    ReportSpec::Stdout(TextFormat::Tsc)
                    | ReportSpec::File {
                        format: TextFormat::Tsc,
                        ..
                    },
                ..
            } => Self::Uncounted,
            Reading::Tests { min_tests, .. } => Self::Tests {
                min_tests: min_tests.get(),
            },
            Reading::Exit => Self::Exit {
                approved: approval.approves_exit(),
            },
        }
    }
}

/// The blockers of the affected checks, check by check in the order given
/// ([`decide_check`]). Place blockers (an install, a stale copy, a dirty
/// checkout) are facts the place reports, not decided here.
pub fn decide(approval: &Approval, checks: &[CheckEvidence<'_>]) -> Vec<Blocker> {
    checks
        .iter()
        .flat_map(|e| decide_check(&e.check.id, Counting::of(e.check, approval), e.base, e.head))
        .collect()
}

/// The blockers of one check: each side's faults, then no tests on either
/// side, then the base below its floor, then a base with no passing test.
/// An exit-only check has no counts and blocks only when not approved; an
/// uncounted one blocks only on its faults.
pub fn decide_check(
    id: &CheckId,
    counting: Counting,
    base: &SideEvidence,
    head: &SideEvidence,
) -> Vec<Blocker> {
    let mut out = Vec::new();
    for (side, ev) in [(Side::Base, base), (Side::JudgedHead, head)] {
        out.extend(ev.faults.iter().map(|fault| match fault {
            SideFault::PrepareFailed => Blocker::SetupFailed {
                step: SetupStep::Prepare(id.clone()),
                side,
            },
            SideFault::RunnerError { exit_code } => Blocker::RunnerError {
                check: id.clone(),
                side,
                exit_code: *exit_code,
            },
            SideFault::Truncated { target, missing } => Blocker::Truncated {
                check: id.clone(),
                side,
                target: target.clone(),
                missing: *missing,
            },
        }));
    }
    let min_tests = match counting {
        Counting::Exit { approved } => {
            if !approved {
                out.push(Blocker::UnapprovedExit { check: id.clone() });
            }
            return out;
        }
        Counting::Uncounted => return out,
        Counting::Tests { min_tests } => min_tests,
    };
    let (base, head) = (base.count, head.count);
    for (side, count) in [(Side::Base, base), (Side::JudgedHead, head)] {
        if count.ran() == 0 {
            out.push(Blocker::NoTests {
                check: id.clone(),
                side,
            });
        }
    }
    if base.ran() > 0 && base.ran() < min_tests {
        out.push(Blocker::BelowFloor {
            check: id.clone(),
            ran: base.ran(),
            floor: min_tests,
        });
    }
    if base.ran() > 0 && base.passed == 0 {
        out.push(Blocker::AllRed {
            check: id.clone(),
            failed: base.failed,
        });
    }
    out
}

/// Whether a pytest exit code is the runner's own error: interrupted (2),
/// internal error (3) or usage or configuration error (4). Exit 5 (no tests
/// collected) is zero evidence, counted as such.
pub fn pytest_runner_error(exit_code: i32) -> bool {
    matches!(exit_code, 2..=4)
}

/// How many declared tests a target that ended early never reported (a Go
/// package whose stream ended in a panic or `FAIL <pkg>`): `None` when it
/// ended normally or every declared test reported.
pub fn truncated<'a>(
    ended_early: bool,
    declared: impl IntoIterator<Item = &'a str>,
    reported: &dyn Fn(&str) -> bool,
) -> Option<u32> {
    if !ended_early {
        return None;
    }
    let missing = declared.into_iter().filter(|name| !reported(name)).count();
    (missing > 0).then(|| u32::try_from(missing).unwrap_or(u32::MAX))
}
