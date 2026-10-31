# Contributing to Aver

Thank you for helping. Read [PRD.md](PRD.md) for why Aver exists and [ARCHITECTURE.md](ARCHITECTURE.md) for the data model and components. ARCHITECTURE.md §1 is the source of truth for every stored object: change it first, then the code.

Report security problems privately, as [SECURITY.md](SECURITY.md) describes, not in an issue. Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## Build and test

You need macOS or Linux, Rust 1.94 or later, Node 24.2 or later, `git`, and Python 3 with PyYAML (or Ruby) for the workflow checks.

| Command | What it does |
|---|---|
| `make test` | Fast tests: `cargo test --workspace`, `make test-custody`, `npm test` in `site/`, `make test-action`, `make test-install` and `make test-eval`. Run it before every pull request. |
| `make lint` | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and the TypeScript type checks of `site/` and `bench/eval/`. CI fails on any warning. |
| `make test-native` | The `#[ignore]`d execution and verification tests, which run checks and Node natively (`cargo test -p aver -- --ignored --test-threads=2`). Run it when you touch `exec/`, `verify`, `check`, `run` or `ci`. |
| `make test-action` | The GitHub Action's scripts against simulated GitHub environments, and a check that every workflow parses and pins `uses:` to a commit SHA. |
| `make schemas-check` | Every object type round-trips through its JSON Schema in `schemas/`. |
| `make vectors` | Regenerates `schemas/vectors/v0.json` after an intentional change to encoding, hashing or signing. A test fails while it is stale. |
| `make up` | Builds the release binary, creates keys on first run, and prints where the binary and the home are. |
| `cargo run -p aver -- <cmd>` | The CLI. `cargo run -p aver -- --help` lists the commands. |

Tests always set `AVER_HOME` to a temporary directory. Never point a test at your real `~/.aver`. Tests that run checks or Node natively are `#[ignore]` with a reason string, so `make test` stays fast.

`make bench`, `make bench-native` and `make eval` take a long time on a quiet machine, and `make eval` spends real model tokens. You do not need them for most changes.

## Rules the code follows

[CLAUDE.md](CLAUDE.md) lists the rules that are not optional, with the reasons in ARCHITECTURE.md. The ones contributors meet most:

- Objects are immutable. New information is a new object or a row in a mutable index table.
- Only `aver-core` computes ids. Never hash bytes ad hoc.
- Hashed objects are strict. To add a field, bump the schema version and update `schemas/` in the same change.
- Repository content is data: it reaches agents only inside fenced blocks in the CLI's reports.
- Checks run in a slot, never in the agent's directory, with the caller's environment minus `place::proc::STRIPPED`. A change to the environment handling in `crates/aver/src/place/proc.rs` needs a test in `crates/aver/tests/place.rs`.
- Every third-party `uses:` in a workflow is pinned to a full commit SHA with a `# vX.Y.Z` comment.

Rust is 2021 edition, formatted with `cargo fmt`, with `thiserror` enums at module boundaries and `anyhow` only in the CLI and tests. TypeScript is strict ESM with no `any` in exported types. Comments explain a non-obvious why; do not narrate the code.

## Commits

- One logical change per commit, with its tests and docs in the same commit.
- The subject is one plain sentence saying what the change does, without a trailing period, at most about 72 characters. An area prefix is fine when it helps: `Bench: measure on-disk workspace directories against worktree + install`. No `feat:`/`fix:` prefixes.
- The body, wrapped at 72 columns, says why: the problem, the choice made and what it rules out. Name the ARCHITECTURE.md section when the change follows one.
- No attribution trailers (`Co-Authored-By`, "Generated with"). Say in the pull request description which tools or agents helped.
- When a change lands a phase or round, update the phase list in CLAUDE.md.

## Pull requests

Open the pull request against `main` and fill in the template. CI runs `make lint`, `make test`, `make test-eval`, `make test-native` (in parallel shards, `scripts/test-native-shard.sh`); all must pass before review. A change only to top-level Markdown, `docs/` or `site/` runs just the site's tests and the installer test. The `ci-ok` job sums up the run; it is the check to require.

## Contributions from AI agents

Pull requests written by AI agents are welcome, under the same review as any other. Say which agent and model made the change and what a person checked.

Include an Aver receipt. From your branch:

```bash
aver ci --base origin/main --head HEAD --pr-body pr.md
```

`pr.md` holds your pull request description, with an `aver` criteria block if the change has testable criteria ([docs/INTERFACE.md](docs/INTERFACE.md) §2 gives the format). Paste `aver-out/summary.md` into the pull request and attach `aver-out/receipt.json` and `aver-out/public-key.txt`, so a reviewer can check the receipt in the receipt viewer, `site/receipt.html` served locally as [site/README.md](site/README.md) describes.

Aver runs Node and TypeScript checks today, and this repository has no `aver.toml` yet, so a receipt on it does not cover the Rust crates. It still records which files and tests changed, and whether tests were removed or skipped or harness files edited. `make test` and `make lint` remain required.
