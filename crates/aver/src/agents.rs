//! Agent registry (ARCHITECTURE.md §1.11 `agents`, §1.12.5).
//!
//! An agent is identified by its Ed25519 public key (`agent_…` is the key's
//! hash) and carries a set of capabilities. `admin` implies every other
//! capability. Rows are never deleted: revocation stamps `revoked_at`, and a
//! revoked agent fails authentication from then on.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use aver_core::{AgentId, Timestamp, VerifyingKey};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("agent {0} is already registered")]
    Exists(AgentId),
    #[error("agent {0} is not registered")]
    NotFound(AgentId),
    #[error("invalid agent name {0:?}: use 1-64 characters from [A-Za-z0-9._-]")]
    InvalidName(String),
    #[error("unknown capability {0:?}")]
    UnknownCapability(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for AgentError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e.into())
    }
}

/// A v0 capability string (§1.12.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "admin")]
    Admin,
    #[serde(rename = "agent:register")]
    AgentRegister,
    #[serde(rename = "snapshot:import")]
    SnapshotImport,
    #[serde(rename = "snapshot:export")]
    SnapshotExport,
    #[serde(rename = "workspace:write")]
    WorkspaceWrite,
    #[serde(rename = "exec:run")]
    ExecRun,
    #[serde(rename = "verify:run")]
    VerifyRun,
    #[serde(rename = "task:create")]
    TaskCreate,
    #[serde(rename = "task:claim")]
    TaskClaim,
    #[serde(rename = "ref:write")]
    RefWrite,
    #[serde(rename = "workspace:materialize")]
    WorkspaceMaterialize,
    #[serde(rename = "task:decide")]
    TaskDecide,
    #[serde(rename = "lease:write")]
    LeaseWrite,
}

impl Capability {
    pub const ALL: [Capability; 13] = [
        Self::Admin,
        Self::AgentRegister,
        Self::SnapshotImport,
        Self::SnapshotExport,
        Self::WorkspaceWrite,
        Self::ExecRun,
        Self::VerifyRun,
        Self::TaskCreate,
        Self::TaskClaim,
        Self::RefWrite,
        Self::WorkspaceMaterialize,
        Self::TaskDecide,
        Self::LeaseWrite,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::AgentRegister => "agent:register",
            Self::SnapshotImport => "snapshot:import",
            Self::SnapshotExport => "snapshot:export",
            Self::WorkspaceWrite => "workspace:write",
            Self::ExecRun => "exec:run",
            Self::VerifyRun => "verify:run",
            Self::TaskCreate => "task:create",
            Self::TaskClaim => "task:claim",
            Self::RefWrite => "ref:write",
            Self::WorkspaceMaterialize => "workspace:materialize",
            Self::TaskDecide => "task:decide",
            Self::LeaseWrite => "lease:write",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Capability {
    type Err = AgentError;
    fn from_str(s: &str) -> Result<Self, AgentError> {
        Self::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| AgentError::UnknownCapability(s.to_owned()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Agent,
    /// The home's own key, which signs transitions, receipts and ledger entries.
    Server,
}

impl AgentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Server => "server",
        }
    }
}

/// A human-readable agent name, also used as its key file name:
/// 1-64 characters from `[A-Za-z0-9._-]`, not starting with `.`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AgentName(String);

impl AgentName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AgentName {
    type Error = AgentError;
    fn try_from(s: String) -> Result<Self, AgentError> {
        let ok = (1..=64).contains(&s.len())
            && !s.starts_with('.')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if ok {
            Ok(Self(s))
        } else {
            Err(AgentError::InvalidName(s))
        }
    }
}

impl From<AgentName> for String {
    fn from(name: AgentName) -> String {
        name.0
    }
}

impl FromStr for AgentName {
    type Err = AgentError;
    fn from_str(s: &str) -> Result<Self, AgentError> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Agent {
    pub id: AgentId,
    #[serde(serialize_with = "serialize_public_key")]
    pub public_key: VerifyingKey,
    pub name: AgentName,
    pub kind: AgentKind,
    pub capabilities: BTreeSet<Capability>,
    pub issued_by: Option<AgentId>,
    pub issued_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
}

fn serialize_public_key<S: serde::Serializer>(key: &VerifyingKey, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&data_encoding::BASE64URL_NOPAD.encode(key.as_bytes()))
}

impl Agent {
    /// Whether the agent holds `capability`, directly or through `admin`.
    pub fn can(&self, capability: Capability) -> bool {
        self.capabilities.contains(&Capability::Admin) || self.capabilities.contains(&capability)
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }
}

pub struct NewAgent {
    pub public_key: VerifyingKey,
    pub name: AgentName,
    pub kind: AgentKind,
    pub capabilities: BTreeSet<Capability>,
    /// `None` only for the bootstrap admin.
    pub issued_by: Option<AgentId>,
}

pub fn register(store: &Store, new: NewAgent) -> Result<Agent, AgentError> {
    let agent = Agent {
        id: AgentId::of_public_key(&new.public_key),
        public_key: new.public_key,
        name: new.name,
        kind: new.kind,
        capabilities: new.capabilities,
        issued_by: new.issued_by,
        issued_at: Timestamp::now(),
        revoked_at: None,
    };
    store.tx(|tx| {
        let inserted = tx.sql.execute(
            "INSERT INTO agents (id, public_key, name, kind, capabilities, issued_by, issued_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (id) DO NOTHING",
            params![
                agent.id.to_string(),
                agent.public_key.as_bytes(),
                agent.name.as_str(),
                agent.kind.as_str(),
                serde_json::to_string(&agent.capabilities).expect("capabilities serialize"),
                agent.issued_by.map(|id| id.to_string()),
                agent.issued_at.to_string(),
            ],
        )?;
        if inserted == 0 {
            return Err(AgentError::Exists(agent.id));
        }
        Ok(())
    })?;
    Ok(agent)
}

pub fn get(store: &Store, id: &AgentId) -> Result<Option<Agent>, AgentError> {
    let row = store.read(|conn| {
        Ok(conn
            .query_row(
                "SELECT public_key, name, kind, capabilities, issued_by, issued_at, revoked_at
                 FROM agents WHERE id = ?1",
                [id.to_string()],
                |r| {
                    Ok(Row {
                        public_key: r.get(0)?,
                        name: r.get(1)?,
                        kind: r.get(2)?,
                        capabilities: r.get(3)?,
                        issued_by: r.get(4)?,
                        issued_at: r.get(5)?,
                        revoked_at: r.get(6)?,
                    })
                },
            )
            .optional()?)
    })?;
    row.map(|row| row.into_agent(*id)).transpose()
}

/// Stamps `revoked_at`. Revoking an already revoked agent keeps the first time.
pub fn revoke(store: &Store, id: &AgentId) -> Result<(), AgentError> {
    let changed = store.tx(|tx| {
        tx.sql
            .execute(
                "UPDATE agents SET revoked_at = COALESCE(revoked_at, ?2) WHERE id = ?1",
                params![id.to_string(), Timestamp::now().to_string()],
            )
            .map_err(StoreError::from)
    })?;
    if changed == 0 {
        return Err(AgentError::NotFound(*id));
    }
    Ok(())
}

struct Row {
    public_key: Vec<u8>,
    name: String,
    kind: String,
    capabilities: String,
    issued_by: Option<String>,
    issued_at: String,
    revoked_at: Option<String>,
}

impl Row {
    fn into_agent(self, id: AgentId) -> Result<Agent, AgentError> {
        let corrupt = |reason: String| {
            AgentError::Store(StoreError::Corrupt {
                id: id.to_string(),
                reason,
            })
        };
        let key: [u8; 32] = self
            .public_key
            .try_into()
            .map_err(|_| corrupt("public key is not 32 bytes".into()))?;
        let public_key = VerifyingKey::from_bytes(&key).map_err(|e| corrupt(e.to_string()))?;
        let kind = match self.kind.as_str() {
            "agent" => AgentKind::Agent,
            "server" => AgentKind::Server,
            other => return Err(corrupt(format!("unknown kind {other:?}"))),
        };
        let timestamp = |s: &str| s.parse::<Timestamp>().map_err(|e| corrupt(e.to_string()));
        Ok(Agent {
            id,
            public_key,
            name: self
                .name
                .parse()
                .map_err(|e: AgentError| corrupt(e.to_string()))?,
            kind,
            capabilities: serde_json::from_str(&self.capabilities)
                .map_err(|e| corrupt(e.to_string()))?,
            issued_by: self
                .issued_by
                .map(|s| {
                    s.parse()
                        .map_err(|e: aver_core::ParseError| corrupt(e.to_string()))
                })
                .transpose()?,
            issued_at: timestamp(&self.issued_at)?,
            revoked_at: self.revoked_at.as_deref().map(timestamp).transpose()?,
        })
    }
}
