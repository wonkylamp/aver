-- Leases: advisory reservations of paths across tasks (ARCHITECTURE.md §1.8.4).

CREATE TABLE leases (
  id          TEXT PRIMARY KEY,
  scope       TEXT NOT NULL,  -- canonical repository path, or "snapshot:<id>" for a workspace without a directory
  task        TEXT,
  attempt     TEXT,
  workspace   TEXT NOT NULL REFERENCES workspaces (id),
  agent       TEXT NOT NULL,
  paths       TEXT NOT NULL,  -- JSON array of path globs (a set, 1-16)
  mode        TEXT NOT NULL CHECK (mode IN ('exclusive', 'shared')),
  reason      TEXT NOT NULL,
  ttl_ms      INTEGER NOT NULL,
  acquired_at TEXT NOT NULL,
  expires_at  TEXT NOT NULL,
  renewals    INTEGER NOT NULL,
  released_at TEXT
);
CREATE INDEX leases_live ON leases (scope, released_at);
CREATE INDEX leases_by_workspace ON leases (workspace, released_at);
