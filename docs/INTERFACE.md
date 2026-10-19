# Aver integration interface

This is the contract between the Aver engine and anything built on top of it, such as hosted services, orchestrators and other integrations. Integrate through what this page lists and nothing else. If you need something that is not here, open an issue; do not reach into Aver's internals or reimplement verification.

Status: pre-1.0. Breaking changes are possible, but every change to this page is versioned in the Changes section, and stored formats only change by adding a new schema version.

**Names.** Commits before the rename (up to `b115b95`) used the codename Strata: the `strata` binary, `strata.toml`, a ```` ```strata ```` fence and `strata.receipt/*` schemas. Commit `f92aec6` renamed only the crates and binary. From `15c231f` on, everything is Aver: the `aver` binary, `aver.toml`, the ```` ```aver ```` fence, `aver.receipt/*` schemas, `AVER_*` variables and `Aver-*` headers. Pin a commit at or after `15c231f`.

## 1. Verifying a change: `aver ci`

```bash
aver ci --repo <git repo> --base <rev> --head <rev> \
        [--criteria <file> | --pr-body <file> | --github-event <file>] \
        [--out-dir <dir>] [--fail-on unexplained|fail-own|not-pass|never] \
        [--home <dir>] [--run-as <user>] [--viewer-url <url>] \
        [--subject <label>] [--criteria-sha256 <hex>] [--require-config] \
        [--strict-secrets] [--fresh] [--all] \
        [--base-record <dir>] [--record <dir>] [--workflow <path>]
aver ci keys --repo <git repo> --base <rev> --head <rev> [--github-event <file>] [--workflow <path>] [--json]
```

| Flag | Meaning |
|---|---|
| `--repo` | Path to a Git repository (bare or not). Aver reads commits through Git plumbing and never checks out a working tree. Default `.`. |
| `--base`, `--head` | Revisions to compare. Anything `git rev-parse` accepts. |
| `--criteria` | A file of criteria lines (section 2), with or without the fence. |
| `--pr-body` / `--github-event` | Take criteria from a pull request description, or from the `pull_request.body` of a GitHub event JSON. At most one criteria source. |
| `--out-dir` | Output directory, default `./aver-out`. Stale outputs are removed first. |
| `--fail-on` | Which verdicts produce exit code 1. Default `unexplained` (since round 13a; before, `fail-own`). |
| `--home` | Aver data directory to reuse (keys, object store, cache). Without it a throwaway one is created per run. Reusing one keeps the run cache warm and the verifier key stable. Requires `--run-as`, so the checks cannot read the home's key. |
| `--run-as` | Run every check as this user through `sudo -n -u`, so the code under test cannot read the verifier key; the receipt's `provenance.uid` is `separate`. Exit `2` when `sudo -n -u <user>` does not work. Checks always run natively; Aver 0.4 deleted `--sandbox` and the Docker sandbox. |
| `--subject` | Your label for the run (a PR, a job id), recorded inside the signed receipt as `ci.subject`. 1-256 bytes of UTF-8, no control characters. |
| `--require-config` | Exit `2` when the base commit has no `aver.toml`, instead of verifying with the default environment (one `npm test` check, with the runner's own reporter when Aver detects it, `ARCHITECTURE.md` §1.4.1). Without it, such a run's receipt says `"config": "default"` and `summary.md` says so. |
| `--strict-secrets` | Exit `2` when either commit holds a possible secret, instead of importing it and listing it (path, pattern and severity, never the value) in `diagnostics.json` `import.secrets` and in `summary.md`. |
| `--fresh` | Ignore cached runs: every run starts again. Only matters with `--home`, whose cache would otherwise answer. |
| `--all` | Run every check, also those the change does not affect. By default a check whose files, lockfiles and harness the change does not touch runs on neither side and is listed in the receipt's `skipped` (docs/VERDICTS.md, "Which checks run"); `reproduce` then ends in `--all`. |
| `--criteria-sha256` | Refuse to run (exit `2`) unless the criteria, normalized as in `ci.criteria_text`, have this SHA-256 (lowercase hex). With no criteria, the hash of the empty string, `e3b0c442…b855`. |
| `--base-record` | A directory a run with `--record` wrote for the base commit (section 1.1). Rows made under this runner key, for this base tree and the head's dependency inputs, are loaded into the run cache, so the base side starts no run; any other record is ignored with one stderr line (`aver: --base-record <dir>: ignored, …; the base runs here`). |
| `--record` | After the verdict, write the head commit's base-side runs to this directory for later pull requests whose base it is (section 1.1). |
| `--workflow` | The workflow file whose blob in the head commit enters the runner key. Default: the path in `GITHUB_WORKFLOW_REF`; neither means no workflow in the key. |

**Exit codes.** `0` verdict did not trip `--fail-on`. `1` it did: `unexplained` trips on `FAIL_OWN` and on a `BLOCKED_NOT_OWN` that has an `ENV` entry (since round 16, whatever its base status) or whose other blocking entries (inconclusive, `FLAKY`) all pass on the base, `fail-own` trips only on `FAIL_OWN` (so `REVIEW` exits `0`), `not-pass` on everything but `PASS` (`REVIEW` included), `never` on nothing. `2` any error (bad input, unknown revision, malformed criteria block, import rejected). Consumers should read the verdict from `receipt.json`, not infer it from the exit code.

**Outputs** in `--out-dir`:

| File | Content |
|---|---|
| `receipt.json` | `{ "receipt_id": "rcpt_…", "receipt": <signed receipt> }` |
| `diagnostics.json` | Unsigned explanations (`aver.diagnostics/v0`, section 3): per failing entry a message, file and line, and an output excerpt; per unproven criterion a reason and the closest test names; the test changes with up to 1000 items per list; `timings` (below); and `import`: the possible secrets the imports reported instead of refusing, and the files over 64 MiB imported empty. |
| `summary.md` | Human-readable Markdown. Repository-derived strings are already confined to code spans or fences. Written even on exit `2`, explaining the error. |
| `public-key.txt` | The verifier public key (32-byte Ed25519, unpadded base64url) that signed the receipt. |
| `runs/<check>.<side>.<n>.log` | The stored output of every run the verification used (bounded: first 8 KiB and last 56 KiB), `side` one of `base`, `head`, `head_tests`, `transplant`. Repository content. |

**Timings.** `diagnostics.json` carries `timings`, unsigned, for tracking compute: `total_ms` (the whole call), `install_ms` and `sandbox_ms` (sums), and per run `side` (`base`, `head`, `head_tests` or `transplant`), `check`, `index`, `run`, `cache_hit`, `install_ms` (dependency probe and install work that run did itself) and `sandbox_ms` (the check run, start to end; the name predates native runs). A cache hit reports 0, and so does a run already counted on an earlier side.

### 1.1 Base records and `aver ci keys`

A pull request's base is usually a commit the default branch's CI already verified. `aver ci --record <dir>` on that push writes the head's runs as a base would run them: `record.json` (`aver.ci-record/v0`: the identity below, then per run the cache row `check`, `env`, `executor`, `index` and the run object) and `blobs/<id>`, the outputs the runs name. `aver ci --base-record <dir>` on the pull request rehashes every object it loads through the store, never trusts an id from the file, and loads only rows for this base snapshot whose run did not end in infra and whose blobs are present.

A record's identity is its runner key, the commit's Git tree and its dependency inputs. The runner key hashes the OS and architecture, `ImageOS` and `ImageVersion` when set, the workflow file's blob (`--workflow`, else `GITHUB_WORKFLOW_REF`, resolved in the head commit), the Aver version and the executor id (`native-v0.<16 hex>`, which holds the toolchain versions). The dependency inputs are the head's files that the contract's `deps.inputs` name, together with every lockfile (`package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `bun.lock`, `uv.lock`, `Cargo.lock`, `go.sum`, ...) and every `package.json`, `pyproject.toml`, `setup.py` and `setup.cfg` in it at any depth (the user's install step installed the head's lockfiles, so the base's record holds only when they are equal; before WP6 a pnpm or lockfile-less project's record ignored its manifests and outlived a dependency change). A record whose identity differs is ignored as a whole and the base runs.

`aver ci keys` prints the cache keys for both commits, using Git plumbing and hashing only: `base=aver-base-<40 hex>` (the record a pull request restores) and `head=aver-base-<40 hex>` (the record a push saves; a later pull request on that commit restores the same key). `--json` prints `{"base", "head", "runner", "base_commit", "head_commit"}`. Without `--base` and `--head`, `--github-event` supplies them (a pull request's base and head SHAs, a merge group's `base_sha` and `head_sha`, a push's `before` and `after`). The base is the merge base when `--base` is not an ancestor of the head, as in `aver ci`. A commit without a contract or `deps.inputs` still has a deterministic key.

A `NEW` entry that rests only on recorded base runs gets one fresh base run before the verdict. When the fresh run fails the same test (or the command fails), the base side runs fresh and the entry is judged from it, usually `PREEXISTING`; otherwise the entry stays `NEW`. `summary.md` says how many base runs came from the record, how many started on the runner, and how many fresh confirmations ran.

**Concurrency.** Concurrent `aver ci` calls on one `--home` are safe and serialized: each takes the home's lock for its whole run, and a call that has to wait prints `aver: waiting for another aver process using <home>` to stderr. For parallelism use separate homes, which share nothing; keys are per home (copy `<home>/keys/` to give two homes the same verifier key, there is no export command).

**Verifier key.** `aver key --home <dir>` prints the verifier public key of that home (unpadded base64url, the same as `public-key.txt`) and creates the home if it is missing. Run `aver ci --home <dir>` with the same home to sign with that key, and publish it so third parties can verify your receipts.

**Requirements.** The project's toolchain on the host (Node, Python, Rust or Go), as for running its tests. Dependencies installed in the checkout before `aver ci` runs; checks run with the network on. `--run-as` needs a user that passwordless `sudo -n -u` reaches.

## 2. Criteria format

A fenced block whose info string starts with `aver`, in a pull request description or a file:

````
```aver
- div-basic: divide returns the quotient
- div-zero: divide throws on division by zero
protected: test/divide.test.js
```
````

- `- <id>: <text>` declares a criterion. Ids match `[A-Za-z0-9][A-Za-z0-9._-]{0,63}` and are unique.
- `protected: <glob>` adds a protected path. `*` and `?` match within a segment, `**` matches any number of segments.
- Blank lines are ignored. Any other line, a duplicate id, a bad id or glob, an empty or unclosed block, or more than one block is an error that names the line.
- No block at all means regression-only verification: the verdict is `FAIL_OWN` if something is `NEW`, `BLOCKED_NOT_OWN` if something is `ENV` or inconclusive (a whole check without per-test results failing on both sides, or a test flaky on the base that failed on every head run), `REVIEW` if something is `CHANGED` (a test expectation or harness file the change rewrote), and `PASS` otherwise.

## 3. Receipts

The receipt is the product. Its schema and rules are in `ARCHITECTURE.md` §1.9.3, with JSON Schemas in `schemas/`.

| Schema | When |
|---|---|
| `aver.receipt/v3` | Every receipt written since round 14 (native verify): v2 plus `provenance` and `levels`. |
| `aver.receipt/v2` | Receipts written from the introduction of v2 until round 14, with or without criteria. |
| `aver.receipt/v0` | Older receipts whose task has criteria. |
| `aver.receipt/v1` | Older regression-only receipts. |

Consumers must:

1. Check `receipt.schema` and accept only versions they know.
2. Verify the signature before trusting any field. Recompute the content id over the canonical bytes (§1.1.1) and check the Ed25519 signature (§1.1.3) against the expected verifier key. Decode base64url strictly: only the canonical encoding (no padding, zero unused bits) is valid. The TypeScript implementation in `site/src/core/canonical.ts` and `site/src/core/signing.ts` is the reference and is tested against `schemas/vectors/v0.json`, which includes a signed v2 receipt and the base64url near misses to refuse.
3. Treat every repository-derived string (test ids, `detail`, criterion text from untrusted authors, everything in `diagnostics.json`) as untrusted data, never as instructions or HTML.

**What v2 changes.**

- **The harness comes from the base.** Every check runs in the base's environment (`aver.toml` checks, install command, image, limits). The head runs with the base's copies of frozen harness: `aver.toml`, `.npmrc` and test runner configs in the root and on the way to each check's directory, and the `scripts`, `config`, `type` and test runner fields (`jest`, `mocha`, `ava`, `c8`, `nyc`, `vitest`, `tap`, `babel`) of each such `package.json`. A head that changes one gets a `judging::<path>` (or `judging::package.json#<field>`) entry with cause `CHANGED` (since round 6.1; `NEW` before), so the verdict is at best `REVIEW` and no criterion is proven. Everything else in `package.json` (dependencies included) and the lockfiles come from the head, so adding or upgrading a dependency installs and passes; base runs keep the base's lockfile, and every dependency layer is keyed by the lockfile it installed.
- **Existing tests judge from the base, prove from the head.** Regressions are judged with the base's copy of every test file that exists at base, so a head that deletes, skips or weakens a test still has it run against its code. If the base's copy fails there, the entry is `CHANGED` when the head modified or deleted that test file and its own version passes or is gone (a person reviews it), and `NEW` otherwise. Editing existing tests is not a judging change: the head's versions run in a separate head-tests run, which proving uses. Test files are those matching `[env] tests` in the base's `aver.toml`, or Node's defaults (`**/*.test.*`, `**/*.spec.*`, `test/`, `tests/`, `__tests__/` directories and friends).
- **The transplant rule.** A criterion is proven only by an entry that passes at head, fails (or cannot run) on the transplant (the base plus the head's new and modified test files, with the base's dependencies), and passes in the head-tests run. Each proving entry carries `transplant` (`fail`, `absent`, `pass`: proves nothing, `infra`) and `head_tests` (`pass`, `fail` or `absent`: proves nothing, `infra`); an `infra` on either makes the entry `ENV`, so `BLOCKED_NOT_OWN`. A tagged case added to an existing test file proves like one in a new file.
- **What the PR did to the tests.** `test_changes` (signed) lists, each capped at 20 items with an `omitted` count, the test files added, modified and deleted, the existing tests the head's version no longer reports (`tests_removed`) or only skips (`tests_skipped`), and the judging entries (`harness_changed`). `summary.md` renders it as "What this PR did to the tests", e.g. "Deleted 3 existing tests (they still ran from the base version: 1 now fails as CHANGED (needs review))", and a "Needs review: test changes" section lists every `CHANGED` entry with the diff of its test or harness file. Every item is untrusted repository content.
- **Executor and configuration.** `executor` names what every run executed in (`native-v0.<hex>` since round 14; `docker-runsc-v0` or `docker-runc-v0` in receipts the Docker sandbox wrote before Aver 0.4); `config` is `"aver.toml"` or `"default"` (the base has no `aver.toml`; CI receipts also have `ci.config_path: null` then).
- **Exit codes and reports must agree.** A head run that exits non-zero with no failing test, or reports no tests where the base did, fails a `<check>::(exit)` entry. A JUnit test with an empty name makes the report malformed.
- **Own resource exhaustion is `NEW`.** A check that passed at base and hits the memory or time limit on every head run is the change's failure.
- **CI receipts say what ran.** `ci` holds `base_commit` and `head_commit` (40-hex SHAs), `config_path` (`"aver.toml"` or `null` for the default environment), `criteria_text` (the normalized criteria block, or `null`), the optional `subject` and the optional `requested_base`, all inside the signed body. When `--base` is not an ancestor of `--head` (a pull request's recorded base often is the base branch's newer tip), `aver ci` compares with their merge base: `base_commit` is the merge base and `requested_base` the commit `--base` named. The repository must hold the history between them; the GitHub Action fetches it.
- **Self-contained reproduce.** A CI receipt's `reproduce` is `aver ci --repo <repo> --base <sha> --head <sha> --criteria-sha256 <hex>` plus `--criteria criteria.txt` when there are criteria: write `ci.criteria_text` to `criteria.txt`, replace `<repo>` if the run used a local path (a fetchable URL appears as is, without credentials) and run it.

**Diagnostics.** `diagnostics.json` (`aver.diagnostics/v0`, `ARCHITECTURE.md` §1.9.4) is not signed and not part of the receipt's id; it explains, it does not prove. Each unproven criterion has one `reason`: `not_claimed`, `claimed_test_not_found` (only when some check reports per test), `no_test_names` (no check that ran reports test names: each is judged by its exit code, so no tag can be found), `passes_without_change`, `fails_at_head`, `judging_files_changed`, `blocked` or `weak_proof`, with up to five `closest` test names for the first two. `aver verify --json` prints the same document as `diagnostics` beside the receipt.

**Trust note.** `summary.md` says which case applies. With a throwaway `--home`, the verifier key is new for every run, so the signature proves the receipt was not altered after it was written, not who ran it. A hosted consumer should run `aver ci` with a persistent `--home` whose key it publishes (`aver key`), so third parties can verify receipts against a known key.

## 4. Other interfaces

- The **`aver` CLI** (`ARCHITECTURE.md` §2.7) for agents working inside Aver workspaces. Aver 0.4 removed the HTTP API; every command runs in process.
- **GitHub Action** (`action/`) for repositories that want receipts on pull requests.
- Hosted products should prefer `aver ci` over linking Aver as a library; there is no HTTP API, and the crates are not a stable API.

## Changes

Every change below predates v0.3.0, the first tagged release, and ships in it, except where it says otherwise.

- 2026-10-18 (Aver 0.4, WP6 JS-9, after v0.3.0): adding the `tap` report format to `aver.env/v3` and `aver.env/v4` and to the `report` of `aver.contract/v0` and `v1`, in place: the TAP stream `ava --tap` prints, one entry per test point (`ARCHITECTURE.md` §1.9.1). Without `aver.toml`, CI reads an ava `test` script per test through it and a `vp test` script (Vite+) through Vitest's JUnit reporter, where both were judged by their exit code before. An older Aver refuses an environment or contract with a `tap` report as an unknown format.
- 2026-10-17 (Aver 0.4, WP5, after v0.3.0): `aver ci keys`, `aver ci --record`, `--base-record` and `--workflow` (section 1.1). No receipt or diagnostics field changes; `summary.md` gains a note when a base record was given.
- 2026-10-17 (Aver 0.4, WP3, after v0.3.0): adding `aver.env/v4`. A check gains `min_tests` (2 or more; a contract's floor) and `approved` (present only as `false`: a check detected in CI or proposed by a head's `aver.toml`); an environment that uses neither keeps its older version and id. `aver ci` is unchanged for a repository with `aver.toml`; without one, CI still detects its checks, but they are now `approved: false`, so the environment is v4 and its id differs from the one an older Aver pinned. Outside CI nothing is detected: `aver verify` and `aver check` exit 2 with `NOT CHECKED` (`ARCHITECTURE.md` §1.13.2), and `aver check -- <cmd>` writes the contract.
- 2026-10-15 (Aver 0.4, after v0.3.0): `aver ci --sandbox` is removed, and checks always run natively; `--home` requires `--run-as`. Receipts written since carry `provenance.mode` `local` or `ci`; `sandbox` and the `docker-*` executors appear only in older receipts. The MCP server is removed; the reference TypeScript for canonical bytes and signing moved to `site/src/core/`.
- 2026-10-14 (round 14, native verify): adding `aver.receipt/v3`.
  - **New version.** Verification writes `aver.receipt/v3`: every v2 field and rule, plus two required signed fields. `provenance`: `{ "mode": "local" | "ci" | "sandbox", "host_profile": hex64, "network": "off" | "on", "uid": "separate" | "same" }`, `host_profile` present exactly for `local` and `ci`, where `executor` is `native-v0.<first 16 hex digits of host_profile>`. `levels`: `{ CheckId: "native" | "junit" | "exit" }` for every check that ran. `schemas/aver.receipt.v3.json` publishes it; `schemas/vectors/v0.json` has a signed v3 sample.
  - **Why a new version.** `local` receipts trust the verifier's machine and the agent (checks ran natively, as the user); a consumer that knows only v2 must refuse them rather than read them as sandboxed. Readers that accept v3 should show the provenance; one that only needs the verdict can treat v3 as v2.
  - **Default change.** `aver verify`, `aver check` and `aver ci` run checks natively; `--sandbox` (`aver ci --sandbox auto|gvisor|docker`) runs them in the Docker sandbox as before, with provenance `sandbox`. The GitHub Action's `sandbox` input defaults to `native`. `aver check` is a dry run of verify (`POST /v0/verify` with `"dry_run": true`, which also takes `"sandbox": true`).

- 2026-10-14 (round 11, dogfood polish): changing `aver.receipt/v2` in place, additively, and adding a report format to `aver.env/v3` in place.
  - **New receipt field.** `skipped` (signed, optional, v2 only, never empty when present): `[{ "check": CheckId, "reason": "not_affected" }]`, sorted by check, the checks the change does not affect, which ran on neither side and have no entry. `schemas/aver.receipt.v2.json` adds it; receipts without it ran every check. A consumer that reads only the verdict needs no change; one that lists checks should list `skipped` as "not run".
  - **Rule change.** A check whose scope (`covers`, else its `cwd`), mounted lockfiles and harness the head does not touch is not run, so a change to documentation alone is `PASS` with no entries instead of running every check (`docs/VERDICTS.md`, "Which checks run"). `aver ci --all` runs every check as before, and its receipt's `reproduce` ends in `--all`. Verdicts of changes that touch every check's scope are unchanged.
  - **New report format.** `tsc` (`aver.env/v3`): one failing entry per TypeScript error, keyed by file, code and message, with absent errors passing. The default type check (no `aver.toml`) uses it, so a base whose type check already fails no longer makes every verdict `BLOCKED_NOT_OWN`; its environment is now `aver.env/v3`.
  - **Vectors.** Unchanged.

- 2026-10-14 (round 10, generic mode): changing `aver.receipt/v2` in place, additively, and adding `aver.env/v3`.
  - **New receipt field.** `uncovered` (signed, optional, v2 only; `Listed<Path>` like the `test_changes` lists, never empty when present): the source files (`.js`, `.ts`, `.py`, `.rs`, `.go` and their variants) the head changed that no check covers. `diagnostics.json` carries it with up to 1000 paths. `schemas/aver.receipt.v2.json` and `aver.diagnostics.v0.json` add it; receipts without it are unchanged.
  - **Rule change.** With `uncovered` present the verdict is `UNPROVEN` where it would have been `PASS` or `REVIEW` (with `unproven` possibly empty); `FAIL_OWN` and `BLOCKED_NOT_OWN` are unchanged. A consumer that treats `UNPROVEN` as "not done" needs no change. In particular, a repository whose `aver.toml` checks one package now gets `UNPROVEN` for a change to another language's files.
  - **Generic mode.** Without `[[check]]`, Aver detects Python, Rust and Go packages and nested Node packages and adds a check for each (`ARCHITECTURE.md` §1.4.1); a repository where it finds nothing beyond the root `package.json` keeps its environment and id. New report formats `libtest`, `go-json` and `pytest-junit`, `[[deps]]` layers and a check's `image`, `deps` and `covers` exist only in `aver.env/v3` (`schemas/aver.env.v3.json`). The `python`, `golang` and `rust` official images join `node` on the pull allowlist. Check sandboxes keep the image's `PATH` and get the offline switches `CARGO_NET_OFFLINE`, `GOPROXY=off`, `GOTOOLCHAIN=local`, `PIP_NO_INDEX` and `UV_OFFLINE`; `/work` and `/tmp` are sized from the memory limit.

- 2026-10-13: changing `aver.receipt/v2` in place, strictly additively: no field is renamed, removed or newly required, and existing fields keep their meaning except for the verdict rule change below.
  - **New enum values.** Receipt entry `cause` (v2 only): `"CHANGED"`. Receipt `verdict` (v2 only): `"REVIEW"`. Diagnostics failure `cause`: `"CHANGED"`. A v0 or v1 receipt carrying either is invalid. In `schemas/`, `aver.receipt.v2.json` inlines its verdict enum with `REVIEW` (the shared `defs` `Verdict`, used by v0 and v1, is unchanged) and both `aver.receipt.v2.json` and `aver.diagnostics.v0.json` add `CHANGED` to `cause`.
  - **New fields.** None in the receipt. In `diagnostics.json` (unsigned), each `failures[]` item may carry `own` (`"pass"`, `"fail"`, `"absent"` or `"infra"`; present on `NEW` and `CHANGED` entries of checks, absent on judging, protected and exit entries): how the head's own version of that test or check fared in the head-tests run; and `file` (a `Path`; present on a `CHANGED` test whose test path names the changed test file): the existing test file the head modified or deleted. `note` may now also explain a `CHANGED` entry or an unbuilt-dependency `ENV` entry. `aver ci` also writes `runs/<check>.<side>.<n>.log`.
  - **Rule changes.** Inputs that used to give `FAIL_OWN` and now give `REVIEW` (when nothing else is `NEW`, `ENV` or inconclusive and no criterion is unproven): (1) every `judging::` entry (a head edit to frozen harness) is `CHANGED` instead of `NEW`; (2) a test whose base copy fails on the head (formerly `NEW`) is `CHANGED` when the head modified or deleted the existing test file that defines it and the head's own version passes or no longer exists (deleted or skipped); if the head's own version fails too it stays `NEW`. A NEW entry becomes `ENV` (so `BLOCKED_NOT_OWN`) when a dependency with an install script that the head's lockfile adds could not be built by the network-less rebuild and the failing run names it; a head importing an undeclared package stays `NEW`. Verdict order is now `FAIL_OWN` > `BLOCKED_NOT_OWN` > `UNPROVEN` > `REVIEW` > `PASS`: `REVIEW` replaces `PASS` when an entry is `CHANGED`, and an `UNPROVEN` receipt stays `UNPROVEN`. With a judging entry and criteria, the verdict is `UNPROVEN` (reason `judging_files_changed`) where it was `FAIL_OWN`, also when the only provers are `FLAKY` (no longer `BLOCKED_NOT_OWN`). The inconclusive rule is unchanged and still outranks `REVIEW`. `attempt.submit` still accepts only `PASS`.
  - **Exit codes.** `--fail-on unexplained` (default) and `fail-own` do not fail on `REVIEW`; `not-pass` does. Suggested GitHub check-run mapping for `REVIEW`: conclusion `neutral`, title "needs maintainer review" (`FAIL_OWN` `failure`, `PASS` `success`, `BLOCKED_NOT_OWN` and `UNPROVEN` as today).
  - **Vectors.** `schemas/vectors/v0.json` did not change: encoding, hashing and signing are unchanged, and its signed v2 receipt (cause `NEW`, verdict `FAIL_OWN`) is still valid under the new rules.
  - Also: `aver ci --fresh`; `summary.md` gains "Needs review: test changes" (each `CHANGED` entry and the bounded diff of its file), says for each `NEW` entry whether the PR's own version of the test passes, and names the default check's runner, reporter and setup steps; `aver run` ranks `PASS` > `UNPROVEN` > `REVIEW` > `BLOCKED_NOT_OWN` > `FAIL_OWN`, fewer `CHANGED` first, treats `REVIEW` as done, and `aver status --json` adds `verifies` and `changed` per attempt. See `docs/VERDICTS.md`.

- 2026-10-13: changing the unreleased `aver.receipt/v2` in place and adding `aver.env/v2`. Verdicts: in v2 an inconclusive entry (a command-level entry failing on both sides, or an entry flaky on the base that failed on every head run) makes the verdict `BLOCKED_NOT_OWN` instead of `PASS`, and its `diagnostics.json` failure carries a `note`. `ci.requested_base` (new, signed, optional): when `--base` is not an ancestor of `--head`, `aver ci` compares with their merge base, records it as `ci.base_commit` and the requested commit as `ci.requested_base`, and says so on stderr and in `summary.md`. Imports in `aver ci` no longer refuse possible secrets or files over 64 MiB: secrets are listed (path, pattern, severity, never the value) in `diagnostics.json` `import.secrets` and `summary.md`, oversize files are imported empty and listed in `import.placeholders`; new `aver ci --strict-secrets` restores rejection; binary files are no longer scanned and `.env.*.example` templates are not secrets. The default check (no `aver.toml`, or no `[[check]]`) adds the runner's reporter for `node --test`, Vitest, Jest, Mocha and tap and runs `prepare` and, when needed, `build` as setup commands (`ARCHITECTURE.md` §1.4.1). `aver.env/v2`: `[env] rebuild` (default true: dependency install scripts run in a network-less sandbox after the install), `[[check]] setup`, and the `jest-json` and `mocha-xunit` report formats. Check sandboxes see `node_modules` as a writable directory of symlinks into the read-only layer. Lines over 4 KiB are cut in run output. The daemon still rejects secrets on import by default (`aver serve --secrets warn`, `aver import --secrets warn`).

- 2026-10-13: changing the unreleased `aver.receipt/v2` in place. Existing test files are no longer judging entries: regressions run the base's copies, a head-tests run of the head's own versions proves criteria (new `head_tests` and `runs.head_tests` on proving entries; the transplant now includes modified test files). `package.json` is split by field (frozen fields give `judging::<path>#<field>`), lockfiles and dependency fields come from the head for head runs only. New signed fields `executor`, `config` and `test_changes`; `summary.md` gains "What this PR did to the tests", the sandbox name, the default-config line and honest key wording. `diagnostics.json` gains `test_changes` and `timings`. New `aver ci --require-config`, a stderr warning when `--sandbox auto` falls back to runc, and a home lock that serializes concurrent `aver ci` and `aver serve` on one home. `--subject`, `ci.base_commit` and `ci.head_commit` are unchanged.
- 2026-10-13: receipts are `aver.receipt/v2` (harness from the base, judging-file entries, the transplant rule, exit entries, own resource exhaustion `NEW`, the `ci` object and a self-contained CI `reproduce`). New: `diagnostics.json`, `aver ci --subject` and `--criteria-sha256`, `aver key`, `[env] tests` in `aver.toml` (`aver.env/v1`), strict base64url decoding. v0 and v1 receipts stay readable and verifiable.
- 2026-10-12: first version.
