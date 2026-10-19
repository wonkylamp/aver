# Tasks by hand

This page covers building Aver from a clone of this repository and creating tasks by hand. Agents use the `aver` CLI; for `aver run` and workspace directories, see [CLI.md](CLI.md).

## Requirements

- macOS or Linux.
- Rust 1.94 or later and `git`, and Node.js for the `calc` example below. Checks run natively, with the project's own toolchain.

## Build and set up a home

```bash
make up
```

This command does the following:

1. Builds the release binary, `target/release/aver`.
2. Creates `$AVER_HOME` (default `~/.aver`) with a server key and an admin key (`keys/admin.json`) on first run. Running it again keeps the existing keys.
3. Prints where the binary and the home are, and the commands to try next.

There is nothing to start or stop. Every `aver` command opens the home in its own process, and commands can run side by side on one home. Set `AVER_HOME` to use another home.

## Add a repository and a task

The CLI acts as the admin key. Any Git repository works. This example turns the `calc` test fixture into one:

```bash
cp -R fixtures/calc /tmp/calc
git -C /tmp/calc init -q && git -C /tmp/calc add -A && git -C /tmp/calc commit -qm "calc"
./target/release/aver import /tmp/calc --ref trunk
```

`import` points the `trunk` ref at the new snapshot and prints its id:

```json
{
  "already_imported": false,
  "commit": "f0037e5…",
  "env": "env_3bkngiirrj4mim666je6fxxsi2ibdnkve5b7xs3zrmiwpjo33wrq",
  "files": 6,
  "snapshot": "snap_g2dpduhxe7mdi6uiyqhq5326eie4r2fd2eaqrskhd2yrxtrxy7iq",
  "transition": "tran_…"
}
```

Snapshot ids are content hashes, so the same files give the same id on any machine.

Write the task to a file. `base` is the snapshot from `import`, `target` is the ref an accepted attempt advances, and criteria ids are what receipts report as proven or unproven:

```json
{
  "title": "Implement divide",
  "intent": "divide(a, b) returns a / b and throws on division by zero",
  "base": "snap_g2dpduhxe7mdi6uiyqhq5326eie4r2fd2eaqrskhd2yrxtrxy7iq",
  "target": "trunk",
  "criteria": [
    { "id": "div-basic", "text": "divide returns the quotient" },
    { "id": "div-zero", "text": "divide throws on division by zero" }
  ]
}
```

```bash
./target/release/aver task create --file task.json
```

The command signs the task as the admin, stores it and prints `{"id": "task_…", "task": …}`. To give the task to an agent, open a workspace directory on it with `aver workspace new /tmp/calc --task <id>` and start the agent there.

A test proves a criterion when its name contains `[crit:<id>]`, as in `test('[crit:div-zero] divide throws on division by zero', …)`, it passes with the change, and it fails (or cannot run) on the base plus only the change's new test files: a test that already passes without the change proves nothing. A task can also define its own checks. They are the stronger proof: the agent cannot change their definitions, they run in the base snapshot's environment rather than one the agent's `aver.toml` chose, and receipts mark their entries `source: "task"`. Add a `checks` list whose entries have `id`, `argv`, and `proves` (a list of criterion ids), plus optional `cwd`, `timeout_ms` and `report` (`{"format": "junit", "path": …}`).

A check's definition is fixed, but the files it runs are repository content the agent can edit. List them in `protected_paths` to make that a failure:

```json
"protected_paths": ["test/**", "package.json"]
```

Each pattern is a repo-relative path where `*` and `?` match within one segment and a whole segment `**` matches any number of segments; a directory covers everything under it. Any file the agent adds, changes or deletes under a protected path becomes a receipt entry `protected::<path>` with cause `NEW`, so the verdict is `FAIL_OWN` whatever the checks report. A task with protected paths is stored as `aver.task/v1`; one without stays `aver.task/v0`.
