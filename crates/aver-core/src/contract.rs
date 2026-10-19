//! Contract (ARCHITECTURE.md §1.13): how a repository says its tests run.
//!
//! A contract is the repository's own commands, read from `aver.toml`, run
//! in the caller's existing environment. It replaces [`crate::Environment`]
//! for new runs. Who approved it decides how far a verdict over it is
//! trusted ([`Approval`], [`Trust`]).

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::id::{kind, BlobId};
use crate::key::{derive, hex_key};
use crate::object::{impl_object, Schema};
use crate::scalar::{CheckId, Path, PathGlob, ShellLine};
use crate::set::{Set, SetItem};
use crate::ParseError;

/// Most checks a contract declares.
pub const MAX_CHECKS: usize = 32;
/// Most patterns in any one glob set of a contract.
pub const MAX_GLOBS: usize = 64;

/// Built test files judged by default when `aver.toml` names none: untracked
/// files a build writes from the test sources.
pub const DEFAULT_BUILT_TESTS: &[&str] = &["dist/**/test/**", "build/**/test/**", "**/*.test.js"];

/// Sensitive paths when `aver.toml` names none: what decides how the
/// repository's own CI runs, so a change to it is for a person to review.
pub const DEFAULT_SENSITIVE: &[&str] = &[".github/actions/**", ".github/workflows/**"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ContractFields")]
pub struct Contract {
    pub schema: Schema<Contract>,
    /// 1 to [`MAX_CHECKS`], unique ids.
    pub checks: Set<ContractCheck>,
    pub deps: Deps,
    pub judge: JudgeGlobs,
    pub ci: CiPolicy,
}

impl_object!(Contract, kind::Contract, "aver.contract/v1");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractFields {
    /// Checked by parsing; [`Contract::new`] writes it.
    #[serde(rename = "schema")]
    _schema: Schema<Contract>,
    checks: Set<ContractCheck>,
    deps: Deps,
    judge: JudgeGlobs,
    ci: CiPolicy,
}

impl TryFrom<ContractFields> for Contract {
    type Error = ParseError;
    fn try_from(f: ContractFields) -> Result<Self, ParseError> {
        Self::new(f.checks, f.deps, f.judge, f.ci)
    }
}

impl Contract {
    pub fn new(
        checks: Set<ContractCheck>,
        deps: Deps,
        judge: JudgeGlobs,
        ci: CiPolicy,
    ) -> Result<Self, ParseError> {
        if checks.is_empty() || checks.len() > MAX_CHECKS {
            return Err(ParseError::invalid(
                "contract",
                format!("declares 1 to {MAX_CHECKS} checks, not {}", checks.len()),
            ));
        }
        Ok(Self {
            schema: Schema::new(),
            checks,
            deps,
            judge,
            ci,
        })
    }

    pub fn check(&self, id: &CheckId) -> Option<&ContractCheck> {
        self.checks.get(id.as_str())
    }
}

/// One command of the contract and how its result is read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CheckSpec", into = "CheckSpec")]
pub struct ContractCheck {
    pub id: CheckId,
    /// Run with `sh -c` in `cwd`, with `$AVER_OUT` set to a fresh directory.
    pub run: ShellLine,
    /// Relative to the repository root; the root when absent.
    pub cwd: Option<Path>,
    /// Run before `run` on every side, in that side's tree.
    pub prepare: Option<ShellLine>,
    pub reading: Reading,
    /// The files whose change makes this check run; `None` runs it always.
    pub paths: Option<Set<PathGlob>>,
    /// Sides run one at a time (services, fixed ports).
    pub serial: bool,
    pub timeout_s: Option<NonZeroU32>,
}

impl SetItem for ContractCheck {
    type Key = str;
    fn key(&self) -> &str {
        self.id.as_str()
    }
}

impl ContractCheck {
    /// The floor on the base's test count; `None` for an exit-only check.
    pub fn min_tests(&self) -> Option<NonZeroU32> {
        match self.reading {
            Reading::Tests { min_tests, .. } => Some(min_tests),
            Reading::Exit => None,
        }
    }

    /// The key runs of this check are cached under: a hash of every field,
    /// so a changed command, directory or reading is a different check.
    pub fn key(&self) -> ContractCheckId {
        ContractCheckId(derive("contract-check", self))
    }
}

hex_key!(
    /// Names a contract check in run keys ([`crate::RunKey`]).
    ContractCheckId,
    "contract check id"
);

/// How a check's result is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// Test by test from a report, with a floor on how many tests the base
    /// must run (90% of the approved count when the dialogue writes it).
    Tests {
        report: ReportSpec,
        min_tests: NonZeroU32,
    },
    /// The exit code alone. Never evidence of a test, so it counts only when
    /// the base's `aver.toml` approved it (§4.1).
    Exit,
}

/// Where a check's per-test report is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportSpec {
    /// Every `*.xml` file the check writes into `$AVER_OUT`.
    Junit,
    /// The check's standard output.
    Stdout(TextFormat),
    /// A file the check writes, relative to `$AVER_OUT`.
    File { format: TextFormat, path: Path },
}

/// A per-test report format other than JUnit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextFormat {
    Libtest,
    GoJson,
    Tsc,
    JestJson,
    Tap,
}

/// The `report` value of a check, as `aver.toml` and the wire spell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportKind {
    Junit,
    Libtest,
    GoJson,
    Tsc,
    JestJson,
    Tap,
    Exit,
}

impl ReportKind {
    fn text(self) -> Option<TextFormat> {
        match self {
            Self::Libtest => Some(TextFormat::Libtest),
            Self::GoJson => Some(TextFormat::GoJson),
            Self::Tsc => Some(TextFormat::Tsc),
            Self::JestJson => Some(TextFormat::JestJson),
            Self::Tap => Some(TextFormat::Tap),
            Self::Junit | Self::Exit => None,
        }
    }
}

impl From<TextFormat> for ReportKind {
    fn from(format: TextFormat) -> Self {
        match format {
            TextFormat::Libtest => Self::Libtest,
            TextFormat::GoJson => Self::GoJson,
            TextFormat::Tsc => Self::Tsc,
            TextFormat::JestJson => Self::JestJson,
            TextFormat::Tap => Self::Tap,
        }
    }
}

/// The flat fields of a [`ContractCheck`], as `aver.toml` and the canonical
/// JSON hold them, before the reading rules are checked: `min_tests` is
/// present exactly when `report` is not `exit`, and `report_file` only with
/// a text report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    pub id: CheckId,
    pub run: ShellLine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare: Option<ShellLine>,
    pub report: ReportKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_file: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_tests: Option<NonZeroU32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Set<PathGlob>>,
    pub serial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<NonZeroU32>,
}

impl TryFrom<CheckSpec> for ContractCheck {
    type Error = ParseError;
    fn try_from(s: CheckSpec) -> Result<Self, ParseError> {
        let id = &s.id;
        let invalid =
            |reason: &str| ParseError::invalid("contract check", format!("{id}: {reason}"));
        let reading = match (s.report, s.min_tests) {
            (ReportKind::Exit, None) if s.report_file.is_none() => Reading::Exit,
            (ReportKind::Exit, _) => return Err(invalid(
                "an exit-only check counts no tests, so it takes neither min_tests nor report_file",
            )),
            (_, None) => return Err(invalid("min_tests is required unless report is \"exit\"")),
            (kind, Some(min_tests)) => {
                let report =
                    match (kind.text(), s.report_file) {
                        (None, None) => ReportSpec::Junit,
                        (None, Some(_)) => return Err(invalid(
                            "report_file names a text report; JUnit is every *.xml in $AVER_OUT",
                        )),
                        (Some(format), None) => ReportSpec::Stdout(format),
                        (Some(format), Some(path)) => ReportSpec::File { format, path },
                    };
                Reading::Tests { report, min_tests }
            }
        };
        if let Some(paths) = &s.paths {
            if paths.is_empty() || paths.len() > MAX_GLOBS {
                return Err(invalid(&format!(
                    "paths holds 1 to {MAX_GLOBS} patterns; omit it to run the check always"
                )));
            }
        }
        Ok(Self {
            id: s.id,
            run: s.run,
            cwd: s.cwd,
            prepare: s.prepare,
            reading,
            paths: s.paths,
            serial: s.serial,
            timeout_s: s.timeout_s,
        })
    }
}

impl From<ContractCheck> for CheckSpec {
    fn from(c: ContractCheck) -> Self {
        let (report, report_file, min_tests) = match c.reading {
            Reading::Exit => (ReportKind::Exit, None, None),
            Reading::Tests { report, min_tests } => match report {
                ReportSpec::Junit => (ReportKind::Junit, None, Some(min_tests)),
                ReportSpec::Stdout(format) => (format.into(), None, Some(min_tests)),
                ReportSpec::File { format, path } => (format.into(), Some(path), Some(min_tests)),
            },
        };
        Self {
            id: c.id,
            run: c.run,
            cwd: c.cwd,
            prepare: c.prepare,
            report,
            report_file,
            min_tests,
            paths: c.paths,
            serial: c.serial,
            timeout_s: c.timeout_s,
        }
    }
}

fn check_globs(what: &str, field: &str, globs: &Set<PathGlob>) -> Result<(), ParseError> {
    if globs.len() > MAX_GLOBS {
        return Err(ParseError::invalid(
            "contract",
            format!("{what}.{field} holds at most {MAX_GLOBS} patterns"),
        ));
    }
    Ok(())
}

/// The untracked dependency and build directories the sides share.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DepsFields")]
pub struct Deps {
    /// Kept across tree swaps and shared by the sides (`node_modules`).
    pub keep: Set<PathGlob>,
    /// Cloned once per side directory (`target`).
    pub seed: Set<PathGlob>,
    /// Run only when the `inputs` differ between the base and the head.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<ShellLine>,
    /// The files the kept directories were installed from (lockfiles).
    pub inputs: Set<PathGlob>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DepsFields {
    keep: Set<PathGlob>,
    seed: Set<PathGlob>,
    #[serde(default)]
    install: Option<ShellLine>,
    inputs: Set<PathGlob>,
}

impl TryFrom<DepsFields> for Deps {
    type Error = ParseError;
    fn try_from(f: DepsFields) -> Result<Self, ParseError> {
        Self::new(f.keep, f.seed, f.install, f.inputs)
    }
}

impl Deps {
    pub fn new(
        keep: Set<PathGlob>,
        seed: Set<PathGlob>,
        install: Option<ShellLine>,
        inputs: Set<PathGlob>,
    ) -> Result<Self, ParseError> {
        check_globs("deps", "keep", &keep)?;
        check_globs("deps", "seed", &seed)?;
        check_globs("deps", "inputs", &inputs)?;
        if install.is_some() && inputs.is_empty() {
            return Err(ParseError::invalid(
                "contract",
                "deps.install runs only when deps.inputs differ, so it needs inputs",
            ));
        }
        Ok(Self {
            keep,
            seed,
            install,
            inputs,
        })
    }
}

/// Which files are judged as tests, harness, built tests or protected paths,
/// beyond the defaults. Read only from the base's `aver.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "JudgeFields")]
pub struct JudgeGlobs {
    /// Added to the default test file patterns.
    pub tests: Set<PathGlob>,
    /// Added to the default frozen harness patterns.
    pub harness: Set<PathGlob>,
    /// Untracked files a run writes that are fingerprinted on both sides.
    pub built_tests: Set<PathGlob>,
    /// Paths the head may not change. Never from a PR body.
    pub protected: Set<PathGlob>,
    /// Paths a change to which is `CHANGED`, for a person to review, even
    /// when every test passes. Read from the base, so a change cannot drop
    /// its own sensitivity.
    pub sensitive: Set<PathGlob>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JudgeFields {
    tests: Set<PathGlob>,
    harness: Set<PathGlob>,
    built_tests: Set<PathGlob>,
    protected: Set<PathGlob>,
    sensitive: Set<PathGlob>,
}

impl TryFrom<JudgeFields> for JudgeGlobs {
    type Error = ParseError;
    fn try_from(f: JudgeFields) -> Result<Self, ParseError> {
        for (field, globs) in [
            ("tests", &f.tests),
            ("harness", &f.harness),
            ("built_tests", &f.built_tests),
            ("protected", &f.protected),
            ("sensitive", &f.sensitive),
        ] {
            check_globs("judge", field, globs)?;
        }
        Ok(Self {
            tests: f.tests,
            harness: f.harness,
            built_tests: f.built_tests,
            protected: f.protected,
            sensitive: f.sensitive,
        })
    }
}

impl JudgeGlobs {
    /// [`DEFAULT_BUILT_TESTS`] as a set.
    pub fn default_built_tests() -> Set<PathGlob> {
        defaults(DEFAULT_BUILT_TESTS)
    }

    /// [`DEFAULT_SENSITIVE`] as a set.
    pub fn default_sensitive() -> Set<PathGlob> {
        defaults(DEFAULT_SENSITIVE)
    }

    /// The first sensitive pattern that covers `path`.
    pub fn sensitive_match(&self, path: &str) -> Option<&PathGlob> {
        self.sensitive.iter().find(|g| g.covers(path))
    }
}

fn defaults(globs: &[&str]) -> Set<PathGlob> {
    Set::new(
        globs
            .iter()
            .map(|g| g.parse().expect("default globs are valid"))
            .collect(),
    )
    .expect("default globs are distinct and sorted")
}

impl Default for JudgeGlobs {
    /// No added patterns, [`DEFAULT_BUILT_TESTS`] and
    /// [`DEFAULT_SENSITIVE`].
    fn default() -> Self {
        Self {
            tests: Set::empty(),
            harness: Set::empty(),
            built_tests: Self::default_built_tests(),
            protected: Set::empty(),
            sensitive: Self::default_sensitive(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiPolicy {
    pub review: ReviewPolicy,
    /// A base run inline in the PR's job blocks the verdict instead of
    /// counting with less trust.
    pub require_recorded_base: bool,
}

/// What CI reports for a REVIEW verdict.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewPolicy {
    /// The check blocks until a human other than the author approves.
    #[default]
    Approve,
    /// The check is neutral.
    Neutral,
}

/// Who approved the contract a verdict ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Approval {
    /// The base tree's `aver.toml`: a human merged it.
    Base { blob: BlobId },
    /// The head tree's `aver.toml` where the base has none (a bootstrap).
    Proposed { blob: BlobId },
    /// A command given on the command line (`aver check -- <cmd>`).
    Adhoc { line: ShellLine },
}

impl Approval {
    /// Whether an exit-only check counts as evidence: only when a human
    /// merged it into the base.
    pub fn approves_exit(&self) -> bool {
        matches!(self, Self::Base { .. })
    }
}

/// Where a verdict was reached, as trust sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Venue {
    Ci,
    Local,
}

/// Whether the change edits CI workflows the base did not approve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Workflows {
    Unchanged,
    EditedUnapproved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "trust", content = "why", rename_all = "lowercase")]
pub enum Trust {
    Trusted,
    Advisory(AdvisoryKind),
}

/// Why a verdict is advisory, the weakest reason first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvisoryKind {
    /// The contract is an ad-hoc command line.
    Adhoc,
    /// The contract is the head's own proposal.
    Proposed,
    /// The runs were made on a developer's or agent's machine.
    Local,
    /// The change edits workflows that run the check.
    WorkflowEdited,
}

impl Trust {
    /// Trusted only for a base-approved contract run in CI on a change that
    /// leaves the workflows alone; otherwise the weakest reason.
    pub fn of(approval: &Approval, venue: Venue, workflows: Workflows) -> Self {
        let reason = match (approval, venue, workflows) {
            (Approval::Adhoc { .. }, _, _) => AdvisoryKind::Adhoc,
            (Approval::Proposed { .. }, _, _) => AdvisoryKind::Proposed,
            (Approval::Base { .. }, Venue::Local, _) => AdvisoryKind::Local,
            (Approval::Base { .. }, Venue::Ci, Workflows::EditedUnapproved) => {
                AdvisoryKind::WorkflowEdited
            }
            (Approval::Base { .. }, Venue::Ci, Workflows::Unchanged) => return Self::Trusted,
        };
        Self::Advisory(reason)
    }
}
