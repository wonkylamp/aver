//! Pushbacks (ARCHITECTURE.md §1.9.7): at most one precise sentence at the top of
//! `aver check` and `aver verify`, telling the agent the one thing to do.
//!
//! Repository text (test names, paths, commands) is data (CLAUDE.md rule 7):
//! a [`Sentence`] keeps it apart from Aver's own words, and
//! [`Sentence::to_line`] writes it only as escaped inline code.

use std::num::NonZeroU32;

use crate::evidence::{Blocker, SetupStep, Side};
use crate::rules::Cap;
use crate::scalar::{Path, ShellLine};

/// Most characters of one piece of repository text a line shows.
pub const MAX_DATA_CHARS: usize = 160;
/// Most candidate commands a [`Pushback::NoContract`] names.
pub const MAX_CANDIDATES: usize = 3;

/// How a test the change removed went: what the head's tree and its runs
/// show about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Removal {
    /// The file that defines it is gone.
    Deleted,
    /// The runner reports it skipped.
    Skipped,
    /// No run reports it, and its file is still there or not known: taken
    /// out of the file, renamed or filtered out, which the reports do not
    /// tell apart.
    NotRun,
}

/// How the base's copy of a removed test fares on the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemovedOutcome {
    /// It fails on the change: the removal hides a regression (FAIL_OWN).
    FailsOnChange,
    /// It still passes: the removal needs a reviewer (REVIEW).
    PassesOnChange,
}

/// What another task's change does to this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverlapKind {
    /// The two changes do not merge.
    Conflicts,
    /// They merge, and the merge fails this task's proving tests.
    BreaksYourTests,
}

/// Lines of a file, 1-based and inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pushback {
    /// Tests that pass on the original code fail on the change.
    Breaks { first: String, count: NonZeroU32 },
    /// The proving test never exercises the code the change modified.
    WeakProof { test: String, function: String },
    /// The proving test fails on the original code only because a name the
    /// change adds is missing there, which any test naming it does.
    MissingName { test: String, name: String },
    /// A test the change added passes and fails across runs of one tree.
    FlakyAdded {
        test: String,
        failed: u32,
        runs: u32,
    },
    Skipped {
        test: String,
        removal: Removal,
        outcome: RemovedOutcome,
    },
    /// A blocker: the tests gave no evidence to judge.
    ZeroEvidence(Blocker),
    /// What the runs cannot show held the verdict below `PASS`.
    Capped(Cap),
    /// The base's tests fail because the environment lacks a variable.
    MissingEnv { var: String, failed: u32, ran: u32 },
    Overlap {
        path: Path,
        lines: Option<LineRange>,
        kind: OverlapKind,
    },
    /// The repository has no contract; `candidates` are commands its CI,
    /// Makefile or package scripts run.
    NoContract { candidates: Vec<ShellLine> },
    /// Reverting this hunk of the change left every proving test passing
    /// (§1.9.4, hunk necessity).
    UntestedHunk { path: Path, lines: LineRange },
}

impl Pushback {
    /// Lower is shown first: no contract, then what makes the evidence void,
    /// then a broken or gamed change, then a weak proof, then another task.
    fn rank(&self) -> u8 {
        match self {
            Self::NoContract { .. } => 0,
            Self::MissingEnv { .. } => 1,
            Self::ZeroEvidence(_) | Self::Capped(_) => 2,
            Self::Breaks { .. } => 3,
            Self::Skipped { .. } => 4,
            Self::FlakyAdded { .. } => 5,
            Self::WeakProof { .. } | Self::MissingName { .. } | Self::UntestedHunk { .. } => 6,
            Self::Overlap { .. } => 7,
        }
    }

    /// The one pushback to show: the first of the highest priority.
    pub fn pick(candidates: impl IntoIterator<Item = Pushback>) -> Option<Pushback> {
        candidates.into_iter().min_by_key(Pushback::rank)
    }

    pub fn sentence(&self) -> Sentence {
        let s = Sentence::default();
        match self {
            Self::Breaks { first, count } if count.get() == 1 => s
                .words("your change breaks ")
                .data(first)
                .words(", which passes on the original code; `aver check` shows the failure."),
            Self::Breaks { first, count } => s
                .words(&format!(
                    "your change breaks {count} tests that pass on the original code: "
                ))
                .data(first)
                .words(&format!(
                    " (+{}); `aver check` shows them.",
                    count.get() - 1
                )),
            Self::WeakProof { test, function } => s
                .words("your test ")
                .data(test)
                .words(" never calls ")
                .data(function)
                .words(", the code you changed; make it call and assert on ")
                .data(function)
                .words("."),
            Self::MissingName { test, name } => s
                .words("your test ")
                .data(test)
                .words(" fails on the original code only because ")
                .data(name)
                .words(", which your change adds, is missing there; make it fail there on a result the original code gets wrong."),
            Self::FlakyAdded { test, failed, runs } => s.words("your new test ").data(test).words(
                &format!(" fails {failed} runs in {runs} on your change; make it deterministic."),
            ),
            Self::Skipped {
                test,
                removal,
                outcome,
            } => {
                let (did, undo, noun) = match removal {
                    Removal::Deleted => ("deleted", "restore it", "deletion"),
                    Removal::Skipped => ("skipped", "un-skip it", "skip"),
                    Removal::NotRun => ("deleted or no longer run", "restore it", "removal"),
                };
                let s = s.words(&format!("you {did} ")).data(test);
                match outcome {
                    RemovedOutcome::FailsOnChange => s.words(&format!(
                        ", which fails on your change; {undo} and fix the code it tests."
                    )),
                    RemovedOutcome::PassesOnChange => s.words(&format!(
                        ", which passes on the original code; {undo}, or a reviewer must accept the {noun}."
                    )),
                }
            }
            Self::ZeroEvidence(blocker) => blocker.sentence(),
            Self::Capped(Cap::NoRuntimeTests) => s.words(
                "no check ran a test on the original code (a type check or an exit code is not a test), so nothing shows the change works; add a check that runs tests to aver.toml.",
            ),
            Self::Capped(Cap::ProposedContract) => s.words(
                "the checks ran the aver.toml this change adds, which a person must approve; merge aver.toml on its own first, or have a reviewer accept the change.",
            ),
            Self::MissingEnv { var, failed, ran } => s
                .words("tests fail on the original code too because ")
                .data(var)
                .words(&format!(
                    " is not set ({failed} of {ran}); set it in your environment."
                )),
            Self::Overlap { path, lines, kind } => {
                let s = s.words("another task changed ").data(path.as_str());
                let s = match lines {
                    Some(r) => s.words(&format!(" lines {}-{}", r.start, r.end)),
                    None => s,
                };
                match kind {
                    OverlapKind::Conflicts => s.words(", which conflicts with yours"),
                    OverlapKind::BreaksYourTests => {
                        s.words(", which breaks your tests when merged with your change")
                    }
                }
                .words("; `aver notices` shows it.")
            }
            Self::UntestedHunk { path, lines } => s
                .words("Reverting your change at ")
                .data(&format!("{}:{}-{}", path, lines.start, lines.end))
                .words(" did not cause any proving test to fail; add an assertion that requires it or remove the unneeded code."),
            Self::NoContract { candidates } => {
                let s = s.words(
                    "Aver does not know this repository's test command yet; run once: aver check -- '<the test command you use>'",
                );
                let shown = &candidates[..candidates.len().min(MAX_CANDIDATES)];
                let mut s = s;
                for (i, line) in shown.iter().enumerate() {
                    s = s.words(if i == 0 { " (it runs " } else { ", " });
                    s = s.data(line.as_str());
                }
                if !shown.is_empty() {
                    s = s.words(")");
                }
                s.words(".")
            }
        }
    }
}

fn side_words(side: Side) -> &'static str {
    match side {
        Side::Base => "the original code",
        Side::JudgedHead => "your change",
        Side::HeadTests => "your change with your tests",
        Side::Transplant => "your tests on the original code",
    }
}

fn tests(n: u32) -> String {
    if n == 1 {
        "1 test".to_owned()
    } else {
        format!("{n} tests")
    }
}

impl Blocker {
    /// The first line of a blocked report: what stopped the verdict and the
    /// fix.
    pub fn sentence(&self) -> Sentence {
        let s = Sentence::default();
        match self {
            Self::NoTests { check, side } => s
                .words(&format!("0 tests ran on {} in check ", side_words(*side)))
                .data(check.as_str())
                .words("; the command must run tests before Aver can judge them."),
            Self::UnapprovedExit { check } => s
                .words("check ")
                .data(check.as_str())
                .words(" reports only its exit code, which counts only once a merged aver.toml approves it; give it a test report."),
            Self::BelowFloor { check, ran, floor } => s
                .words(&format!(
                    "only {} ran on the original code in check ",
                    tests(*ran)
                ))
                .data(check.as_str())
                .words(&format!(
                    ", below the contract's minimum of {floor}; make the missing tests run."
                )),
            Self::AllRed { check, failed } => s
                .words(&match failed {
                    1 => "the only test that ran on the original code failed in check ".to_owned(),
                    n => format!("all {n} tests that ran on the original code failed in check "),
                })
                .data(check.as_str())
                .words("; the original code must pass before Aver can judge a change."),
            Self::RunnerError {
                check,
                side,
                exit_code,
            } => s
                .words("the test runner of check ")
                .data(check.as_str())
                .words(&format!(
                    " stopped with its own error (exit {exit_code}) on {}; fix the command or its configuration.",
                    side_words(*side)
                )),
            Self::Truncated {
                check,
                side,
                target,
                missing,
            } => s
                .data(target.as_str())
                .words(&format!(" stopped early on {} in check ", side_words(*side)))
                .data(check.as_str())
                .words(&format!(
                    ", with {} it declares never reported; a panic or abort cut it off.",
                    tests(*missing)
                )),
            Self::SetupFailed { step, side } => match step {
                SetupStep::Install => s.words("the install command"),
                SetupStep::Prepare(check) => s
                    .words("the prepare command of check ")
                    .data(check.as_str()),
            }
            .words(&format!(
                " failed on {}; fix it before the tests can run.",
                side_words(*side)
            )),
            Self::StaleCopy { keep, tracked } => s
                .data(keep.as_str())
                .words(" holds a stale copy of ")
                .data(tracked.as_str())
                .words(", so the tests would run the copy, not your change; install the package in editable mode."),
            Self::DirtyCheckout { paths } => {
                let s = s.words("the checkout's tracked files differ from the commit");
                let s = match paths.iter().next() {
                    Some(first) => {
                        let s = s.words(": ").data(first.as_str());
                        if paths.len() > 1 {
                            s.words(&format!(" (+{})", paths.len() - 1))
                        } else {
                            s
                        }
                    }
                    None => s,
                };
                s.words("; setup steps must not rewrite tracked files.")
            }
        }
    }
}

/// One piece of a [`Sentence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Aver's own words and numbers.
    Words(String),
    /// Text that came from the repository.
    Data(String),
}

/// A sentence that keeps repository text apart from Aver's words, so each
/// renderer fences or escapes it its own way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sentence(Vec<Piece>);

impl Sentence {
    fn words(mut self, words: &str) -> Self {
        self.0.push(Piece::Words(words.to_owned()));
        self
    }

    fn data(mut self, data: &str) -> Self {
        self.0.push(Piece::Data(data.to_owned()));
        self
    }

    pub fn pieces(&self) -> &[Piece] {
        &self.0
    }

    /// One line of plain text: each data piece as an inline code span, its
    /// control and bidirectional characters escaped, cut to
    /// [`MAX_DATA_CHARS`] characters.
    pub fn to_line(&self) -> String {
        self.0
            .iter()
            .map(|piece| match piece {
                Piece::Words(w) => w.clone(),
                Piece::Data(d) => code_span(d),
            })
            .collect()
    }
}

fn code_span(data: &str) -> String {
    let mut text = String::new();
    for (i, c) in data.chars().enumerate() {
        if i == MAX_DATA_CHARS {
            text.push('…');
            break;
        }
        match c {
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            c if c.is_control() || is_bidi(c) => {
                text.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => text.push(c),
        }
    }
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    if text.is_empty() {
        return "` `".to_owned();
    }
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

fn is_bidi(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
}
