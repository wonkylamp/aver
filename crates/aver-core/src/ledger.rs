//! Ledger entry (ARCHITECTURE.md §1.10): append-only, hash-chained per scope,
//! signed by the server key.

use serde::{Deserialize, Serialize};

use crate::id::{kind, AgentId, AttemptId, LedgerId, ReceiptId, SnapshotId, TaskId};
use crate::object::{nullable, Schema, Versioned};
use crate::scalar::Timestamp;
use crate::sig::{Signed, SignedBody};

pub type LedgerEntry = Signed<LedgerBody>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerBody {
    pub schema: Schema<LedgerBody>,
    /// e.g. `ref:trunk`.
    pub scope: String,
    /// The previous entry in this scope; `null` for the first.
    #[serde(deserialize_with = "nullable")]
    pub prev: Option<LedgerId>,
    pub kind: LedgerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<LedgerAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<AttemptId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<SnapshotId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<SnapshotId>,
    /// What changed.
    pub summary: String,
    /// Why.
    pub rationale: String,
    /// What failed and what it taught.
    pub lessons: Vec<String>,
    pub agent: AgentId,
    pub at: Timestamp,
}

impl Versioned for LedgerBody {
    const SCHEMA: &'static str = "aver.ledger/v0";
}

impl SignedBody for LedgerBody {
    type Kind = kind::Ledger;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LedgerKind {
    Import,
    /// Written only before Aver 0.4, by a submit through the HTTP API; kept
    /// so the entries already stored still parse and hash the same.
    Attempt,
    Accept,
    /// Written only before Aver 0.4, as [`Self::Attempt`].
    Reject,
    Escalate,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerAttempt {
    pub attempt: AttemptId,
    pub receipt: ReceiptId,
    pub verdict: String,
}
