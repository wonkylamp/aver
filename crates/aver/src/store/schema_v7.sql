-- The built test files of a run (ARCHITECTURE.md §1.11): the content hash
-- of each, so verification compares a base's build with a judged head's
-- (`built::<path>` entries) without storing the files.

CREATE TABLE IF NOT EXISTS run_built (
    run   TEXT PRIMARY KEY,
    files TEXT NOT NULL
) WITHOUT ROWID;
