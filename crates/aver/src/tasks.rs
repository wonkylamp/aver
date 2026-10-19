//! Tasks and attempts (ARCHITECTURE.md §1.7, §1.8, §2.6).
//!
//! A task is an immutable object signed by an author holding `task:create`.
//! Claiming a task creates an attempt row and the workspace it edits, in one
//! transaction. Verification appends receipts to the attempt. Deciding
//! ([`Tasks::decide`]) freezes the attempts into [`AttemptRecord`]s and
//! chains the decision onto the ledger, in one transaction.

use std::sync::Arc;

use aver_core::id::kind;
use aver_core::{
    AgentId, AttemptId, AttemptOutcome, AttemptRecord, Cause, LedgerAttempt, LedgerKind, Receipt,
    ReceiptId, Schema, SigningKey, SnapshotId, Task, TaskId, Timestamp, WorkspaceId,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::agents::{self, AgentError, Capability};
use crate::ledger::{self, Entry, Scope};
use crate::store::{Store, StoreError, Tx};
use crate::workspace::{WorkspaceError, Workspaces};

/// Ledger `lessons` listed for a rejected attempt, at most.
const MAX_LESSONS: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Invalid(String),
    #[error("attempt {id} is {state}")]
    AttemptClosed { id: AttemptId, state: AttemptState },
    #[error("{0}")]
    StaleReceipt(String),
    #[error("{0}")]
    AlreadyDecided(String),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for TaskError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e.into())
    }
}

impl From<AgentError> for TaskError {
    fn from(e: AgentError) -> Self {
        match e {
            AgentError::Store(e) => Self::Store(e),
            other => Self::Invalid(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptState {
    Open,
    Accepted,
    Rejected,
    Abandoned,
}

impl AttemptState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Abandoned => "abandoned",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Open, Self::Accepted, Self::Rejected, Self::Abandoned]
            .into_iter()
            .find(|state| state.as_str() == s)
    }
}

impl std::fmt::Display for AttemptState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The live attempt row (§1.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Attempt {
    pub id: AttemptId,
    pub task: TaskId,
    pub agent: AgentId,
    pub workspace: WorkspaceId,
    pub base: SnapshotId,
    pub state: AttemptState,
    /// In creation order.
    pub receipts: Vec<ReceiptId>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Claim {
    pub attempt: AttemptId,
    pub workspace: WorkspaceId,
    pub base: SnapshotId,
    /// The new workspace's version, the `if_version` of its first edit.
    pub version: u64,
}

/// Which attempt of a task won, and why (`aver accept`, §2.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub winner: AttemptId,
    /// The ranking the choice came from, best first.
    #[serde(default)]
    pub ranking: Vec<aver_core::rank::Ranked>,
    /// Why, when the ranking alone does not say.
    #[serde(default)]
    pub rationale: Option<String>,
    /// Extra lessons (an attempt's give-up reason, for example).
    #[serde(default)]
    pub lessons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Decided {
    pub task: TaskId,
    pub winner: AttemptId,
    /// The ledger `accept` entry; `None` when the task was already decided.
    pub ledger: Option<aver_core::LedgerId>,
    /// Every attempt the decision closed, with its new state.
    pub attempts: Vec<(AttemptId, AttemptState)>,
    pub already: bool,
}

pub struct Tasks {
    store: Arc<Store>,
    workspaces: Arc<Workspaces>,
    server_key: Arc<SigningKey>,
}

impl Tasks {
    pub fn new(
        store: Arc<Store>,
        workspaces: Arc<Workspaces>,
        server_key: Arc<SigningKey>,
    ) -> Self {
        Self {
            store,
            workspaces,
            server_key,
        }
    }

    /// Stores a task after checking that its author is a registered agent
    /// holding `task:create` whose key made the signature, that its base
    /// exists, and that its checks prove only its own criteria and do not
    /// reuse a check id of the base environment. Idempotent.
    pub fn create(&self, task: &Task) -> Result<TaskId, TaskError> {
        let body = &task.body;
        let author = agents::get(&self.store, &body.author)?
            .filter(|a| !a.is_revoked())
            .ok_or_else(|| {
                TaskError::Forbidden(format!("author {} is not a registered agent", body.author))
            })?;
        if !author.can(Capability::TaskCreate) {
            return Err(TaskError::Forbidden(format!(
                "author {} lacks capability task:create",
                body.author
            )));
        }
        task.verify(&author.public_key)
            .map_err(|e| TaskError::Invalid(format!("task signature: {e}")))?;
        let base = self.workspaces.get_snapshot(&body.base)?;
        let base_env: aver_core::Environment = self.store.get(&crate::store::pinned_env(&base)?)?;
        let reserved = crate::verify::PROTECTED_CHECK;
        if !body.protected_paths.is_empty()
            && (base_env.checks.get(reserved).is_some()
                || body.checks.iter().any(|c| c.id.as_str() == reserved))
        {
            return Err(TaskError::Invalid(format!(
                "check id {reserved:?} is reserved for protected-path entries, but the task or its base environment defines it"
            )));
        }
        let judging = crate::verify::JUDGING_CHECK;
        if base_env.checks.get(judging).is_some()
            || body.checks.iter().any(|c| c.id.as_str() == judging)
        {
            return Err(TaskError::Invalid(format!(
                "check id {judging:?} is reserved for judging-file entries, but the task or its base environment defines it"
            )));
        }
        for check in body.checks.iter() {
            if base_env.checks.get(check.id.as_str()).is_some() {
                return Err(TaskError::Invalid(format!(
                    "task check {} collides with a check of the base environment",
                    check.id
                )));
            }
            if let Some(unknown) = check.proves.iter().find(|c| !body.criteria.contains(c)) {
                return Err(TaskError::Invalid(format!(
                    "task check {} proves {unknown}, which is not a criterion of the task",
                    check.id
                )));
            }
        }
        if let Some(target) = &body.target {
            if target.as_str().starts_with("ledger/") {
                return Err(TaskError::Invalid(
                    "a task cannot target a ledger/ ref".into(),
                ));
            }
            // An accepted attempt moves the target, so naming one delegates
            // ref:write to every claimer; only an author holding it may.
            if !author.can(Capability::RefWrite) {
                return Err(TaskError::Forbidden(format!(
                    "author {} lacks capability ref:write, which a task target needs",
                    body.author
                )));
            }
        }
        Ok(self.store.put(task)?)
    }

    pub fn get(&self, id: &TaskId) -> Result<Task, TaskError> {
        self.store.get(id).map_err(|e| match e {
            StoreError::NotFound(_) => TaskError::NotFound(format!("task {id} not found")),
            other => other.into(),
        })
    }

    /// The snapshot a claim without `base` starts from: the task's target
    /// ref if it exists, else the task's base.
    fn default_base(&self, task: &Task) -> Result<SnapshotId, TaskError> {
        let target = match &task.body.target {
            Some(name) => self
                .store
                .get_ref::<kind::Snapshot>(name)?
                .map(|r| r.target),
            None => None,
        };
        Ok(target.unwrap_or(task.body.base))
    }

    /// Creates an attempt by `caller` and its workspace, on `base` if given,
    /// else on the task's target ref if it exists, else on the task's base.
    pub fn claim(
        &self,
        caller: &AgentId,
        task_id: &TaskId,
        base: Option<SnapshotId>,
    ) -> Result<Claim, TaskError> {
        let task = self.get(task_id)?;
        let base = match base {
            Some(base) => base,
            None => self.default_base(&task)?,
        };
        let id = AttemptId::generate();
        self.store.tx(|tx| {
            let ws = self.workspaces.open_in(tx, caller, &base, Some(id))?;
            tx.sql.execute(
                "INSERT INTO attempts (id, task, agent, workspace, base, state, receipts, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'open', '[]', ?6)",
                params![
                    id.to_string(),
                    task_id.to_string(),
                    caller.to_string(),
                    ws.id.to_string(),
                    base.to_string(),
                    ws.created_at.to_string(),
                ],
            )?;
            Ok(Claim {
                attempt: id,
                workspace: ws.id,
                base,
                version: ws.version,
            })
        })
    }

    /// The attempt, if `caller` owns it.
    pub fn attempt(&self, caller: &AgentId, id: &AttemptId) -> Result<Attempt, TaskError> {
        self.attempt_for(caller, id, false)
    }

    /// The attempt, if `caller` owns it or `overseer` (a caller holding
    /// `task:decide`) is set.
    pub fn attempt_for(
        &self,
        caller: &AgentId,
        id: &AttemptId,
        overseer: bool,
    ) -> Result<Attempt, TaskError> {
        let attempt = self
            .store
            .read(|conn| load_attempt(conn, id))?
            .ok_or_else(|| attempt_not_found(id))?;
        if !overseer {
            check_attempt_owner(&attempt, caller)?;
        }
        Ok(attempt)
    }

    /// Every attempt at `task`, oldest first.
    pub fn attempts_of(&self, task: &TaskId) -> Result<Vec<Attempt>, TaskError> {
        Ok(self.store.read(|conn| {
            let ids: Vec<String> = conn
                .prepare("SELECT id FROM attempts WHERE task = ?1 ORDER BY created_at, id")?
                .query_map([task.to_string()], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                let id: AttemptId = id.parse().map_err(|e| StoreError::Corrupt {
                    id: id.clone(),
                    reason: format!("{e}"),
                })?;
                out.extend(load_attempt(conn, &id)?);
            }
            Ok(out)
        })?)
    }

    /// Marks the open attempts in `ids` `abandoned`: a rerun replaced the
    /// run they belong to, so their workspaces and leases no longer speak
    /// for the task (§2.11). Their receipts stay; a closed attempt is left
    /// as it is, so running it again is a no-op.
    pub fn abandon(&self, ids: &[AttemptId]) -> Result<(), TaskError> {
        self.store.tx(|tx| {
            for id in ids {
                if load_attempt(&tx.sql, id)?.is_some_and(|a| a.state == AttemptState::Open) {
                    set_attempt_state(tx, id, AttemptState::Abandoned)?;
                }
            }
            Ok(())
        })
    }

    /// Records the winner among a task's attempts (§2.6). In one
    /// transaction: every open attempt with a receipt is frozen into an
    /// [`AttemptRecord`] against its latest receipt (the winner `accepted`,
    /// the others `rejected`), open attempts without one become
    /// `abandoned`, and one ledger `accept` entry in scope `task:<id>` lists
    /// them all. No ref moves. Deciding again with the same winner returns
    /// the earlier decision; a different winner fails `ALREADY_DECIDED`.
    pub fn decide(
        &self,
        caller: &AgentId,
        task_id: &TaskId,
        decision: &Decision,
    ) -> Result<Decided, TaskError> {
        let task = self.get(task_id)?;
        let attempts = self.attempts_of(task_id)?;
        let winner = attempts
            .iter()
            .find(|a| a.id == decision.winner)
            .ok_or_else(|| {
                TaskError::Invalid(format!(
                    "attempt {} is not an attempt at task {task_id}",
                    decision.winner
                ))
            })?
            .clone();
        if let Some(accepted) = attempts.iter().find(|a| a.state == AttemptState::Accepted) {
            return if accepted.id == winner.id {
                Ok(Decided {
                    task: *task_id,
                    winner: winner.id,
                    ledger: None,
                    attempts: attempts.iter().map(|a| (a.id, a.state)).collect(),
                    already: true,
                })
            } else {
                Err(TaskError::AlreadyDecided(format!(
                    "task {task_id} was already decided for attempt {}",
                    accepted.id
                )))
            };
        }
        check_attempt_open(&winner)?;
        let winner_receipt = *winner.receipts.last().ok_or_else(|| {
            TaskError::StaleReceipt(format!(
                "attempt {} has no receipt; verify it first",
                winner.id
            ))
        })?;
        let head = self.workspaces.snapshot(&winner.agent, &winner.workspace)?;
        let receipt: Receipt = self.store.get(&winner_receipt)?;
        if receipt.body.head_snapshot != head || receipt.body.base_snapshot != winner.base {
            return Err(TaskError::StaleReceipt(format!(
                "the latest receipt of attempt {} verified {} against {}, but the attempt is at {head} on {}; verify again",
                winner.id, receipt.body.head_snapshot, receipt.body.base_snapshot, winner.base
            )));
        }
        let mut latest: Vec<(Attempt, Option<(ReceiptId, Receipt)>)> = Vec::new();
        for attempt in attempts
            .into_iter()
            .filter(|a| a.state == AttemptState::Open)
        {
            let receipt = match attempt.receipts.last() {
                Some(id) => Some((*id, self.store.get::<Receipt>(id)?)),
                None => None,
            };
            latest.push((attempt, receipt));
        }
        self.store.tx(|tx| {
            let mut states = Vec::new();
            let mut listed = Vec::new();
            let mut lessons_out = Vec::new();
            for (attempt, receipt) in &latest {
                let current = load_attempt(&tx.sql, &attempt.id)?
                    .ok_or_else(|| attempt_not_found(&attempt.id))?;
                if current.receipts.last() != attempt.receipts.last() {
                    return Err(TaskError::StaleReceipt(format!(
                        "attempt {} got a new receipt during decide; decide again",
                        attempt.id
                    )));
                }
                check_attempt_open(&current)?;
                let state = match receipt {
                    None => AttemptState::Abandoned,
                    Some((receipt_id, receipt)) => {
                        let won = attempt.id == winner.id;
                        tx.put(&AttemptRecord {
                            schema: Schema::new(),
                            attempt: attempt.id,
                            task: *task_id,
                            agent: attempt.agent,
                            base: attempt.base,
                            head: receipt.body.head_snapshot,
                            receipt: *receipt_id,
                            outcome: if won {
                                AttemptOutcome::Accepted
                            } else {
                                AttemptOutcome::Rejected
                            },
                            submitted_at: Timestamp::now(),
                        })?;
                        listed.push(LedgerAttempt {
                            attempt: attempt.id,
                            receipt: *receipt_id,
                            verdict: receipt.body.verdict.as_str().to_owned(),
                        });
                        if won {
                            AttemptState::Accepted
                        } else {
                            lessons_out.extend(
                                lessons(receipt)
                                    .into_iter()
                                    .map(|l| format!("{}: {l}", attempt.id)),
                            );
                            AttemptState::Rejected
                        }
                    }
                };
                set_attempt_state(tx, &attempt.id, state)?;
                states.push((attempt.id, state));
            }
            lessons_out.extend(decision.lessons.iter().cloned());
            lessons_out.truncate(MAX_LESSONS);
            let rationale = match &decision.rationale {
                Some(text) => text.clone(),
                None => decision
                    .ranking
                    .iter()
                    .map(|r| format!("{}. {}: {}", r.place, r.attempt, r.why))
                    .collect::<Vec<_>>()
                    .join("; "),
            };
            let ledger = ledger::append(
                tx,
                &self.server_key,
                Entry {
                    scope: Scope::Task(*task_id),
                    kind: LedgerKind::Accept,
                    task: Some(*task_id),
                    attempts: listed,
                    winner: Some(winner.id),
                    base: Some(winner.base),
                    head: Some(head),
                    summary: format!(
                        "Chose attempt {} of {} for {:?}",
                        winner.id,
                        states.len(),
                        task.body.title
                    ),
                    rationale,
                    lessons: lessons_out,
                    agent: *caller,
                },
            )?;
            Ok(Decided {
                task: *task_id,
                winner: winner.id,
                ledger: Some(ledger),
                attempts: states,
                already: false,
            })
        })
    }
}

/// What a rejected receipt says failed: its causes and unproven criteria.
fn lessons(receipt: &Receipt) -> Vec<String> {
    let causes = receipt.body.checks.iter().filter_map(|c| {
        let cause = match c.cause? {
            Cause::Preexisting => return None,
            other => other.as_str(),
        };
        Some(format!("{cause}: {}", c.id))
    });
    let unproven = receipt
        .body
        .unproven
        .iter()
        .map(|c| format!("unproven: {c}"));
    causes.chain(unproven).take(MAX_LESSONS).collect()
}

fn attempt_not_found(id: &AttemptId) -> TaskError {
    TaskError::NotFound(format!("attempt {id} not found"))
}

pub(crate) fn check_attempt_owner(attempt: &Attempt, caller: &AgentId) -> Result<(), TaskError> {
    if attempt.agent == *caller {
        Ok(())
    } else {
        Err(TaskError::Forbidden(format!(
            "attempt {} belongs to {}",
            attempt.id, attempt.agent
        )))
    }
}

pub(crate) fn check_attempt_open(attempt: &Attempt) -> Result<(), TaskError> {
    match attempt.state {
        AttemptState::Open => Ok(()),
        state => Err(TaskError::AttemptClosed {
            id: attempt.id,
            state,
        }),
    }
}

pub(crate) fn load_attempt(
    conn: &Connection,
    id: &AttemptId,
) -> Result<Option<Attempt>, StoreError> {
    type Raw = (String, String, String, String, String, String, String);
    let row: Option<Raw> = conn
        .query_row(
            "SELECT task, agent, workspace, base, state, receipts, created_at
             FROM attempts WHERE id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((task, agent, workspace, base, state, receipts, created_at)) = row else {
        return Ok(None);
    };
    let corrupt = |reason: String| StoreError::Corrupt {
        id: id.to_string(),
        reason,
    };
    let bad = |e: aver_core::ParseError| corrupt(e.to_string());
    Ok(Some(Attempt {
        id: *id,
        task: task.parse().map_err(bad)?,
        agent: agent.parse().map_err(bad)?,
        workspace: workspace.parse().map_err(bad)?,
        base: base.parse().map_err(bad)?,
        state: AttemptState::parse(&state)
            .ok_or_else(|| corrupt(format!("unknown state {state:?}")))?,
        receipts: serde_json::from_str(&receipts).map_err(|e| corrupt(e.to_string()))?,
        created_at: created_at.parse().map_err(bad)?,
    }))
}

/// Appends `receipt` to the attempt's receipt list.
pub(crate) fn push_receipt(
    tx: &Tx<'_>,
    id: &AttemptId,
    receipt: &ReceiptId,
) -> Result<(), StoreError> {
    tx.sql.execute(
        "UPDATE attempts SET receipts = json_insert(receipts, '$[#]', ?2) WHERE id = ?1",
        params![id.to_string(), receipt.to_string()],
    )?;
    Ok(())
}

fn set_attempt_state(tx: &Tx<'_>, id: &AttemptId, state: AttemptState) -> Result<(), StoreError> {
    tx.sql.execute(
        "UPDATE attempts SET state = ?2 WHERE id = ?1",
        params![id.to_string(), state.as_str()],
    )?;
    Ok(())
}
