//! Check run (ARCHITECTURE.md §1.9.1): one execution of one check in one
//! sandbox, and the key a contract check's runs are cached under (§1.9.6).

use serde::{Deserialize, Serialize};

use crate::contract::ContractCheckId;
use crate::env::{HostArch, HostOs};
use crate::host::{HostProfile, HostTool};
use crate::id::{kind, BlobId, EnvId, SnapshotId, TreeId};
use crate::key::{derive, hex_key};
use crate::object::{impl_object, Schema};
use crate::scalar::{CheckId, Path, TestCheckId, Text, Timestamp};
use crate::set::{Set, SetItem};
use crate::ParseError;

pub const MAX_TEST_MESSAGE_BYTES: usize = 2048;

/// Serialized with `outcome`, `infra` and `exit_code` fields derived from
/// [`RunResult`], so `infra` is present iff the outcome is `infra` and
/// `exit_code` is present unless the sandbox itself failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCheckRun", into = "RawCheckRun")]
pub struct CheckRun {
    pub schema: Schema<CheckRun>,
    /// The code that ran.
    pub snapshot: SnapshotId,
    /// The environment it ran in, which may differ from the snapshot's.
    pub env: EnvId,
    pub check: CheckId,
    pub index: RunIndex,
    /// Executor profile id, e.g. `docker-runc-v0`.
    pub executor: String,
    pub result: RunResult,
    pub duration_ms: u32,
    /// Combined stdout and stderr, bounded.
    pub output: BlobId,
    /// Raw JUnit XML, when the check declares a report.
    pub report: Option<BlobId>,
    pub tests: Set<TestResult>,
    pub started_at: Timestamp,
}

impl_object!(CheckRun, kind::Run, "aver.run/v0");

/// How a run ended, with the detail each ending carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult {
    Pass {
        exit_code: i32,
    },
    Fail {
        exit_code: i32,
    },
    Oom {
        exit_code: i32,
    },
    Timeout {
        exit_code: i32,
    },
    /// The run could not start or report (missing tool, full disk,
    /// malformed result).
    Sandbox,
}

impl RunResult {
    pub fn outcome(&self) -> Outcome {
        self.ending().outcome()
    }

    /// The outcome with `infra` split into a resource cap and a sandbox
    /// failure, which is what the cause rules look at.
    pub fn ending(&self) -> Ending {
        match self {
            Self::Pass { .. } => Ending::Pass,
            Self::Fail { .. } => Ending::Fail,
            Self::Oom { .. } | Self::Timeout { .. } => Ending::Cap,
            Self::Sandbox => Ending::Sandbox,
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        match *self {
            Self::Pass { exit_code }
            | Self::Fail { exit_code }
            | Self::Oom { exit_code }
            | Self::Timeout { exit_code } => Some(exit_code),
            Self::Sandbox => None,
        }
    }
}

/// The coarse outcome of a run, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    Infra,
}

/// How a run ended as the cause rules see it (ARCHITECTURE.md §1.9.3): an
/// `infra` outcome is either a resource cap the check hit (out of memory or
/// timeout) or a sandbox that failed around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ending {
    Pass,
    Fail,
    /// The run hit the memory or time limit.
    Cap,
    /// The sandbox failed: no container, a Docker error, a malformed result.
    Sandbox,
}

impl Ending {
    pub fn outcome(self) -> Outcome {
        match self {
            Self::Pass => Outcome::Pass,
            Self::Fail => Outcome::Fail,
            Self::Cap | Self::Sandbox => Outcome::Infra,
        }
    }

    pub fn is_infra(self) -> bool {
        matches!(self, Self::Cap | Self::Sandbox)
    }
}

impl From<Outcome> for Ending {
    /// A test-level outcome: tests never end in infra, and an `infra` outcome
    /// without its kind counts as a sandbox failure.
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Pass => Self::Pass,
            Outcome::Fail => Self::Fail,
            Outcome::Infra => Self::Sandbox,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Infra {
    Oom,
    Timeout,
    Sandbox,
}

/// Rerun index: 1, 2 or 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct RunIndex(u8);

impl RunIndex {
    pub const FIRST: Self = Self(1);
    pub const SECOND: Self = Self(2);
    pub const THIRD: Self = Self(3);

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for RunIndex {
    type Error = ParseError;
    fn try_from(n: u8) -> Result<Self, ParseError> {
        if (1..=3).contains(&n) {
            Ok(Self(n))
        } else {
            Err(ParseError::invalid(
                "run index",
                format!("{n} is not 1, 2 or 3"),
            ))
        }
    }
}

impl From<RunIndex> for u8 {
    fn from(index: RunIndex) -> u8 {
        index.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestResult {
    pub id: TestCheckId,
    pub status: TestStatus,
    /// Failure message excerpt; failures only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Text<MAX_TEST_MESSAGE_BYTES>>,
}

impl SetItem for TestResult {
    type Key = str;
    fn key(&self) -> &str {
        self.id.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestStatus {
    Pass,
    Fail,
    Skip,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheckRun {
    schema: Schema<CheckRun>,
    snapshot: SnapshotId,
    env: EnvId,
    check: CheckId,
    index: RunIndex,
    executor: String,
    outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    infra: Option<Infra>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    duration_ms: u32,
    output: BlobId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report: Option<BlobId>,
    tests: Set<TestResult>,
    started_at: Timestamp,
}

impl TryFrom<RawCheckRun> for CheckRun {
    type Error = ParseError;
    fn try_from(raw: RawCheckRun) -> Result<Self, ParseError> {
        let result = match (raw.outcome, raw.infra, raw.exit_code) {
            (Outcome::Pass, None, Some(exit_code)) => RunResult::Pass { exit_code },
            (Outcome::Fail, None, Some(exit_code)) => RunResult::Fail { exit_code },
            (Outcome::Infra, Some(Infra::Oom), Some(exit_code)) => RunResult::Oom { exit_code },
            (Outcome::Infra, Some(Infra::Timeout), Some(exit_code)) => {
                RunResult::Timeout { exit_code }
            }
            (Outcome::Infra, Some(Infra::Sandbox), None) => RunResult::Sandbox,
            _ => {
                return Err(ParseError::invalid(
                    "check run",
                    "infra must be present iff outcome is infra, and exit_code present unless infra is sandbox",
                ))
            }
        };
        Ok(Self {
            schema: raw.schema,
            snapshot: raw.snapshot,
            env: raw.env,
            check: raw.check,
            index: raw.index,
            executor: raw.executor,
            result,
            duration_ms: raw.duration_ms,
            output: raw.output,
            report: raw.report,
            tests: raw.tests,
            started_at: raw.started_at,
        })
    }
}

impl From<CheckRun> for RawCheckRun {
    fn from(run: CheckRun) -> Self {
        let infra = match run.result {
            RunResult::Pass { .. } | RunResult::Fail { .. } => None,
            RunResult::Oom { .. } => Some(Infra::Oom),
            RunResult::Timeout { .. } => Some(Infra::Timeout),
            RunResult::Sandbox => Some(Infra::Sandbox),
        };
        Self {
            schema: run.schema,
            snapshot: run.snapshot,
            env: run.env,
            check: run.check,
            index: run.index,
            executor: run.executor,
            outcome: run.result.outcome(),
            infra,
            exit_code: run.result.exit_code(),
            duration_ms: run.duration_ms,
            output: run.output,
            report: run.report,
            tests: run.tests,
            started_at: run.started_at,
        }
    }
}

/// What a contract check's run is cached under: the same tree, check, place,
/// installed dependencies and rerun index give the same result, so a base
/// run recorded once serves every change on that base.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunKey {
    pub tree: TreeId,
    pub check: ContractCheckId,
    pub place: PlaceKey,
    pub deps: DepsState,
    pub index: RunIndex,
}

impl RunKey {
    pub fn id(&self) -> RunKeyId {
        RunKeyId(derive("run-key", self))
    }
}

hex_key!(
    /// Names a [`RunKey`] in the run cache.
    RunKeyId,
    "run key"
);

/// Where a run happened, as far as its result can depend on it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum PlaceKey {
    Host(HostKey),
}

/// The OS, the CPU, and the real path and `--version` output of every tool
/// the check invokes, so a run from another machine or toolchain is never
/// reused: a [`HostProfile`] over the check's own tools.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostKey(HostProfile);

impl HostKey {
    pub fn of(os: HostOs, arch: HostArch, tools: &[HostTool]) -> Self {
        Self(HostProfile::of(os, arch, tools))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

hex_key!(
    /// The dependency inputs (lockfiles, manifests) of the tree the kept
    /// directories were installed from, by path and blob.
    DepsState,
    "deps state"
);

impl DepsState {
    /// The hash of the `(path, blob)` pairs, sorted, so their order and
    /// repeats do not matter.
    pub fn of<'a>(inputs: impl IntoIterator<Item = (&'a Path, &'a BlobId)>) -> Self {
        let mut rows: Vec<(&str, String)> = inputs
            .into_iter()
            .map(|(path, blob)| (path.as_str(), blob.to_string()))
            .collect();
        rows.sort();
        rows.dedup();
        Self(derive("deps-state", &rows))
    }
}
