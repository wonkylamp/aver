//! Ledger appends (ARCHITECTURE.md §1.10).
//!
//! Each scope is a hash chain of server-signed entries whose head lives in
//! the ref `ledger/<scope>`. [`append`] links a new entry to the current head
//! and moves the ref by compare-and-set inside the caller's transaction, so
//! an entry and the change it records commit together, and two writers can
//! never fork a chain. This is the only writer of ledger entries.

use std::fmt;

use aver_core::id::kind;
use aver_core::{
    AgentId, AttemptId, LedgerAttempt, LedgerBody, LedgerEntry, LedgerId, LedgerKind, RefName,
    Schema, SigningKey, SnapshotId, TaskId, Timestamp,
};

use crate::store::{StoreError, Tx};

/// A ledger chain. Its textual form is the entry's `scope` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Changes to a branch ref: `ref:<name>`.
    Ref(RefName),
    /// Imports that move no ref: `imports`.
    Imports,
    /// Attempts at a task without a target ref: `task:<id>`.
    Task(TaskId),
}

impl Scope {
    /// The ref holding the chain's head, `ledger/<scope>`.
    pub fn head_ref(&self) -> RefName {
        format!("ledger/{self}")
            .parse()
            .expect("ref names and content ids are valid ref segments")
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ref(name) => write!(f, "ref:{name}"),
            Self::Imports => f.write_str("imports"),
            Self::Task(id) => write!(f, "task:{id}"),
        }
    }
}

/// The fields of an entry that its writer chooses; `schema`, `prev`, `at`
/// and the signature are filled in by [`append`].
#[derive(Debug, Clone)]
pub struct Entry {
    pub scope: Scope,
    pub kind: LedgerKind,
    pub task: Option<TaskId>,
    pub attempts: Vec<LedgerAttempt>,
    pub winner: Option<AttemptId>,
    pub base: Option<SnapshotId>,
    pub head: Option<SnapshotId>,
    pub summary: String,
    pub rationale: String,
    pub lessons: Vec<String>,
    pub agent: AgentId,
}

/// Signs `entry` with the server key, chains it onto its scope and moves
/// `ledger/<scope>` to it.
pub fn append(tx: &Tx<'_>, server_key: &SigningKey, entry: Entry) -> Result<LedgerId, StoreError> {
    let head_ref = entry.scope.head_ref();
    let prev = tx.get_ref::<kind::Ledger>(&head_ref)?.map(|r| r.target);
    let signed = LedgerEntry::sign(
        LedgerBody {
            schema: Schema::new(),
            scope: entry.scope.to_string(),
            prev,
            kind: entry.kind,
            task: entry.task,
            attempts: entry.attempts,
            winner: entry.winner,
            base: entry.base,
            head: entry.head,
            summary: entry.summary,
            rationale: entry.rationale,
            lessons: entry.lessons,
            agent: entry.agent,
            at: Timestamp::now(),
        },
        server_key,
    );
    let id = tx.put(&signed)?;
    tx.compare_and_set(&head_ref, prev.as_ref(), &id)?;
    Ok(id)
}
