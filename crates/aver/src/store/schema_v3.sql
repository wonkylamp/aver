-- On-disk workspaces (ARCHITECTURE.md §1.8, §1.8.1-§1.8.3).

ALTER TABLE workspaces ADD COLUMN mode TEXT NOT NULL DEFAULT 'api' CHECK (mode IN ('api', 'disk'));

-- Who else changed this path: overlap notices across workspaces (R5c).
CREATE INDEX overlay_by_path ON overlay (path);

CREATE TABLE materializations (
  workspace   TEXT PRIMARY KEY REFERENCES workspaces (id),
  dir         TEXT NOT NULL UNIQUE,
  repo        TEXT,
  checkout    TEXT NOT NULL,
  clone       TEXT NOT NULL CHECK (clone IN ('clonefile', 'reflink', 'copy')),
  farms       TEXT NOT NULL,  -- JSON array of {dir, deps, mode}
  seeded      TEXT NOT NULL,  -- JSON array of paths
  absent      TEXT NOT NULL,  -- JSON array of paths
  git_view    INTEGER NOT NULL,
  port_base   INTEGER,
  port_count  INTEGER,
  state       TEXT NOT NULL CHECK (state IN ('live', 'released', 'lost')),
  created_at  TEXT NOT NULL,
  released_at TEXT
);
CREATE INDEX materializations_by_repo ON materializations (repo, state);

CREATE TABLE checkouts (
  tree         TEXT PRIMARY KEY,
  dir          TEXT NOT NULL,
  files        INTEGER NOT NULL,
  bytes        INTEGER NOT NULL,
  absent       TEXT NOT NULL,
  state        TEXT NOT NULL CHECK (state IN ('building', 'ready')),
  last_used_at TEXT NOT NULL
);

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
