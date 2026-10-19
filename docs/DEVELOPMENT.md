# Developing Aver

How to test Aver, where the code lives and how the website is built. [CLAUDE.md](../CLAUDE.md) lists every `make` target and the rules for changing the code.

## Tests and benchmarks

| Command | What it runs |
|---|---|
| `make test` | Rust tests that run no check natively, the custody crash matrix, the website's tests, the GitHub Action's script tests, the install script tests and the evaluation harness's self-test |
| `make test-action` | The GitHub Action's scripts against simulated GitHub environments (stub `gh` and `aver`) |
| `make test-install` | `site/install.sh` and the Homebrew formula script against fake releases and stub platforms |
| `make test-native` | The `#[ignore]`d execution and verification tests, which run checks and Node natively |
| `make lint` | `cargo fmt --check`, `clippy -D warnings`, and TypeScript type checks |
| `make bench` | The import, workspace and cache benchmarks; writes `bench/results.md` |
| `make bench-native` | Native `aver verify` after a one-line edit, against the tests alone ([BENCHMARKS.md](BENCHMARKS.md#native-verify-speed)) |
| `make eval` | The A/B evaluation: the same Claude Code agents on six hard scenarios in `bench/eval/tasks/` (already-failing and flaky suites, a vague task, a tempting test to skip, two tasks on overlapping files, a refactor) in a plain worktree and under `aver run`, scored by hidden acceptance tests ([BENCHMARKS.md](BENCHMARKS.md#ab-evaluation-with-real-agents)). Spends real tokens; resumable (`ARGS="--scenarios <name>"`, `--dry-run` for scripted agents) |
| `bench/workspace-disk.sh` | Workspace directory creation and sync against `git worktree add`; writes `bench/results/disk.json` |
| `make schemas-check` | Every object type round-trips through its JSON Schema |

## Deleting an Aver home

A throwaway home (an `AVER_HOME` from a trial or a benchmark) holds read-only checkouts, so `rm -rf` fails on it. Delete it with:

```sh
aver gc --delete-home /tmp/trial-home
```

It refuses a directory that is not an Aver home (`keys/` and `aver.db`) and one another live `aver` process is using (a record in its `running/`). By hand, give yourself write access first:

```sh
chmod -R u+w /tmp/trial-home && rm -rf /tmp/trial-home
```

## Website

[site/](../site/) is a static landing page and a receipt viewer that checks a receipt's id and signature in the browser with the TypeScript signing code in `site/src/core/`. It deploys as plain files to GitHub Pages or Cloudflare Pages. `make site` rebuilds the viewer bundle. [site/README.md](../site/README.md) covers testing and deployment.

## Layout

| Path | Contents |
|---|---|
| `crates/aver-core` | Ids, canonical JSON, hashing, signing, object types, cause and verdict rules |
| `crates/aver` | Object store, Git bridge, environment, workspaces and their directories, execution, verification, tasks, the in-process `Api`, CLI |
| `action/` | The `aver-verify` GitHub Action and its tests |
| `schemas/` | Versioned JSON Schemas for every stored object, plus cross-language test vectors |
| `fixtures/` | Small Node, Python, Go and Rust projects used by the tests |
| `packaging/` | The Homebrew formula template and the install script tests |
| `bench/` | Benchmark scripts and results |
