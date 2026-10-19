//! The execution cache index (ARCHITECTURE.md §2.4.4): `run_cache` maps
//! `(snapshot, env, check, executor, index)` to the `RunId` of a `pass` or
//! `fail` run. Only the executor writes it.
//!
//! The check component is the check id when the environment defines that
//! exact check. A check the environment does not define (a task check) is
//! keyed by its id plus its canonical definition, because two tasks may give
//! different checks the same id.

use aver_core::canonical::to_canonical;
use aver_core::{CheckDef, EnvId, Environment, Object, RunId, RunIndex, SnapshotId};
use rusqlite::{params, OptionalExtension};

use crate::store::{Store, StoreError, Tx};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    pub snapshot: SnapshotId,
    pub env: EnvId,
    pub check: String,
    pub executor: String,
    pub index: RunIndex,
}

impl CacheKey {
    pub fn new(
        snapshot: &SnapshotId,
        env: &Environment,
        check: &CheckDef,
        executor: &str,
        index: RunIndex,
    ) -> Self {
        let check = if env.checks.get(check.id.as_str()) == Some(check) {
            check.id.to_string()
        } else {
            let def = serde_json::to_value(check).expect("check definitions serialize");
            let def = to_canonical(&def).expect("check definitions hold no floats");
            format!("{}:{}", check.id, String::from_utf8_lossy(&def))
        };
        Self {
            snapshot: *snapshot,
            env: env.id(),
            check,
            executor: executor.to_owned(),
            index,
        }
    }

    /// Every row whose snapshot is `snapshot`, with its run.
    pub fn rows_of(store: &Store, snapshot: &SnapshotId) -> Result<Vec<(Self, RunId)>, StoreError> {
        let rows: Vec<(String, String, String, u8, String)> = store.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT env, check_id, executor, run_index, run FROM run_cache
                 WHERE snapshot = ?1 ORDER BY env, check_id, executor, run_index",
            )?;
            let rows = stmt
                .query_map(params![snapshot.to_string()], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        rows.into_iter()
            .map(|(env, check, executor, index, run)| {
                let corrupt = |reason: String| StoreError::Corrupt {
                    id: format!("run_cache entry for {check}"),
                    reason,
                };
                let key = Self {
                    snapshot: *snapshot,
                    env: env
                        .parse()
                        .map_err(|e: aver_core::ParseError| corrupt(e.to_string()))?,
                    check: check.clone(),
                    executor,
                    index: RunIndex::try_from(index).map_err(|e| corrupt(e.to_string()))?,
                };
                let run = run
                    .parse()
                    .map_err(|e: aver_core::ParseError| corrupt(e.to_string()))?;
                Ok((key, run))
            })
            .collect()
    }

    pub fn lookup(&self, store: &Store) -> Result<Option<RunId>, StoreError> {
        let found: Option<String> = store.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT run FROM run_cache
                     WHERE snapshot = ?1 AND env = ?2 AND check_id = ?3 AND executor = ?4 AND run_index = ?5",
                    params![
                        self.snapshot.to_string(),
                        self.env.to_string(),
                        self.check,
                        self.executor,
                        self.index.get()
                    ],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        found
            .map(|id| {
                id.parse()
                    .map_err(|e: aver_core::ParseError| StoreError::Corrupt {
                        id: format!("run_cache entry for {}", self.check),
                        reason: e.to_string(),
                    })
            })
            .transpose()
    }

    pub fn insert(&self, tx: &Tx<'_>, run: &RunId) -> Result<(), StoreError> {
        tx.sql.execute(
            "INSERT OR IGNORE INTO run_cache (snapshot, env, check_id, executor, run_index, run)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.snapshot.to_string(),
                self.env.to_string(),
                self.check,
                self.executor,
                self.index.get(),
                run.to_string()
            ],
        )?;
        Ok(())
    }
}
