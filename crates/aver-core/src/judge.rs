//! The cause of one receipt entry as one decision over what each side
//! showed (ARCHITECTURE.md §1.9.3). Every input is a small
//! enum and every `match` names each variant, so a new variant does not
//! compile until it is judged.
//!
//! Precedence: `ENV` when Aver lost the base's or the judged head's result
//! or the base gave no evidence (a judged head whose every run hit a
//! resource cap over a base that finished is the head's: `NEW`); then
//! `NEW`; then `CHANGED`; then `PREEXISTING` and `FLAKY`; then no cause. A
//! lost head-tests or transplant result is read as unknown: it can only
//! excuse, so it never turns `NEW` into `ENV`, and turns anything else into
//! `ENV`, never into no cause.

use crate::receipt::{BaseStatus, Cause, HeadStatus};
use crate::rules::Classification;
use crate::run::Ending;

/// Where the entry's test is defined, and what the head did to that file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    UntouchedSource,
    ModifiedSource,
    /// A Rust source file the head deleted whose base version held test
    /// items or doc-tests.
    DeletedSourceWithInlineTests,
    UntouchedTestFile,
    ModifiedTestFile,
    DeletedTestFile,
    AddedTestFile,
    /// A compiled test file that differs between the base and the judged
    /// head although both built it from the same frozen sources
    /// (`built::<path>`).
    BuiltArtifactModified,
}

/// The base side after its reruns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Base {
    Pass,
    Fail,
    Flaky,
    CompileError,
    Infra,
    /// The check's base runs ran no test at all.
    ZeroTests,
    /// The test does not exist on the base.
    Absent,
}

/// The judged head: the head's code with the base's frozen tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Judged {
    Pass,
    Fail,
    Flaky,
    CompileError,
    /// Every head run hit a resource cap or timed out.
    Exhausted,
    /// Aver lost the result: a sandbox failure, output or a report Aver
    /// cut, an incomplete probe, a target Aver did not run again.
    Infra,
    /// The runs completed and their report lacks the test (a crashed test
    /// binary).
    Missing,
    /// No base copy of the test to judge: an added test, or a test that
    /// went with its deleted source file.
    NotApplicable,
}

/// The head-tests run: the head's own version of the test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Own {
    Pass,
    Fail,
    Flaky,
    Skipped,
    Absent,
    CompileError,
    /// Hit its caps or timed out: the head's loss, read as a failure.
    Exhausted,
    Infra,
    NotRun,
}

/// The head's test on the base's code, for a proving entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transplant {
    FailOnAssertion,
    /// Failed only because something it uses is missing on the base: a
    /// weak proof.
    FailOnCompileOrImport,
    Pass,
    Infra,
    NotRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entry {
    pub origin: Origin,
    pub base: Base,
    pub judged: Judged,
    pub own: Own,
    pub transplant: Transplant,
}

/// Whether runs passed, and whether they failed, ignoring infra endings.
fn results(runs: &[Ending]) -> (bool, bool) {
    let any = |want: Ending| runs.contains(&want);
    (any(Ending::Pass), any(Ending::Fail))
}

impl Base {
    /// The base side from its runs, or one test's results, after reruns
    /// ([`crate::rules::needs_rerun`]): none is a test the base lacks, and a
    /// side that ended in infra lost its result.
    pub fn of(runs: &[Ending]) -> Self {
        let ended_infra = runs.last().is_some_and(|r| r.is_infra());
        match (runs.is_empty(), ended_infra, results(runs)) {
            (true, _, _) => Self::Absent,
            (false, true, _) | (false, false, (false, false)) => Self::Infra,
            (false, false, (true, true)) => Self::Flaky,
            (false, false, (true, false)) => Self::Pass,
            (false, false, (false, true)) => Self::Fail,
        }
    }
}

impl Judged {
    /// The judged head from its runs, or one test's results, after reruns:
    /// none is a test missing from a report the runs completed, every run at
    /// a resource cap is the head's exhaustion, and any other side that
    /// ended in infra lost its result.
    pub fn of(runs: &[Ending]) -> Self {
        let ended_infra = runs.last().is_some_and(|r| r.is_infra());
        let exhausted = runs.iter().all(|r| *r == Ending::Cap);
        match (runs.is_empty(), exhausted, ended_infra, results(runs)) {
            (true, _, _, _) => Self::Missing,
            (false, true, _, _) => Self::Exhausted,
            (false, false, true, _) | (false, false, false, (false, false)) => Self::Infra,
            (false, false, false, (true, true)) => Self::Flaky,
            (false, false, false, (true, false)) => Self::Pass,
            (false, false, false, (false, true)) => Self::Fail,
        }
    }
}

/// Judges one entry. Its `base` and `head` are the receipt's statuses.
pub fn judge(e: Entry) -> Classification {
    let cause = match evidence(e) {
        Evidence::Lost => Some(Cause::Env),
        Evidence::Found(found) => {
            let cause = decide(e.origin.kind(), found, e.own.version());
            let own_lost = match e.own {
                Own::Infra => true,
                Own::Pass
                | Own::Fail
                | Own::Flaky
                | Own::Skipped
                | Own::Absent
                | Own::CompileError
                | Own::Exhausted
                | Own::NotRun => false,
            };
            let transplant_lost = match e.transplant {
                Transplant::Infra => true,
                Transplant::FailOnAssertion
                | Transplant::FailOnCompileOrImport
                | Transplant::Pass
                | Transplant::NotRun => false,
            };
            match cause {
                Some(Cause::New) => Some(Cause::New),
                Some(Cause::Preexisting | Cause::Flaky | Cause::Changed | Cause::Env)
                    if own_lost =>
                {
                    Some(Cause::Env)
                }
                Some(c @ (Cause::Preexisting | Cause::Flaky | Cause::Changed | Cause::Env)) => {
                    Some(c)
                }
                None if own_lost || transplant_lost => Some(Cause::Env),
                None => None,
            }
        }
    };
    Classification {
        base: e.base.status(),
        head: head_status(e),
        cause,
    }
}

/// Whether an entry with `cause` proves its criteria under the transplant
/// rule: no cause, the head's own version passes, and on the base's code
/// it failed on an assertion. A failure only because something it uses is
/// missing there is a weak proof and proves nothing.
pub fn proves(cause: Option<Cause>, own: Own, transplant: Transplant) -> bool {
    let own_passes = match own {
        Own::Pass => true,
        Own::Fail
        | Own::Flaky
        | Own::Skipped
        | Own::Absent
        | Own::CompileError
        | Own::Exhausted
        | Own::Infra
        | Own::NotRun => false,
    };
    let needs_change = match transplant {
        Transplant::FailOnAssertion => true,
        Transplant::FailOnCompileOrImport
        | Transplant::Pass
        | Transplant::Infra
        | Transplant::NotRun => false,
    };
    cause.is_none() && own_passes && needs_change
}

/// What the head did to the file that defines the test, as the cause
/// rules see it.
enum Kind {
    /// The head left the file alone, or added it: judged as it ran.
    Kept,
    /// The head modified it: its own version may have rewritten what the
    /// base's copy expects.
    Rewritten,
    /// The head deleted it.
    Removed,
    WentWithCode,
    Built,
}

impl Origin {
    fn kind(self) -> Kind {
        match self {
            Self::UntouchedSource | Self::UntouchedTestFile | Self::AddedTestFile => Kind::Kept,
            Self::ModifiedSource | Self::ModifiedTestFile => Kind::Rewritten,
            Self::DeletedTestFile => Kind::Removed,
            Self::DeletedSourceWithInlineTests => Kind::WentWithCode,
            Self::BuiltArtifactModified => Kind::Built,
        }
    }
}

/// The base result a head result is compared with.
enum Was {
    Pass,
    Fail,
    Flaky,
    Absent,
}

impl Base {
    fn status(self) -> BaseStatus {
        match self {
            Self::Pass => BaseStatus::Pass,
            Self::Fail | Self::CompileError | Self::Infra | Self::ZeroTests => BaseStatus::Fail,
            Self::Flaky => BaseStatus::Flaky,
            Self::Absent => BaseStatus::NotApplicable,
        }
    }
}

/// The head's result: the judged head's, or the head's own version's
/// where the judged head holds no base copy of the test.
enum Is {
    Pass,
    Fail,
    Flaky,
    Crashed,
    Unbuilt,
    /// No result on the head at all.
    Gone,
}

enum Evidence {
    Lost,
    Found(Found),
}

fn evidence(e: Entry) -> Evidence {
    let was = match e.base {
        Base::Pass => Was::Pass,
        Base::Fail | Base::CompileError => Was::Fail,
        Base::Flaky => Was::Flaky,
        Base::Absent => Was::Absent,
        Base::Infra | Base::ZeroTests => return Evidence::Lost,
    };
    let is = match e.judged {
        Judged::Pass => Is::Pass,
        Judged::Fail => Is::Fail,
        Judged::Flaky => Is::Flaky,
        Judged::CompileError => Is::Unbuilt,
        Judged::Missing => Is::Crashed,
        Judged::Exhausted => {
            return match was {
                Was::Pass | Was::Fail | Was::Flaky => Evidence::Found(Found::New),
                Was::Absent => Evidence::Lost,
            }
        }
        Judged::Infra => return Evidence::Lost,
        Judged::NotApplicable => match e.own {
            Own::Pass => Is::Pass,
            Own::Fail | Own::CompileError | Own::Exhausted => Is::Fail,
            Own::Flaky => Is::Flaky,
            Own::Skipped | Own::Absent => Is::Gone,
            Own::Infra | Own::NotRun => return Evidence::Lost,
        },
    };
    Evidence::Found(compare(was, is))
}

fn head_status(e: Entry) -> HeadStatus {
    match e.judged {
        Judged::Pass => HeadStatus::Pass,
        Judged::Flaky => HeadStatus::Flaky,
        Judged::Fail
        | Judged::CompileError
        | Judged::Exhausted
        | Judged::Infra
        | Judged::Missing => HeadStatus::Fail,
        Judged::NotApplicable => match e.own {
            Own::Pass => HeadStatus::Pass,
            Own::Flaky => HeadStatus::Flaky,
            Own::Fail
            | Own::Skipped
            | Own::Absent
            | Own::CompileError
            | Own::Exhausted
            | Own::Infra
            | Own::NotRun => HeadStatus::Fail,
        },
    }
}

/// The head's result against the base's, before what the head did to the
/// test's file counts.
enum Found {
    New,
    Preexisting,
    Flaky,
    Unbuilt,
    /// The base had the test and the head has no result for it.
    Gone,
    /// The head passes a test the base passed.
    HeldPassing,
    /// The head passes, or neither side has the test.
    Held,
}

fn compare(was: Was, is: Is) -> Found {
    match (is, was) {
        (Is::Pass, Was::Pass) => Found::HeldPassing,
        (Is::Pass, Was::Fail | Was::Absent) => Found::Held,
        (Is::Pass, Was::Flaky) => Found::Flaky,
        (Is::Fail, Was::Pass | Was::Absent) => Found::New,
        (Is::Fail, Was::Fail) => Found::Preexisting,
        (Is::Fail, Was::Flaky) => Found::Flaky,
        (Is::Flaky, Was::Pass | Was::Absent) => Found::New,
        (Is::Flaky, Was::Fail | Was::Flaky) => Found::Flaky,
        (Is::Crashed, Was::Pass | Was::Fail | Was::Flaky | Was::Absent) => Found::New,
        (Is::Unbuilt, Was::Pass | Was::Fail | Was::Flaky | Was::Absent) => Found::Unbuilt,
        (Is::Gone, Was::Pass | Was::Fail | Was::Flaky) => Found::Gone,
        (Is::Gone, Was::Absent) => Found::Held,
    }
}

/// The head's own version of the test, as the `CHANGED` rules read it.
enum Version {
    Passes,
    Skipped,
    Deleted,
    Fails,
    NoReport,
}

impl Own {
    fn version(self) -> Version {
        match self {
            Self::Pass => Version::Passes,
            Self::Skipped => Version::Skipped,
            Self::Absent => Version::Deleted,
            Self::Fail | Self::Flaky | Self::Exhausted => Version::Fails,
            Self::CompileError | Self::NotRun | Self::Infra => Version::NoReport,
        }
    }
}

fn decide(kind: Kind, found: Found, own: Version) -> Option<Cause> {
    let removed = match kind {
        Kind::WentWithCode => return Some(Cause::Changed),
        Kind::Built => {
            return match found {
                Found::New | Found::Unbuilt => Some(Cause::New),
                Found::Preexisting
                | Found::Flaky
                | Found::Gone
                | Found::HeldPassing
                | Found::Held => Some(Cause::Changed),
            }
        }
        Kind::Kept => return kept(found, own),
        Kind::Rewritten => false,
        Kind::Removed => true,
    };
    match found {
        Found::New => match own {
            Version::Passes => Some(Cause::Changed),
            Version::Skipped | Version::Deleted | Version::Fails | Version::NoReport => {
                Some(Cause::New)
            }
        },
        Found::Preexisting => match own {
            Version::Passes | Version::Skipped | Version::Deleted => Some(Cause::Changed),
            Version::Fails | Version::NoReport => Some(Cause::Preexisting),
        },
        Found::Unbuilt => match own {
            Version::Passes | Version::Skipped | Version::Deleted | Version::Fails => {
                Some(Cause::Changed)
            }
            Version::NoReport => Some(Cause::New),
        },
        Found::Flaky => Some(Cause::Flaky),
        Found::Gone => Some(Cause::Changed),
        Found::HeldPassing => match own {
            Version::Skipped | Version::Deleted => Some(Cause::Changed),
            Version::Passes | Version::Fails | Version::NoReport => {
                removed.then_some(Cause::Changed)
            }
        },
        Found::Held => removed.then_some(Cause::Changed),
    }
}

fn kept(found: Found, own: Version) -> Option<Cause> {
    match found {
        Found::New | Found::Unbuilt => Some(Cause::New),
        Found::Preexisting => Some(Cause::Preexisting),
        Found::Flaky => Some(Cause::Flaky),
        Found::Gone => Some(Cause::Changed),
        Found::HeldPassing => match own {
            Version::Skipped => Some(Cause::Changed),
            Version::Passes | Version::Deleted | Version::Fails | Version::NoReport => None,
        },
        Found::Held => None,
    }
}
