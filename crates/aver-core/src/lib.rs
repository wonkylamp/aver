//! Aver's data model, ARCHITECTURE.md §1: identifiers, canonical JSON,
//! hashing, signatures, every stored object type, and the cause and verdict
//! rules. No I/O and no async; everything here is a pure function of its
//! inputs except [`LiveId::generate`] and [`Timestamp::now`], which read the
//! clock.
//!
//! This crate is the only place ids are computed. Store objects through
//! [`Object::canonical_bytes`] and name them with [`Object::id`],
//! [`BlobId::of_bytes`] or [`AgentId::of_public_key`].

pub mod attempt;
pub mod canonical;
pub mod contract;
pub mod env;
pub mod evidence;
pub mod host;
pub mod id;
pub mod judge;
mod key;
pub mod ledger;
pub mod object;
pub mod params;
pub mod pushback;
pub mod rank;
pub mod receipt;
pub mod rules;
pub mod run;
pub mod scalar;
pub mod scope;
pub mod set;
pub mod sig;
pub mod snapshot;
pub mod tag;
pub mod task;
pub mod tree;

pub use attempt::{AttemptOutcome, AttemptRecord};
pub use contract::{
    AdvisoryKind, Approval, CheckSpec, CiPolicy, Contract, ContractCheck, ContractCheckId, Deps,
    JudgeGlobs, Reading, ReportKind, ReportSpec, ReviewPolicy, TextFormat, Trust, Venue, Workflows,
};
pub use env::{
    CheckDef, EnvSchema, Environment, HostArch, HostOs, HostTarget, Image, Install, Layer,
    LayerKind, Limits, Lockfile, Platform, Report, ReportFormat,
};
pub use evidence::{Blocker, CheckEvidence, SetupStep, Side, SideCount, SideEvidence, SideFault};
pub use host::{HostProfile, HostTool, NATIVE_EXECUTOR_PREFIX};
pub use id::{
    AgentId, AttemptId, AttemptRecordId, BlobId, ContentKind, ContractId, EnvId, Id, LeaseId,
    LedgerId, LiveId, LiveKind, ReceiptId, RunId, SnapshotId, TaskId, TransitionId, TreeId,
    WorkspaceId,
};
pub use ledger::{LedgerAttempt, LedgerBody, LedgerEntry, LedgerKind};
pub use object::{Object, ParseError, Schema, Versioned};
pub use pushback::{Pushback, Sentence};
pub use receipt::{
    BaseStatus, Cause, CheckSource, CiInfo, ConfigSource, HeadStatus, Level, Listed, Network,
    ProofRule, Provenance, ProvenanceMode, Receipt, ReceiptBody, ReceiptCheck, ReceiptSchema,
    SideRuns, SkipReason, Skipped, TestChanges, Transplant, Uid, Verdict,
};
pub use run::{
    CheckRun, DepsState, Ending, HostKey, Outcome, PlaceKey, RunIndex, RunKey, RunKeyId, RunResult,
    TestResult, TestStatus,
};
pub use scalar::{
    CheckId, CriterionId, Digest, EntryId, EnvVarName, GitSha, Name, Path, PathGlob, RefName,
    ReportPath, ShellLine, Subject, TestCheckId, Text, Timestamp,
};
pub use set::{Set, SetItem};
pub use sig::{
    decode_base64url, Signature, SignatureError, Signed, SignedBody, SigningKey, VerifyingKey,
};
pub use snapshot::{Snapshot, SnapshotSchema, Transition, TransitionBody, Via};
pub use task::{Criteria, Criterion, Task, TaskBody, TaskCheckDef, TaskSchema};
pub use tree::{Node, Tree, TreeEntry};

/// Largest blob the store accepts in v0 (§1.2).
pub const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
