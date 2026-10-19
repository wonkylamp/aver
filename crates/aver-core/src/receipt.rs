//! Receipt (ARCHITECTURE.md §1.9.3), signed by the server's verifier key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::host::HostProfile;
use crate::id::{kind, AgentId, AttemptId, EnvId, RunId, SnapshotId, TaskId};
use crate::object::{nullable, Versioned};
use crate::scalar::{
    CheckId, CriterionId, EntryId, GitSha, Path, Subject, TestCheckId, Text, Timestamp,
};
use crate::set::{Set, SetItem};
use crate::sig::{Signed, SignedBody};
use crate::task::Criteria;
use crate::ParseError;

pub const MAX_DETAIL_BYTES: usize = 2048;
/// Most items a [`Listed`] of [`TestChanges`] names; the rest are counted.
pub const MAX_LISTED: usize = 20;
/// Longest executor profile id a receipt records.
pub const MAX_EXECUTOR_BYTES: usize = 64;

/// The check id of judging-file entries (`judging::<path>`, §1.9.3). In v2 a
/// receipt with one proves no criterion.
pub const JUDGING_CHECK: &str = "judging";

/// The check id of built-artifact entries (`built::<path>`, §1.9.3): a
/// compiled test file that differs between the base and the judged head.
pub const BUILT_CHECK: &str = "built";

pub type Receipt = Signed<ReceiptBody>;

/// The receipt schema versions this crate reads. v1 allows an empty criteria
/// list, which a receipt for a v2 task carries; a v0 or v1 receipt carries v1
/// exactly when its criteria are empty, so every v0 receipt keeps its bytes
/// and id. v2 adds the transplant proof rule and the CI fields and takes any
/// criteria. v3 adds `provenance` (where and how the runs executed: natively
/// on the verifier's machine, in CI, or in the sandbox) and `levels` (how
/// each check was read), so a consumer that knows only v2 fails closed
/// rather than reading a native receipt as sandboxed; verification writes
/// only v3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptSchema {
    #[serde(rename = "aver.receipt/v0")]
    V0,
    #[serde(rename = "aver.receipt/v1")]
    V1,
    #[serde(rename = "aver.receipt/v2")]
    V2,
    #[serde(rename = "aver.receipt/v3")]
    V3,
}

impl ReceiptSchema {
    /// The pre-v2 version a receipt over these criteria carries.
    pub fn for_criteria(criteria: &Criteria) -> Self {
        if criteria.is_empty() {
            Self::V1
        } else {
            Self::V0
        }
    }

    /// How a receipt of this version decides that a criterion is proven.
    pub fn proof_rule(self) -> ProofRule {
        match self {
            Self::V0 | Self::V1 => ProofRule::HeadPasses,
            Self::V2 | Self::V3 => ProofRule::Transplant,
        }
    }
}

/// When an entry that proves a criterion proves it (§1.9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofRule {
    /// v0 and v1: the entry passes on head with no cause.
    HeadPasses,
    /// v2: also, it failed or was absent on the transplant tree, passed in
    /// the head-tests run, and the head changed no judging file.
    Transplant,
}

/// Where a v3 receipt's runs executed (§1.9.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub mode: ProvenanceMode,
    /// The host profile of every native run (§1.4.3); present exactly when
    /// `mode` is `local` or `ci`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_profile: Option<HostProfile>,
    /// Whether the check runs could reach the network.
    pub network: Network,
    /// Whether the check runs ran as another user than the verifier.
    pub uid: Uid,
}

/// How a verification's runs executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProvenanceMode {
    /// Natively on the verifier's own machine, as its user: the receipt
    /// trusts the agent and the host.
    Local,
    /// Natively on a throwaway CI runner (`aver ci`).
    Ci,
    /// In the hardened Docker sandbox (§2.4.1).
    Sandbox,
}

impl ProvenanceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ci => "ci",
            Self::Sandbox => "sandbox",
        }
    }
}

/// Whether check runs could reach the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Off,
    On,
}

/// Whether check runs ran as a user other than the verifier's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Uid {
    /// A user that cannot read the verifier's key.
    Separate,
    /// The verifier's own user.
    Same,
}

/// How a check's results were read (§1.9.3, v3): by a parser Aver has for
/// the runner, from JUnit XML, or from the exit code alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Native,
    Junit,
    Exit,
}

impl Level {
    /// The level of a check that declares `report`, or none.
    pub fn of(report: Option<&crate::env::Report>) -> Self {
        match report.map(|r| r.format) {
            None => Self::Exit,
            Some(crate::env::ReportFormat::Junit) => Self::Junit,
            Some(_) => Self::Native,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Junit => "junit",
            Self::Exit => "exit",
        }
    }
}

/// What a CI receipt records about the run that produced it (§2.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiInfo {
    pub base_commit: GitSha,
    pub head_commit: GitSha,
    /// `aver.toml` when the base has one, else `null` (default environment).
    #[serde(deserialize_with = "nullable")]
    pub config_path: Option<Path>,
    /// The normalized criteria block, `null` when the run declared none.
    #[serde(deserialize_with = "nullable")]
    pub criteria_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
    /// The commit `--base` resolved to, present exactly when it is not an
    /// ancestor of the head, so that `base_commit` is their merge base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_base: Option<GitSha>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReceiptFields")]
pub struct ReceiptBody {
    pub schema: ReceiptSchema,
    pub task_id: TaskId,
    /// `null` when verify runs outside an attempt.
    pub attempt_id: Option<AttemptId>,
    pub agent_id: AgentId,
    pub base_snapshot: SnapshotId,
    pub head_snapshot: SnapshotId,
    /// The environment both sides ran in.
    pub environment_hash: EnvId,
    /// Whether the head's environment differs from the base's.
    pub env_changed: bool,
    pub criteria: Criteria,
    pub checks: Set<ReceiptCheck>,
    /// In criteria order.
    pub unproven: Vec<CriterionId>,
    pub verdict: Verdict,
    pub reproduce: String,
    /// Verifier version.
    pub verifier: String,
    pub created_at: Timestamp,
    /// v2: the executor profile every run of the verification used, e.g.
    /// `docker-runsc-v0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// v2: where the configuration every check ran under came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigSource>,
    /// v2: what the head did to the project's tests and harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_changes: Option<TestChanges>,
    /// v2 receipts written by `aver ci` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiInfo>,
    /// v2, optional: the source files the head changed that no check could
    /// exercise (§1.9.3), present only when there are some. A receipt with
    /// any cannot be `PASS` or `REVIEW`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncovered: Option<Listed<Path>>,
    /// v2, optional: the checks that ran on neither side, and why (§1.9.3),
    /// sorted by check; present only when there are some. A skipped check
    /// has no entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Set<Skipped>>,
    /// v3: where and how the runs executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    /// v3: how each check that ran was read, by check id: every check of
    /// the verification that is not `skipped`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<BTreeMap<CheckId, Level>>,
}

/// A check a verification did not run (§1.9.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skipped {
    pub check: CheckId,
    pub reason: SkipReason,
}

impl SetItem for Skipped {
    type Key = str;
    fn key(&self) -> &str {
        self.check.as_str()
    }
}

/// Why a check did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    /// Nothing the head changed is a file the check covers, a lockfile it
    /// installs from, or harness on the way to its directory, so it would
    /// report on the head what it reports on the base.
    #[serde(rename = "not_affected")]
    NotAffected,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAffected => "not_affected",
        }
    }
}

/// Where a verification's configuration came from (§1.9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConfigSource {
    /// The base's `aver.toml`.
    #[serde(rename = "aver.toml")]
    AverToml,
    /// The base has no `aver.toml`: the default environment, one `npm test`
    /// check.
    #[serde(rename = "default")]
    Default,
}

/// What a head did to the tests (§1.9.3), so a reviewer sees tampering at a
/// glance. Paths and test ids are repository content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestChanges {
    /// Test files the head added.
    pub files_added: Listed<Path>,
    /// Test files that exist at base and the head changed (contents or kind).
    pub files_modified: Listed<Path>,
    /// Test files that exist at base and the head deleted.
    pub files_deleted: Listed<Path>,
    /// Tests with a result at base that no head-tests report mentions.
    pub tests_removed: Listed<TestCheckId>,
    /// Tests with a result at base that every head-tests report skips.
    pub tests_skipped: Listed<TestCheckId>,
    /// The judging-file entries: frozen harness the head changed.
    pub harness_changed: Listed<EntryId>,
}

/// Up to [`MAX_LISTED`] items in canonical order, and how many more there
/// were.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "ListedFields<T>")]
#[serde(bound(deserialize = "T: SetItem + Deserialize<'de>"))]
pub struct Listed<T: SetItem> {
    pub items: Set<T>,
    pub omitted: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(bound(deserialize = "T: SetItem + Deserialize<'de>"))]
struct ListedFields<T: SetItem> {
    items: Set<T>,
    omitted: u32,
}

impl<T: SetItem> TryFrom<ListedFields<T>> for Listed<T> {
    type Error = ParseError;
    fn try_from(f: ListedFields<T>) -> Result<Self, ParseError> {
        if f.items.len() > MAX_LISTED {
            return Err(ParseError::invalid(
                "receipt",
                format!("a test_changes list names at most {MAX_LISTED} items"),
            ));
        }
        Ok(Self {
            items: f.items,
            omitted: f.omitted,
        })
    }
}

impl<T: SetItem + PartialEq> Listed<T> {
    /// The first [`MAX_LISTED`] of `items` in canonical order, the rest counted.
    pub fn of(items: Vec<T>) -> Self {
        let all = Set::new(items).expect("listed items are their own keys");
        let total = all.len();
        let mut items = all.into_vec();
        items.truncate(MAX_LISTED);
        Self {
            omitted: u32::try_from(total - items.len()).unwrap_or(u32::MAX),
            items: Set::new(items).expect("a prefix of a set is a set"),
        }
    }
}

impl<T: SetItem> Listed<T> {
    /// How many items there were, listed or not.
    pub fn total(&self) -> u64 {
        self.items.len() as u64 + u64::from(self.omitted)
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

impl TestChanges {
    /// Whether the head changed no test file and no harness, and every test
    /// that ran at base still ran.
    pub fn is_empty(&self) -> bool {
        self.files_added.is_empty()
            && self.files_modified.is_empty()
            && self.files_deleted.is_empty()
            && self.tests_removed.is_empty()
            && self.tests_skipped.is_empty()
            && self.harness_changed.is_empty()
    }
}

/// The wire fields of [`ReceiptBody`], before the version rule is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptFields {
    schema: ReceiptSchema,
    task_id: TaskId,
    #[serde(deserialize_with = "nullable")]
    attempt_id: Option<AttemptId>,
    agent_id: AgentId,
    base_snapshot: SnapshotId,
    head_snapshot: SnapshotId,
    environment_hash: EnvId,
    env_changed: bool,
    criteria: Criteria,
    checks: Set<ReceiptCheck>,
    unproven: Vec<CriterionId>,
    verdict: Verdict,
    reproduce: String,
    verifier: String,
    created_at: Timestamp,
    #[serde(default)]
    executor: Option<String>,
    #[serde(default)]
    config: Option<ConfigSource>,
    #[serde(default)]
    test_changes: Option<TestChanges>,
    #[serde(default)]
    ci: Option<CiInfo>,
    #[serde(default)]
    uncovered: Option<Listed<Path>>,
    #[serde(default)]
    skipped: Option<Set<Skipped>>,
    #[serde(default)]
    provenance: Option<Provenance>,
    #[serde(default)]
    levels: Option<BTreeMap<CheckId, Level>>,
}

impl TryFrom<ReceiptFields> for ReceiptBody {
    type Error = ParseError;
    fn try_from(f: ReceiptFields) -> Result<Self, ParseError> {
        let invalid = |reason: &str| Err(ParseError::invalid("receipt", reason));
        let v2_fields = f.ci.is_some()
            || f.uncovered.is_some()
            || f.skipped.is_some()
            || f.verdict == Verdict::Review
            || f.executor.is_some()
            || f.config.is_some()
            || f.test_changes.is_some()
            || f.checks.iter().any(|c| {
                c.cause == Some(Cause::Changed)
                    || c.transplant.is_some()
                    || c.head_tests.is_some()
                    || !c.runs.transplant.is_empty()
                    || !c.runs.head_tests.is_empty()
            });
        if (f.provenance.is_some() || f.levels.is_some()) && f.schema != ReceiptSchema::V3 {
            return invalid("provenance and levels exist only in aver.receipt/v3");
        }
        match f.schema {
            ReceiptSchema::V0 | ReceiptSchema::V1 => {
                if f.schema != ReceiptSchema::for_criteria(&f.criteria) {
                    return invalid("a receipt carries aver.receipt/v1 exactly when its criteria are empty, else v0 or v2");
                }
                if v2_fields {
                    return invalid(
                        "ci, executor, config, test_changes, uncovered, skipped, transplant, head_tests and their runs, cause CHANGED and verdict REVIEW exist only in aver.receipt/v2",
                    );
                }
            }
            ReceiptSchema::V2 | ReceiptSchema::V3 => {
                if f.schema == ReceiptSchema::V3 {
                    check_v3(&f)?;
                }
                match &f.executor {
                    None => return invalid("an aver.receipt/v2 names its executor"),
                    Some(e) if !is_executor(e) => {
                        return invalid("executor must match [A-Za-z0-9._-]{1,64}")
                    }
                    Some(_) => {}
                }
                if f.config.is_none() {
                    return invalid("an aver.receipt/v2 says where its configuration came from");
                }
                if f.test_changes.is_none() {
                    return invalid("an aver.receipt/v2 carries test_changes");
                }
                if f.uncovered.as_ref().is_some_and(Listed::is_empty) {
                    return invalid("uncovered is omitted rather than empty");
                }
                if f.uncovered.is_some() && matches!(f.verdict, Verdict::Pass | Verdict::Review) {
                    return invalid("a receipt with uncovered changes is neither PASS nor REVIEW");
                }
                if let Some(skipped) = &f.skipped {
                    if skipped.is_empty() {
                        return invalid("skipped is omitted rather than empty");
                    }
                    let has_entry = |check: &str| {
                        f.checks.iter().any(|c| {
                            let id = c.id.as_str();
                            id == check
                                || id
                                    .strip_prefix(check)
                                    .is_some_and(|rest| rest.starts_with("::"))
                        })
                    };
                    if skipped.iter().any(|s| has_entry(s.check.as_str())) {
                        return invalid("a skipped check has no entry");
                    }
                }
                if let Some(ci) = &f.ci {
                    if ci.requested_base.as_ref() == Some(&ci.base_commit) {
                        return invalid(
                            "ci.requested_base is present only when it differs from ci.base_commit",
                        );
                    }
                }
                for c in f.checks.iter() {
                    if c.transplant.is_some() && c.proves.is_empty() {
                        return invalid(
                            "transplant is recorded only on an entry that proves a criterion",
                        );
                    }
                    if c.transplant.is_some() == c.runs.transplant.is_empty() {
                        return invalid(
                            "runs.transplant lists runs exactly when transplant is present",
                        );
                    }
                    if c.transplant.is_some() != c.head_tests.is_some() {
                        return invalid("transplant and head_tests are recorded together");
                    }
                    if c.head_tests.is_some() == c.runs.head_tests.is_empty() {
                        return invalid(
                            "runs.head_tests lists runs exactly when head_tests is present",
                        );
                    }
                }
            }
        }
        Ok(Self {
            schema: f.schema,
            task_id: f.task_id,
            attempt_id: f.attempt_id,
            agent_id: f.agent_id,
            base_snapshot: f.base_snapshot,
            head_snapshot: f.head_snapshot,
            environment_hash: f.environment_hash,
            env_changed: f.env_changed,
            criteria: f.criteria,
            checks: f.checks,
            unproven: f.unproven,
            verdict: f.verdict,
            reproduce: f.reproduce,
            verifier: f.verifier,
            created_at: f.created_at,
            executor: f.executor,
            config: f.config,
            test_changes: f.test_changes,
            ci: f.ci,
            uncovered: f.uncovered,
            skipped: f.skipped,
            provenance: f.provenance,
            levels: f.levels,
        })
    }
}

/// The rules only a v3 receipt has: `provenance` and `levels` are present,
/// a native receipt names its host profile and runs under that profile's
/// executor id, a sandboxed one names none, and no skipped check has a
/// level.
fn check_v3(f: &ReceiptFields) -> Result<(), ParseError> {
    let invalid = |reason: &str| Err(ParseError::invalid("receipt", reason));
    let Some(provenance) = &f.provenance else {
        return invalid("an aver.receipt/v3 carries provenance");
    };
    let Some(levels) = &f.levels else {
        return invalid("an aver.receipt/v3 carries levels");
    };
    match (provenance.mode, &provenance.host_profile) {
        (ProvenanceMode::Sandbox, Some(_)) => {
            return invalid("a sandboxed receipt names no host profile")
        }
        (ProvenanceMode::Local | ProvenanceMode::Ci, None) => {
            return invalid("a native receipt names its host profile")
        }
        (ProvenanceMode::Local | ProvenanceMode::Ci, Some(profile)) => {
            if f.executor.as_deref() != Some(profile.executor().as_str()) {
                return invalid(
                    "a native receipt's executor is native-v0. and the first 16 hex digits of its host profile",
                );
            }
        }
        (ProvenanceMode::Sandbox, None) => {}
    }
    if let Some(skipped) = &f.skipped {
        if skipped.iter().any(|s| levels.contains_key(&s.check)) {
            return invalid("a skipped check has no level");
        }
    }
    Ok(())
}

fn is_executor(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_EXECUTOR_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl ReceiptBody {
    pub const SCHEMA_V1: &'static str = "aver.receipt/v1";
    pub const SCHEMA_V2: &'static str = "aver.receipt/v2";
    pub const SCHEMA_V3: &'static str = "aver.receipt/v3";
}

/// The oldest receipt version; [`ReceiptBody::SCHEMA_V1`],
/// [`ReceiptBody::SCHEMA_V2`] and [`ReceiptBody::SCHEMA_V3`] are the others
/// read.
impl Versioned for ReceiptBody {
    const SCHEMA: &'static str = "aver.receipt/v0";
}

impl SignedBody for ReceiptBody {
    type Kind = kind::Receipt;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptCheck {
    pub id: EntryId,
    pub base: BaseStatus,
    pub head: HeadStatus,
    #[serde(deserialize_with = "nullable")]
    pub cause: Option<Cause>,
    pub proves: Set<CriterionId>,
    pub source: CheckSource,
    pub runs: SideRuns,
    /// Failure excerpt. Repository-derived: deliver as delimited data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Text<MAX_DETAIL_BYTES>>,
    /// v2: how a proving entry fared on the transplant tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transplant: Option<Transplant>,
    /// v2: how a proving entry fared in the head-tests run (the head with
    /// its own versions of the test files); recorded with `transplant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_tests: Option<Transplant>,
}

/// An entry's result on one of the proof runs (§1.9.3): the transplant tree
/// (the base plus the head's new and modified test files), or the
/// head-tests tree (the head with its own versions of the test files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transplant {
    /// On the transplant: passed in some run, so the test passes without
    /// the change. On the head-tests tree: passed and never failed.
    Pass,
    /// Failed in the runs.
    Fail,
    /// No report of the runs has a result for the test.
    Absent,
    /// The runs ended in infrastructure failure.
    Infra,
}

impl SetItem for ReceiptCheck {
    type Key = str;
    fn key(&self) -> &str {
        self.id.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SideRuns {
    pub base: Vec<RunId>,
    pub head: Vec<RunId>,
    /// v2: the transplant runs of a proving entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transplant: Vec<RunId>,
    /// v2: the head-tests runs of a proving entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub head_tests: Vec<RunId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BaseStatus {
    #[serde(rename = "pass")]
    Pass,
    #[serde(rename = "fail")]
    Fail,
    /// Base reruns disagreed.
    #[serde(rename = "flaky")]
    Flaky,
    /// The test did not exist on base.
    #[serde(rename = "n/a")]
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadStatus {
    Pass,
    Fail,
    Flaky,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Cause {
    New,
    Preexisting,
    Flaky,
    Env,
    /// v2: the base's copy of an existing test (or a frozen harness file)
    /// fails at head, and the head changed it so that its own version passes
    /// (§1.9.3). Not a regression and not a pass: a human reviews the change.
    Changed,
}

impl Cause {
    /// The wire form, e.g. `NEW`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::Preexisting => "PREEXISTING",
            Self::Flaky => "FLAKY",
            Self::Env => "ENV",
            Self::Changed => "CHANGED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Pass,
    FailOwn,
    BlockedNotOwn,
    Unproven,
    /// v2: it would be `PASS`, but the head changed test expectations or
    /// harness (`CHANGED` entries) that a human has to review.
    Review,
}

impl Verdict {
    /// The wire form, e.g. `FAIL_OWN`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::FailOwn => "FAIL_OWN",
            Self::BlockedNotOwn => "BLOCKED_NOT_OWN",
            Self::Unproven => "UNPROVEN",
            Self::Review => "REVIEW",
        }
    }
}

/// Who defined the check: the project config or the task author.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckSource {
    Env,
    Task,
}
