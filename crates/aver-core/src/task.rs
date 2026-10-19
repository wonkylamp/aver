//! Task (ARCHITECTURE.md §1.7), signed by its author.

use serde::{Deserialize, Deserializer, Serialize};

use crate::env::Report;
use crate::id::{kind, AgentId, SnapshotId};
use crate::object::Versioned;
use crate::scalar::{CheckId, CriterionId, Path, PathGlob, RefName, Text, Timestamp};
use crate::set::{Set, SetItem};
use crate::sig::{Signed, SignedBody};
use crate::ParseError;

pub const MAX_TITLE_BYTES: usize = 200;
pub const MAX_INTENT_BYTES: usize = 16 * 1024;
/// Most protected path patterns a task may declare.
pub const MAX_PROTECTED_PATHS: usize = 64;

pub type Task = Signed<TaskBody>;

/// The task schema versions this crate reads. v1 adds `protected_paths`; v2
/// allows an empty criteria list, with or without protected paths. A task
/// carries the lowest version that represents it, so every v0 and v1 task
/// keeps its bytes and id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskSchema {
    #[serde(rename = "aver.task/v0")]
    V0,
    #[serde(rename = "aver.task/v1")]
    V1,
    #[serde(rename = "aver.task/v2")]
    V2,
}

impl TaskSchema {
    /// The version a task with these criteria and protected paths must carry.
    pub fn for_task(criteria: &Criteria, protected_paths: &Set<PathGlob>) -> Self {
        if criteria.is_empty() {
            Self::V2
        } else if protected_paths.is_empty() {
            Self::V0
        } else {
            Self::V1
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "TaskFields")]
pub struct TaskBody {
    pub schema: TaskSchema,
    pub title: Text<MAX_TITLE_BYTES>,
    pub intent: Text<MAX_INTENT_BYTES>,
    /// The snapshot the task was written against.
    pub base: SnapshotId,
    /// The ref an accepted attempt advances.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<RefName>,
    pub criteria: Criteria,
    #[serde(default, skip_serializing_if = "Set::is_empty")]
    pub checks: Set<TaskCheckDef>,
    /// Paths the head may not add, change or delete (v1 and v2).
    #[serde(default, skip_serializing_if = "Set::is_empty")]
    pub protected_paths: Set<PathGlob>,
    pub author: AgentId,
    pub created_at: Timestamp,
}

/// The wire fields of [`TaskBody`], before the version rules are checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskFields {
    schema: TaskSchema,
    title: Text<MAX_TITLE_BYTES>,
    intent: Text<MAX_INTENT_BYTES>,
    base: SnapshotId,
    #[serde(default)]
    target: Option<RefName>,
    criteria: Criteria,
    #[serde(default)]
    checks: Set<TaskCheckDef>,
    #[serde(default)]
    protected_paths: Set<PathGlob>,
    author: AgentId,
    created_at: Timestamp,
}

impl TryFrom<TaskFields> for TaskBody {
    type Error = ParseError;
    fn try_from(f: TaskFields) -> Result<Self, ParseError> {
        if f.schema != TaskSchema::for_task(&f.criteria, &f.protected_paths) {
            return Err(ParseError::invalid(
                "task",
                "a task carries aver.task/v2 exactly when its criteria are empty, else v1 exactly when it has protected_paths, else v0",
            ));
        }
        if f.checks.iter().any(|c| {
            c.report
                .as_ref()
                .is_some_and(|r| r.format != crate::env::ReportFormat::Junit)
        }) {
            return Err(ParseError::invalid(
                "task",
                "a task check's report is JUnit; other formats exist only in aver.env/v2",
            ));
        }
        if f.protected_paths.len() > MAX_PROTECTED_PATHS {
            return Err(ParseError::invalid(
                "task",
                format!("at most {MAX_PROTECTED_PATHS} protected_paths"),
            ));
        }
        Ok(Self {
            schema: f.schema,
            title: f.title,
            intent: f.intent,
            base: f.base,
            target: f.target,
            criteria: f.criteria,
            checks: f.checks,
            protected_paths: f.protected_paths,
            author: f.author,
            created_at: f.created_at,
        })
    }
}

impl TaskBody {
    pub const SCHEMA_V1: &'static str = "aver.task/v1";
    pub const SCHEMA_V2: &'static str = "aver.task/v2";

    /// Whether `path` is covered by one of the task's protected paths.
    pub fn protects(&self, path: &str) -> Option<&PathGlob> {
        self.protected_paths.iter().find(|g| g.covers(path))
    }
}

/// The oldest task version; [`TaskBody::SCHEMA_V1`] and
/// [`TaskBody::SCHEMA_V2`] are the others read.
impl Versioned for TaskBody {
    const SCHEMA: &'static str = "aver.task/v0";
}

impl SignedBody for TaskBody {
    type Kind = kind::Task;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub id: CriterionId,
    pub text: String,
}

/// An ordered list of criteria with unique ids. Empty only in a v2 task and
/// the receipts verifying one: regression-only mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Criteria(Vec<Criterion>);

impl Criteria {
    pub fn new(criteria: Vec<Criterion>) -> Result<Self, ParseError> {
        for (i, c) in criteria.iter().enumerate() {
            if criteria[..i].iter().any(|prev| prev.id == c.id) {
                return Err(ParseError::invalid(
                    "criteria",
                    format!("duplicate id {:?}", c.id),
                ));
            }
        }
        Ok(Self(criteria))
    }

    pub fn as_slice(&self) -> &[Criterion] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Criterion> {
        self.0.iter()
    }

    pub fn contains(&self, id: &CriterionId) -> bool {
        self.0.iter().any(|c| &c.id == id)
    }
}

impl<'de> Deserialize<'de> for Criteria {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(Vec::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// A check defined by the task author: a `CheckDef` plus the criteria it proves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskCheckDef {
    pub id: CheckId,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
    pub proves: Set<CriterionId>,
}

impl SetItem for TaskCheckDef {
    type Key = str;
    fn key(&self) -> &str {
        self.id.as_str()
    }
}
