//! The built test files of a run (ARCHITECTURE.md §1.11, `run_built`): the
//! content hash of each file a run's build left in its directory that
//! matches the built test patterns. Only the executor writes it, beside the
//! run it describes; verification compares the base's with the judged
//! head's (`built::<path>` entries, §1.9.3). A run without a row was not
//! looked at (a run of a runner that does not look, or one stored before
//! the table), which is not the same as a run that built nothing.

use std::collections::BTreeMap;

use aver_core::{BlobId, RunId};
use rusqlite::{params, OptionalExtension};

use crate::store::{Store, StoreError, Tx};

pub(crate) fn insert(
    tx: &Tx<'_>,
    run: &RunId,
    built: &BTreeMap<String, BlobId>,
) -> Result<(), StoreError> {
    let files: BTreeMap<&str, String> = built
        .iter()
        .map(|(path, blob)| (path.as_str(), blob.to_string()))
        .collect();
    let files = serde_json::to_string(&files).expect("a string map serializes");
    tx.sql.execute(
        "INSERT OR IGNORE INTO run_built (run, files) VALUES (?1, ?2)",
        params![run.to_string(), files],
    )?;
    Ok(())
}

/// The built test files of run `run`, or `None` when nobody looked.
pub fn of(store: &Store, run: &RunId) -> Result<Option<BTreeMap<String, BlobId>>, StoreError> {
    let found: Option<String> = store.read(|conn| {
        Ok(conn
            .query_row(
                "SELECT files FROM run_built WHERE run = ?1",
                params![run.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    })?;
    let Some(text) = found else {
        return Ok(None);
    };
    let corrupt = |reason: String| StoreError::Corrupt {
        id: format!("run_built entry for {run}"),
        reason,
    };
    let files: BTreeMap<String, String> =
        serde_json::from_str(&text).map_err(|e| corrupt(e.to_string()))?;
    files
        .into_iter()
        .map(|(path, blob)| {
            let blob = blob
                .parse()
                .map_err(|e: aver_core::ParseError| corrupt(e.to_string()))?;
            Ok((path, blob))
        })
        .collect::<Result<_, _>>()
        .map(Some)
}
