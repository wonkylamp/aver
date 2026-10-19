//! Attempt record (ARCHITECTURE.md §1.8): an attempt frozen on submit.

use serde::{Deserialize, Serialize};

use crate::id::{kind, AgentId, AttemptId, ReceiptId, SnapshotId, TaskId};
use crate::object::{impl_object, Schema};
use crate::scalar::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    pub schema: Schema<AttemptRecord>,
    pub attempt: AttemptId,
    pub task: TaskId,
    pub agent: AgentId,
    pub base: SnapshotId,
    pub head: SnapshotId,
    /// Must have `head_snapshot == head`.
    pub receipt: ReceiptId,
    pub outcome: AttemptOutcome,
    pub submitted_at: Timestamp,
}

impl_object!(AttemptRecord, kind::AttemptRecord, "aver.attempt/v0");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptOutcome {
    Accepted,
    Rejected,
}
