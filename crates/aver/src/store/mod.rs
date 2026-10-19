//! Object store and state database (ARCHITECTURE.md §2.1, §3, §1.11).
//!
//! One SQLite file in WAL mode holds immutable objects and every mutable index,
//! so a multi-object write plus its ref and index updates commit atomically
//! through [`Store::tx`]. There is one writer connection behind a mutex and a
//! small pool of read-only connections.
//!
//! Objects are keyed by their content id and never updated; `put` is
//! idempotent. [`Store::get`] strictly re-parses the stored bytes and checks
//! they still hash to the requested id. Refs are named pointers updated only
//! by compare-and-set.

use std::path::{Path, PathBuf};

use aver_core::{
    BlobId, ContentKind, EnvId, Id, Object, ParseError, RefName, Snapshot, MAX_BLOB_BYTES,
};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};

const MIGRATIONS: &[&str] = &[
    include_str!("schema_v1.sql"),
    include_str!("schema_v2.sql"),
    include_str!("schema_v3.sql"),
    include_str!("schema_v4.sql"),
    include_str!("schema_v5.sql"),
    include_str!("schema_v6.sql"),
    include_str!("schema_v7.sql"),
    include_str!("schema_v8.sql"),
];
const COMPRESS_OVER_BYTES: usize = 1024;
const ZSTD_LEVEL: i32 = 3;
const MAX_IDLE_READERS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("object {0} not found")]
    NotFound(String),
    #[error("object {id} is corrupt: {reason}")]
    Corrupt { id: String, reason: String },
    #[error("blob of {0} bytes exceeds the {MAX_BLOB_BYTES}-byte limit")]
    BlobTooLarge(usize),
    #[error("ref {name} is at {actual:?}, not the expected {expected:?}")]
    RefConflict {
        name: RefName,
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error("database schema version {found} is newer than this binary supports ({supported})")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("snapshot {0} is aver.snapshot/v1, which pins no environment")]
    NoPinnedEnv(String),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// The environment a v0 snapshot pins. The services still judge by pinned
/// environments, so a v1 snapshot (a tree alone) is refused here.
pub fn pinned_env(snapshot: &Snapshot) -> Result<EnvId> {
    snapshot
        .env
        .ok_or_else(|| StoreError::NoPinnedEnv(snapshot.id().to_string()))
}

/// A ref's current target and version. The version starts at 1 and increments
/// on every successful compare-and-set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ref<K: ContentKind> {
    pub target: Id<K>,
    pub version: u64,
}

pub struct Store {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and applies pending migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut writer = Connection::open(&path)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        configure(&writer)?;
        migrate(&mut writer)?;
        Ok(Self {
            path,
            writer: Mutex::new(writer),
            readers: Mutex::new(Vec::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Makes every committed transaction durable. Commits run with
    /// `synchronous=NORMAL`, so a power loss may drop the last ones from
    /// the WAL; anything written outside the store that names them (a Git
    /// commit's receipt trailer, a pushed branch) waits for this first.
    /// The checkpoint copies the WAL into the database file, and `sync_all`
    /// (`F_FULLFSYNC` on macOS, where plain `fsync` stops at the drive's
    /// cache) flushes both files.
    pub fn barrier(&self) -> Result<()> {
        {
            let conn = self.writer.lock();
            let (busy, _, _): (i64, i64, i64) =
                conn.query_row("PRAGMA wal_checkpoint(FULL)", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
            if busy != 0 {
                // A reader kept the checkpoint from finishing; the WAL
                // itself still holds the commits, so flush it whole.
                tracing::debug!("wal checkpoint was busy; syncing the WAL instead");
            }
        }
        std::fs::File::open(&self.path)?.sync_all()?;
        let mut wal = self.path.clone().into_os_string();
        wal.push("-wal");
        match std::fs::File::open(&wal) {
            Ok(f) => f.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    pub fn put_blob(&self, bytes: &[u8]) -> Result<BlobId> {
        self.tx(|tx| tx.put_blob(bytes))
    }

    pub fn put<T: Object>(&self, object: &T) -> Result<Id<T::Kind>> {
        self.tx(|tx| tx.put(object))
    }

    /// The stored bytes: raw contents for a blob, canonical JSON otherwise.
    pub fn get_raw<K: ContentKind>(&self, id: &Id<K>) -> Result<Vec<u8>> {
        self.read(|conn| get_raw(conn, &id.to_string()))
    }

    /// Reads and strictly parses an object, checking it still hashes to `id`.
    pub fn get<T: Object>(&self, id: &Id<T::Kind>) -> Result<T> {
        self.read(|conn| get_object(conn, id))
    }

    pub fn has<K: ContentKind>(&self, id: &Id<K>) -> Result<bool> {
        self.read(|conn| has(conn, &id.to_string()))
    }

    /// The uncompressed size of each object, in order, read on one
    /// connection without loading any bodies.
    pub fn sizes<K: ContentKind>(&self, ids: &[Id<K>]) -> Result<Vec<u64>> {
        self.read(|conn| {
            let mut stmt = conn.prepare_cached("SELECT size FROM objects WHERE id = ?1")?;
            ids.iter()
                .map(|id| {
                    let key = id.to_string();
                    let size: Option<i64> = stmt.query_row([&key], |row| row.get(0)).optional()?;
                    size.map(|s| s as u64).ok_or(StoreError::NotFound(key))
                })
                .collect()
        })
    }

    /// The ref's target, or `None` if it does not exist. A ref whose target is
    /// not of kind `K` is reported as corrupt.
    pub fn get_ref<K: ContentKind>(&self, name: &RefName) -> Result<Option<Ref<K>>> {
        self.read(|conn| get_ref(conn, name))
    }

    /// Points `name` at `new` if it currently points at `expected` (`None`
    /// meaning the ref must not exist). Returns the new version.
    pub fn compare_and_set<K: ContentKind>(
        &self,
        name: &RefName,
        expected: Option<&Id<K>>,
        new: &Id<K>,
    ) -> Result<u64> {
        self.tx(|tx| tx.compare_and_set(name, expected, new))
    }

    /// Runs `f` in one immediate write transaction. Commits if `f` returns
    /// `Ok`, rolls back on `Err` or panic.
    pub fn tx<R, E: From<StoreError>>(
        &self,
        f: impl FnOnce(&Tx<'_>) -> Result<R, E>,
    ) -> Result<R, E> {
        let mut conn = self.writer.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::from)?;
        let wrapped = Tx { sql: tx };
        let value = f(&wrapped)?;
        wrapped.sql.commit().map_err(StoreError::from)?;
        Ok(value)
    }

    /// Runs `f` on a pooled read-only connection, which sees the latest
    /// committed state.
    pub(crate) fn read<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let pooled = self.readers.lock().pop();
        let conn = match pooled {
            Some(conn) => conn,
            None => self.open_reader()?,
        };
        let result = f(&conn);
        let mut readers = self.readers.lock();
        if readers.len() < MAX_IDLE_READERS {
            readers.push(conn);
        }
        result
    }

    fn open_reader(&self) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure(&conn)?;
        Ok(conn)
    }
}

/// A write transaction. Everything done through one `Tx` commits or rolls
/// back together.
pub struct Tx<'a> {
    /// The open transaction, for index tables owned by other modules in this crate.
    pub(crate) sql: rusqlite::Transaction<'a>,
}

impl Tx<'_> {
    pub fn put_blob(&self, bytes: &[u8]) -> Result<BlobId> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(StoreError::BlobTooLarge(bytes.len()));
        }
        let id = BlobId::of_bytes(bytes);
        put_raw(&self.sql, &id, bytes)?;
        Ok(id)
    }

    pub fn put<T: Object>(&self, object: &T) -> Result<Id<T::Kind>> {
        let bytes = object.canonical_bytes();
        let id = object.id();
        put_raw(&self.sql, &id, &bytes)?;
        Ok(id)
    }

    pub fn get_raw<K: ContentKind>(&self, id: &Id<K>) -> Result<Vec<u8>> {
        get_raw(&self.sql, &id.to_string())
    }

    pub fn get<T: Object>(&self, id: &Id<T::Kind>) -> Result<T> {
        get_object(&self.sql, id)
    }

    pub fn has<K: ContentKind>(&self, id: &Id<K>) -> Result<bool> {
        has(&self.sql, &id.to_string())
    }

    pub fn get_ref<K: ContentKind>(&self, name: &RefName) -> Result<Option<Ref<K>>> {
        get_ref(&self.sql, name)
    }

    pub fn compare_and_set<K: ContentKind>(
        &self,
        name: &RefName,
        expected: Option<&Id<K>>,
        new: &Id<K>,
    ) -> Result<u64> {
        let current = get_ref::<K>(&self.sql, name)?;
        let current_target = current.as_ref().map(|r| r.target);
        if current_target.as_ref() != expected {
            return Err(StoreError::RefConflict {
                name: name.clone(),
                expected: expected.map(ToString::to_string),
                actual: current_target.map(|id| id.to_string()),
            });
        }
        let version = current.map_or(1, |r| r.version + 1);
        self.sql.execute(
            "INSERT INTO refs (name, target, version) VALUES (?1, ?2, ?3)
             ON CONFLICT (name) DO UPDATE SET target = excluded.target, version = excluded.version",
            params![name.as_str(), new.to_string(), version as i64],
        )?;
        Ok(version)
    }
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Commands run side by side on one home, so a writer may queue behind
    // another process's long transaction.
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    Ok(())
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")?;
    let current: i64 = tx.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;
    let supported = MIGRATIONS.len() as i64;
    if current > supported {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported,
        });
    }
    for (version, sql) in (1..).zip(MIGRATIONS).skip(current as usize) {
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            [version],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn put_raw<K: ContentKind>(conn: &Connection, id: &Id<K>, bytes: &[u8]) -> Result<()> {
    let key = id.to_string();
    if has(conn, &key)? {
        return Ok(());
    }
    let compressed;
    let data = if bytes.len() > COMPRESS_OVER_BYTES {
        compressed = zstd::bulk::compress(bytes, ZSTD_LEVEL)?;
        &compressed[..]
    } else {
        bytes
    };
    conn.execute(
        "INSERT INTO objects (id, kind, size, data) VALUES (?1, ?2, ?3, ?4)",
        params![key, K::PREFIX, bytes.len() as i64, data],
    )?;
    Ok(())
}

fn has(conn: &Connection, id: &str) -> Result<bool> {
    let found = conn
        .query_row("SELECT 1 FROM objects WHERE id = ?1", [id], |_| Ok(()))
        .optional()?;
    Ok(found.is_some())
}

fn get_raw(conn: &Connection, id: &str) -> Result<Vec<u8>> {
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT size, data FROM objects WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (size, data) = row.ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
    let size = size as usize;
    let bytes = if size > COMPRESS_OVER_BYTES {
        zstd::bulk::decompress(&data, size).map_err(|e| StoreError::Corrupt {
            id: id.to_owned(),
            reason: format!("zstd: {e}"),
        })?
    } else {
        data
    };
    if bytes.len() != size {
        return Err(StoreError::Corrupt {
            id: id.to_owned(),
            reason: format!("stored size {size} but {} bytes", bytes.len()),
        });
    }
    Ok(bytes)
}

fn get_object<T: Object>(conn: &Connection, id: &Id<T::Kind>) -> Result<T> {
    let key = id.to_string();
    let bytes = get_raw(conn, &key)?;
    let corrupt = |reason: String| StoreError::Corrupt {
        id: key.clone(),
        reason,
    };
    let object = T::from_canonical(&bytes).map_err(|e: ParseError| corrupt(e.to_string()))?;
    if object.id() != *id {
        return Err(corrupt("contents do not hash to the id".into()));
    }
    Ok(object)
}

fn get_ref<K: ContentKind>(conn: &Connection, name: &RefName) -> Result<Option<Ref<K>>> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT target, version FROM refs WHERE name = ?1",
            [name.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    row.map(|(target, version)| {
        let target = target
            .parse()
            .map_err(|e: ParseError| StoreError::Corrupt {
                id: format!("ref {name}"),
                reason: e.to_string(),
            })?;
        Ok(Ref {
            target,
            version: version as u64,
        })
    })
    .transpose()
}
