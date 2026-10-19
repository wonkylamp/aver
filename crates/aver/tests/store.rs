use std::sync::{Arc, Barrier};

use aver::store::{Ref, Store, StoreError};
use aver_core::id::kind;
use aver_core::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn open() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("aver.db")).unwrap();
    (dir, store)
}

fn raw(store: &Store) -> Connection {
    Connection::open(store.path()).unwrap()
}

fn tree_of(names: &[&str]) -> Tree {
    Tree::new(
        Set::new(
            names
                .iter()
                .map(|n| TreeEntry {
                    name: n.parse().unwrap(),
                    node: Node::File(BlobId::of_bytes(n.as_bytes())),
                })
                .collect(),
        )
        .unwrap(),
    )
}

fn snapshot(n: u8) -> Snapshot {
    let env = format!("env_{}", "a".repeat(52)).parse().unwrap();
    Snapshot::new(tree_of(&[&format!("f{n}")]).id(), env)
}

fn trunk() -> RefName {
    "trunk".parse().unwrap()
}

fn object_rows(store: &Store) -> i64 {
    raw(store)
        .query_row("SELECT COUNT(*) FROM objects", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn put_is_idempotent_and_returns_the_content_id() {
    let (_dir, store) = open();
    let id = store.put_blob(b"hello\n").unwrap();
    assert_eq!(id, BlobId::of_bytes(b"hello\n"));
    assert_eq!(store.put_blob(b"hello\n").unwrap(), id);

    let tree = tree_of(&["a", "b"]);
    assert_eq!(store.put(&tree).unwrap(), tree.id());
    assert_eq!(store.put(&tree).unwrap(), tree.id());
    assert_eq!(object_rows(&store), 2);
    assert_eq!(store.get_raw(&id).unwrap(), b"hello\n");
    assert!(store.has(&id).unwrap());
}

#[test]
fn bodies_over_one_kib_are_compressed_and_read_back_exactly() {
    let (_dir, store) = open();
    let big: Vec<u8> = b"0123456789abcdef".repeat(4096);
    let small = vec![7u8; 1024];
    let big_id = store.put_blob(&big).unwrap();
    let small_id = store.put_blob(&small).unwrap();
    assert_eq!(store.get_raw(&big_id).unwrap(), big);
    assert_eq!(store.get_raw(&small_id).unwrap(), small);

    let stored_len = |id: &BlobId| -> (i64, i64, String) {
        raw(&store)
            .query_row(
                "SELECT size, length(data), kind FROM objects WHERE id = ?1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    };
    let (size, len, kind) = stored_len(&big_id);
    assert_eq!((size, kind.as_str()), (big.len() as i64, "blob"));
    assert!(len < size / 10, "stored {len} bytes for {size}");
    assert_eq!(stored_len(&small_id).1, 1024, "small bodies stay raw");
}

#[test]
fn get_parses_objects_strictly_and_detects_corruption() {
    let (_dir, store) = open();
    let snap = snapshot(1);
    let id = store.put(&snap).unwrap();
    assert_eq!(store.get::<Snapshot>(&id).unwrap(), snap);
    assert_eq!(
        raw(&store)
            .query_row(
                "SELECT kind FROM objects WHERE id = ?1",
                [id.to_string()],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "snap"
    );

    let other = snapshot(2);
    raw(&store)
        .execute(
            "UPDATE objects SET data = ?1, size = ?2 WHERE id = ?3",
            rusqlite::params![
                other.canonical_bytes(),
                other.canonical_bytes().len() as i64,
                id.to_string()
            ],
        )
        .unwrap();
    assert!(matches!(
        store.get::<Snapshot>(&id),
        Err(StoreError::Corrupt { .. })
    ));

    raw(&store)
        .execute(
            "UPDATE objects SET data = ?1, size = ?2 WHERE id = ?3",
            rusqlite::params![b"{\"x\":1}".to_vec(), 7, id.to_string()],
        )
        .unwrap();
    assert!(matches!(
        store.get::<Snapshot>(&id),
        Err(StoreError::Corrupt { .. })
    ));
}

#[test]
fn missing_objects_are_not_found() {
    let (_dir, store) = open();
    let id = snapshot(1).id();
    assert!(!store.has(&id).unwrap());
    assert!(
        matches!(store.get::<Snapshot>(&id), Err(StoreError::NotFound(s)) if s == id.to_string())
    );
    assert!(matches!(
        store.get_raw(&BlobId::of_bytes(b"x")),
        Err(StoreError::NotFound(_))
    ));
}

#[test]
fn oversized_blobs_are_rejected() {
    let (_dir, store) = open();
    let huge = vec![0u8; MAX_BLOB_BYTES + 1];
    assert!(matches!(store.put_blob(&huge), Err(StoreError::BlobTooLarge(n)) if n == huge.len()));
    assert_eq!(object_rows(&store), 0);
}

#[test]
fn a_failed_transaction_writes_nothing() {
    let (_dir, store) = open();
    let tree = tree_of(&["a"]);
    let snap = Snapshot::new(tree.id(), snapshot(1).env.unwrap());

    let result: Result<(), StoreError> = store.tx(|tx| {
        tx.put_blob(b"a")?;
        tx.put(&tree)?;
        let id = tx.put(&snap)?;
        tx.compare_and_set(&trunk(), None, &id)?;
        assert!(tx.has(&id)?, "reads inside the transaction see its writes");
        Err(StoreError::NotFound("forced rollback".into()))
    });
    assert!(result.is_err());
    assert_eq!(object_rows(&store), 0);
    assert_eq!(store.get_ref::<kind::Snapshot>(&trunk()).unwrap(), None);

    store
        .tx(|tx| {
            tx.put_blob(b"a")?;
            tx.put(&tree)?;
            let id = tx.put(&snap)?;
            tx.compare_and_set(&trunk(), None, &id)
        })
        .unwrap();
    assert_eq!(object_rows(&store), 3);
    assert_eq!(
        store.get_ref::<kind::Snapshot>(&trunk()).unwrap(),
        Some(Ref {
            target: snap.id(),
            version: 1
        })
    );
}

#[test]
fn a_panicking_transaction_writes_nothing() {
    let (_dir, store) = open();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = store.tx(|tx| -> Result<(), StoreError> {
            tx.put_blob(b"a")?;
            panic!("boom");
        });
    }));
    assert!(caught.is_err());
    assert_eq!(object_rows(&store), 0);
    store.put_blob(b"b").unwrap();
    assert_eq!(object_rows(&store), 1, "the writer is usable after a panic");
}

#[test]
fn refs_update_only_by_compare_and_set() {
    let (_dir, store) = open();
    let (s1, s2, s3) = (snapshot(1).id(), snapshot(2).id(), snapshot(3).id());

    assert_eq!(store.compare_and_set(&trunk(), None, &s1).unwrap(), 1);
    match store.compare_and_set(&trunk(), None, &s2) {
        Err(StoreError::RefConflict {
            expected, actual, ..
        }) => {
            assert_eq!(expected, None);
            assert_eq!(actual, Some(s1.to_string()));
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert_eq!(store.compare_and_set(&trunk(), Some(&s1), &s2).unwrap(), 2);
    assert!(matches!(
        store.compare_and_set(&trunk(), Some(&s1), &s3),
        Err(StoreError::RefConflict { .. })
    ));
    assert_eq!(
        store.get_ref::<kind::Snapshot>(&trunk()).unwrap(),
        Some(Ref {
            target: s2,
            version: 2
        })
    );
    assert!(matches!(
        store.compare_and_set(&"other".parse().unwrap(), Some(&s1), &s3),
        Err(StoreError::RefConflict { actual: None, .. })
    ));
}

#[test]
fn concurrent_compare_and_set_has_exactly_one_winner() {
    let (_dir, store) = open();
    let store = Arc::new(store);
    let threads = 8;
    let barrier = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|n| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let id = snapshot(n as u8).id();
                barrier.wait();
                store.compare_and_set(&trunk(), None, &id).map(|_| id)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let winners: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(
        store
            .get_ref::<kind::Snapshot>(&trunk())
            .unwrap()
            .unwrap()
            .target,
        *winners[0]
    );
}

#[test]
fn a_ref_read_as_the_wrong_kind_is_corrupt() {
    let (_dir, store) = open();
    store
        .compare_and_set(&trunk(), None, &snapshot(1).id())
        .unwrap();
    assert!(matches!(
        store.get_ref::<kind::Ledger>(&trunk()),
        Err(StoreError::Corrupt { .. })
    ));
}

#[test]
fn reopening_keeps_data_and_does_not_rerun_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("aver.db");
    let id = {
        let store = Store::open(&path).unwrap();
        store.put_blob(b"persisted").unwrap()
    };
    let store = Store::open(&path).unwrap();
    assert_eq!(store.get_raw(&id).unwrap(), b"persisted");

    let conn = raw(&store);
    let versions: Vec<i64> = conn
        .prepare("SELECT version FROM schema_version")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(versions, [1, 2, 3, 4, 5, 6, 7, 8]);
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

#[test]
fn every_state_table_exists() {
    let (_dir, store) = open();
    let conn = raw(&store);
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "agents",
            "attempts",
            "checkouts",
            "diagnostics",
            "git_map",
            "import_memo",
            "leases",
            "materializations",
            "objects",
            "overlay",
            "refs",
            "run_built",
            "run_cache",
            "schema_version",
            "transitions_by_snapshot",
            "workspaces",
        ]
    );
}

#[test]
fn a_version_4_home_with_request_rows_opens_without_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aver.db");
    let id = Store::open(&path).unwrap().put_blob(b"kept").unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE requests (
             agent TEXT NOT NULL, nonce TEXT NOT NULL,
             ts_ms INTEGER NOT NULL, method TEXT NOT NULL, path TEXT NOT NULL,
             body_hash TEXT NOT NULL, signature TEXT NOT NULL,
             PRIMARY KEY (agent, nonce)
         ) WITHOUT ROWID;
         CREATE INDEX requests_by_ts ON requests (ts_ms);
         INSERT INTO requests VALUES ('agent_x', 'n', 1, 'POST', '/v0/x', 'h', 's');
         DELETE FROM schema_version WHERE version = 5;",
    )
    .unwrap();
    version_5_tables(&conn);
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.get_raw(&id).unwrap(), b"kept");
    let left: i64 = raw(&store)
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'requests%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}

/// The tables schema v6 drops, with a row each, as a version 5 home has
/// them.
fn version_5_tables(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE deps_layers (
             deps_key TEXT PRIMARY KEY,
             volume   TEXT NOT NULL,
             state    TEXT NOT NULL CHECK (state IN ('building', 'ready', 'failed')),
             size     INTEGER
         ) WITHOUT ROWID;
         INSERT INTO deps_layers VALUES ('deps_x', 'aver-deps-x', 'ready', 1);
         CREATE TABLE host_deps (
             key          TEXT PRIMARY KEY,
             dir          TEXT NOT NULL,
             source       TEXT NOT NULL CHECK (source IN ('installed', 'adopted')),
             state        TEXT NOT NULL CHECK (state IN ('building', 'ready', 'failed')),
             files        INTEGER,
             bytes        INTEGER,
             error        TEXT,
             created_at   TEXT NOT NULL,
             last_used_at TEXT NOT NULL
         );
         INSERT INTO host_deps VALUES
             ('hdeps_x', '/x', 'installed', 'ready', 1, 1, NULL, 't', 't');
         DELETE FROM schema_version WHERE version >= 6;",
    )
    .unwrap();
}

#[test]
fn a_version_5_home_with_dependency_rows_opens_without_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aver.db");
    let id = Store::open(&path).unwrap().put_blob(b"kept").unwrap();
    version_5_tables(&Connection::open(&path).unwrap());

    let store = Store::open(&path).unwrap();
    assert_eq!(store.get_raw(&id).unwrap(), b"kept");
    let conn = raw(&store);
    let left: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('deps_layers', 'host_deps')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
    let version: i64 = conn
        .query_row("SELECT max(version) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 8);
}

#[test]
fn a_home_with_ledger_entries_from_before_0_4_still_reads_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aver.db");
    let key = SigningKey::from_bytes(&[3; 32]);
    let task: TaskId = format!("task_{}", "a".repeat(52)).parse().unwrap();
    let written: Vec<LedgerId> = {
        let store = Store::open(&path).unwrap();
        [LedgerKind::Attempt, LedgerKind::Reject]
            .into_iter()
            .map(|kind| {
                store
                    .tx(|tx| {
                        aver::ledger::append(
                            tx,
                            &key,
                            aver::ledger::Entry {
                                scope: aver::ledger::Scope::Task(task),
                                kind,
                                task: Some(task),
                                attempts: Vec::new(),
                                winner: None,
                                base: None,
                                head: None,
                                summary: "submitted".into(),
                                rationale: String::new(),
                                lessons: Vec::new(),
                                agent: AgentId::of_public_key(&key.verifying_key()),
                            },
                        )
                    })
                    .unwrap()
            })
            .collect()
    };

    let store = Store::open(&path).unwrap();
    let head = store
        .get_ref::<kind::Ledger>(&format!("ledger/task:{task}").parse().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(head.target, written[1]);
    let reject: LedgerEntry = store.get(&written[1]).unwrap();
    reject.verify(&key.verifying_key()).unwrap();
    assert_eq!(reject.body.kind, LedgerKind::Reject);
    assert_eq!(reject.id(), written[1]);
    assert_eq!(reject.body.prev, Some(written[0]));
    let attempt: LedgerEntry = store.get(&written[0]).unwrap();
    assert_eq!(attempt.body.kind, LedgerKind::Attempt);
    assert_eq!(attempt.id(), written[0]);
}

#[test]
fn a_database_from_a_newer_binary_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aver.db");
    drop(Store::open(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .execute("INSERT INTO schema_version (version) VALUES (99)", [])
        .unwrap();
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::SchemaTooNew {
            found: 99,
            supported: 8
        })
    ));
}
