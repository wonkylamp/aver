-- Aver state database, version 2. ARCHITECTURE.md §1.9.4 and §1.11.

-- Unsigned diagnostics of a receipt: derived data, not evidence, so a row
-- rather than an object. `body` is the JSON document.
CREATE TABLE diagnostics (
    receipt TEXT PRIMARY KEY,
    body    TEXT NOT NULL
) WITHOUT ROWID;
