-- What a Git import of a commit found, so importing the same commit again
-- (`aver workspace new` on an unchanged HEAD) reads neither the tree nor
-- the blobs (ARCHITECTURE.md §2.2). One row per commit and the transition
-- that recorded it; `findings` is a JSON array of [path, pattern] for every
-- secret-scanner finding, so a stricter policy still refuses the commit.

CREATE TABLE IF NOT EXISTS import_memo (
    git_commit TEXT NOT NULL,
    transition TEXT NOT NULL,
    files      INTEGER NOT NULL,
    bytes      INTEGER NOT NULL,
    findings   TEXT NOT NULL,
    PRIMARY KEY (git_commit, transition)
) WITHOUT ROWID;
