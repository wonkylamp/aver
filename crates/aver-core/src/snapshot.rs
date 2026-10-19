//! Snapshot (ARCHITECTURE.md §1.5) and Transition (§1.6).
//!
//! A snapshot is state only: a root tree, and in v0 the environment pinned
//! with it. How it came to exist (parents, author, message, time) lives in
//! signed transitions.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::id::{kind, AgentId, AttemptId, EnvId, SnapshotId, TreeId, WorkspaceId};
use crate::object::{sealed, Object, Schema, Versioned};
use crate::scalar::{GitSha, Timestamp};
use crate::sig::{Signed, SignedBody};
use crate::ParseError;

/// The snapshot schema versions this crate reads. v0 pins an environment
/// beside the tree; v1 is the tree alone, because the repository's contract
/// (`aver.toml` in that tree) says how to test it. A snapshot carries v0
/// exactly when it has an environment, so every v0 snapshot keeps its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotSchema {
    #[serde(rename = "aver.snapshot/v0")]
    V0,
    #[serde(rename = "aver.snapshot/v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub root: TreeId,
    /// v0 only: the environment pinned with the tree.
    pub env: Option<EnvId>,
}

impl Snapshot {
    pub const SCHEMA_V1: &'static str = "aver.snapshot/v1";

    /// A v0 snapshot: the tree and its pinned environment.
    pub fn new(root: TreeId, env: EnvId) -> Self {
        Self {
            root,
            env: Some(env),
        }
    }

    /// A v1 snapshot: the tree alone.
    pub fn of_tree(root: TreeId) -> Self {
        Self { root, env: None }
    }

    pub fn schema(&self) -> SnapshotSchema {
        match self.env {
            Some(_) => SnapshotSchema::V0,
            None => SnapshotSchema::V1,
        }
    }

    /// The root tree, in every version.
    pub fn root(&self) -> TreeId {
        self.root
    }
}

/// The oldest snapshot version; [`Snapshot::SCHEMA_V1`] is also read.
impl Versioned for Snapshot {
    const SCHEMA: &'static str = "aver.snapshot/v0";
}
impl sealed::Sealed for Snapshot {}
impl Object for Snapshot {
    type Kind = kind::Snapshot;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFields {
    schema: SnapshotSchema,
    root: TreeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    env: Option<EnvId>,
}

impl Serialize for Snapshot {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        SnapshotFields {
            schema: self.schema(),
            root: self.root,
            env: self.env,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let f = SnapshotFields::deserialize(d)?;
        let snapshot = Self {
            root: f.root,
            env: f.env,
        };
        if snapshot.schema() != f.schema {
            return Err(serde::de::Error::custom(ParseError::invalid(
                "snapshot",
                "a snapshot carries aver.snapshot/v0 exactly when it has an env, else v1",
            )));
        }
        Ok(snapshot)
    }
}

/// A transition, signed by the server key.
pub type Transition = Signed<TransitionBody>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionBody {
    pub schema: Schema<TransitionBody>,
    /// Ordered parents; empty for a root import.
    pub from: Vec<SnapshotId>,
    pub to: SnapshotId,
    pub via: Via,
    /// Who requested the transition.
    pub agent: AgentId,
    pub message: String,
    pub at: Timestamp,
}

impl Versioned for TransitionBody {
    const SCHEMA: &'static str = "aver.transition/v0";
}

impl SignedBody for TransitionBody {
    type Kind = kind::Transition;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Via {
    GitImport {
        repo: String,
        commit: GitSha,
    },
    Workspace {
        workspace: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<AttemptId>,
    },
}
