-- Aver state database, version 1. ARCHITECTURE.md §1.8 and §1.11.
-- Ids are stored in their full textual form. Timestamps are RFC 3339 text
-- (aver_core::Timestamp) unless the column name ends in _ms.

-- Bodies longer than 1024 bytes are zstd-compressed; `size` is the raw length,
-- so compression is implied by size and needs no extra column.
CREATE TABLE objects (
    id   TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    size INTEGER NOT NULL,
    data BLOB NOT NULL
);

-- Named pointers, CAS-updated. `target` is a content id whose kind depends on
-- the namespace: snapshots for branch-like refs, ledger entries for `ledger/<scope>`.
CREATE TABLE refs (
    name    TEXT PRIMARY KEY,
    target  TEXT NOT NULL,
    version INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE agents (
    id           TEXT PRIMARY KEY,
    public_key   BLOB NOT NULL CHECK (length(public_key) = 32),
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,
    capabilities TEXT NOT NULL DEFAULT '[]', -- JSON array of capability strings
    issued_by    TEXT REFERENCES agents(id),  -- NULL only for the bootstrap admin
    issued_at    TEXT NOT NULL,
    revoked_at   TEXT
);

-- Audit log of signed writes; its key also refuses a write replayed after a
-- restart. Reads are not recorded (auth.rs, ARCHITECTURE.md §6.3).
CREATE TABLE requests (
    agent     TEXT NOT NULL REFERENCES agents(id),
    nonce     TEXT NOT NULL,
    ts_ms     INTEGER NOT NULL,
    method    TEXT NOT NULL,
    path      TEXT NOT NULL,
    body_hash TEXT NOT NULL,
    signature TEXT NOT NULL,
    PRIMARY KEY (agent, nonce)
) WITHOUT ROWID;
CREATE INDEX requests_by_ts ON requests (ts_ms);

CREATE TABLE workspaces (
    id         TEXT PRIMARY KEY,
    base       TEXT NOT NULL,
    owner      TEXT NOT NULL REFERENCES agents(id),
    -- Deferred because an attempt and its workspace reference each other and
    -- are created in one transaction.
    attempt    TEXT REFERENCES attempts(id) DEFERRABLE INITIALLY DEFERRED,
    state      TEXT NOT NULL CHECK (state IN ('open', 'committed', 'discarded')),
    version    INTEGER NOT NULL DEFAULT 0,
    head       TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((state = 'committed') = (head IS NOT NULL))
);

-- The copy-on-write delta, one row per changed path. `kind = 'delete'` rows
-- carry no blob.
CREATE TABLE overlay (
    workspace TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path      TEXT NOT NULL,
    kind      TEXT NOT NULL CHECK (kind IN ('file', 'exec', 'symlink', 'delete')),
    blob      TEXT,
    PRIMARY KEY (workspace, path),
    CHECK ((kind = 'delete') = (blob IS NULL))
) WITHOUT ROWID;

CREATE TABLE attempts (
    id         TEXT PRIMARY KEY,
    task       TEXT NOT NULL,
    agent      TEXT NOT NULL REFERENCES agents(id),
    workspace  TEXT NOT NULL REFERENCES workspaces(id) DEFERRABLE INITIALLY DEFERRED,
    base       TEXT NOT NULL,
    state      TEXT NOT NULL CHECK (state IN ('open', 'submitted', 'accepted', 'rejected', 'abandoned')),
    receipts   TEXT NOT NULL DEFAULT '[]', -- JSON array of receipt ids in creation order
    created_at TEXT NOT NULL
);

-- Rowid order is insertion order, which export uses as "earliest".
CREATE TABLE transitions_by_snapshot (
    snapshot   TEXT NOT NULL,
    transition TEXT NOT NULL,
    PRIMARY KEY (snapshot, transition)
);

-- Blobs and trees map one-to-one onto Git objects. Commits are keyed by the
-- transition that records them, because one snapshot can be several commits.
CREATE TABLE git_map (
    aver_id TEXT PRIMARY KEY,
    git_sha1  TEXT NOT NULL,
    kind      TEXT NOT NULL CHECK (kind IN ('blob', 'tree', 'commit'))
) WITHOUT ROWID;
CREATE INDEX git_map_by_sha1 ON git_map (git_sha1);

CREATE TABLE run_cache (
    snapshot  TEXT NOT NULL,
    env       TEXT NOT NULL,
    check_id  TEXT NOT NULL,
    executor  TEXT NOT NULL,
    run_index INTEGER NOT NULL CHECK (run_index BETWEEN 1 AND 3),
    run       TEXT NOT NULL,
    PRIMARY KEY (snapshot, env, check_id, executor, run_index)
) WITHOUT ROWID;

CREATE TABLE deps_layers (
    deps_key TEXT PRIMARY KEY,
    volume   TEXT NOT NULL,
    state    TEXT NOT NULL CHECK (state IN ('building', 'ready', 'failed')),
    size     INTEGER
) WITHOUT ROWID;
