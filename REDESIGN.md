# Aver 0.4 Master Specification: 10/10 Version Control for AI Agents

You are refactoring `~/dev/strata` (`crates/aver-core`, `crates/aver`, `action/`).
Read this entire specification before making edits.

**PROCESS & PARALLELISM:**
- Execute the core architectural refactors (WP1 -> WP5) in coordinated sequence so core files (`verify.rs`, `judging.rs`, `rules.rs`) do not suffer merge conflicts.
- You have plenty of session tokens and may freely spawn Claude subagents for parallelizable module work, test writing, real-repository compatibility testing, and live agent evaluations (WP6).
- Set `export CARGO_PROFILE_DEV_DEBUG=0` in your shell so `target/` disk usage stays compact across workspaces and slots.

---

## 0. Architectural Synthesis (What We Are Building)

Aver is **Version Control for AI Agents** (`POSITIONING.md`): every agent gets an instant Copy-on-Write (CoW) workspace, automatic micro-checkpoints with last-green rewind, live semantic collision radar, union-merged parallel combines, and proof-carrying commits; humans keep Git and receive verified branches/PRs with a 30-second review summary.

Today's #1 failure mode is that Aver tries to rebuild each repository's environment itself (`detect/presets.rs`, `exec/native.rs` pools/installs, `exec/sandbox.rs`, `disk/deps.rs`), which broke 24 of 26 real repositories and caused false `PASS` verdicts when 0 tests ran.

**The New Model (Synthesis of Candidate 3 + Candidate 1):**
1. **Stop rebuilding environments.** Aver runs the repository's declared **Contract** (`aver.toml`) in the caller's existing environment (the agent's local shell/tools or the user's CI job after their own `setup-*` and install steps).
2. **Decouple environment from Snapshot identity.** `aver.snapshot/v1` is `{ schema, root: TreeId }` only (`v0` stays readable via `Snapshot::root()`).
3. **Two Execution Places, One Unified Judge Engine (`aver/src/judge/`):**
   - **In CI (`Place::Ci(Workdir)`):** Single-checkout tree swapper (`aver/src/place/workdir.rs`). Adopts the CI checkout prepared by the user's workflow steps, keeps untracked dependency/build dirs (`deps.keep`), swaps tracked files from Git blobs with fresh mtimes across `head-tests -> judged_head -> base (if not cached) -> transplant -> restore`, and records base runs across PRs via `actions/cache`.
   - **Locally (`Place::Local(Sides)`):** CoW slot directories under `$AVER_HOME/sides/<repo16>/{B-<tree16>, J-<ws>, T-<ws>}` (`aver/src/place/sides.rs`). Never mutates the agent's working directory mid-session. Reuses the agent's or checkout's installed dependencies (`node_modules`, `.venv`) via read-only CoW adopt + path relocation (rewriting absolute workspace paths in `.pth`, `direct_url.json`, `.modules.yaml` to the slot path) and `--seed` CoW cloning for `target/`.

---

## 1. The Great Deletion: Cut ~35,000 Lines of Bloat First

### 1.1 Triage Uncommitted Work in `~/dev/strata`
- **KEEP** staged/uncommitted fixes in `judging.rs`, `inline_tests.rs`, `diagnostics.rs`, and `run/report.rs` that freeze Rust doctests (`///` and `//!`) and fix Cargo workspace member path resolution (`tests/x.rs` vs `crates/member/tests/x.rs`).
- **DISCARD** `crates/aver/src/cargo_features.rs` (461 lines). Under the Contract model, the repository's own test command (e.g. `cargo test --all-features` or `make test-full`) runs directly.

### 1.2 Subsystems to Delete Completely
1. **Delete the MCP Server (`mcp/`):** Delete the entire `mcp/` TypeScript directory and `crates/aver/src/cli/mcp.rs`. Agents use the `aver` CLI directly via Bash, removing the Node >=24.2 dependency that caused `DEPS_FAILED` across workspaces and removing the redundant 23-tool file-editing API.
2. **Delete Docker Sandboxes & gVisor:** Delete `crates/aver/src/exec/sandbox.rs`, `exec/materialize.rs`, `exec/deps.rs`, `exec/slots.rs`, `action/install-gvisor.sh`, and tests `sandbox_security.rs`, `generic_docker.rs`, `exec_docker.rs`.
3. **Delete the Native Environment Rebuilder & Pools:** Delete `crates/aver/src/detect.rs`, `detect/presets.rs`, `detect/languages.rs`, `exec/netns.rs`, `disk/deps.rs`, `disk/cargo.rs`, `disk/pools.rs`, `disk/usage.rs`, and `tests/native_env.rs`. Keep only `detect/suggest.rs`'s CI/Makefile/package.json test-command extractor (as a candidate suggester when no contract exists) and `adhoc.rs`'s runner-reporter flags.
4. **Delete the Legacy Preview Engine:** Delete `crates/aver/src/check.rs` (3,309 lines). `aver check` and `aver verify` now share one unified judge engine (`crates/aver/src/judge/`).
5. **Remove the Background Daemon Requirement:** Make `cli/ops.rs` execute in-process by default using SQLite `WAL` mode (`busy_timeout = 30000`) and per-slot filesystem `flock`s. Every `aver` command inherits the caller's live shell `PATH` and environment directly, and `Ctrl-C` kills child test processes immediately without orphaned daemon builds or 15-minute lock hangs.
6. **Delete 7 of 10 Claude Hooks & Editor Spam:** In `cli/integrate.rs` and `cli/hook/`, remove `pre-edit`, `post-edit`, `pre-bash`, `post-bash`, `pre-agent`, and unrequested `.vscode`/Copilot config writers (`aver setup` only configures tools explicitly present or requested). Do NOT install any Git hooks (`pre-commit`, `post-commit`, `pre-push`). Keep only:
   - `SessionStart` (injects a 75-word workspace brief).
   - `Stop` / `SubagentStop` (checks if `aver verify` passed on the current workspace tree before letting the agent stop).
   - `WorktreeCreate` / `WorktreeRemove` (for `claude --worktree`).

---

## 2. Core Data Model (`crates/aver-core/src/`)

### 2.1 Decouple Environment from Snapshot (`aver-core/src/snapshot.rs`)
- Bump to `aver.snapshot/v1`:
  ```rust
  pub struct Snapshot {
      pub schema: SnapshotSchema, // "aver.snapshot/v1"
      pub root: TreeId,
  }
  ```
- Keep `v0` (`{ root, env }`) deserializable; `Snapshot::root(&self) -> TreeId` returns `root` for both.

### 2.2 The Repository Test Contract (`aver-core/src/contract.rs`)
Replaces `Environment` for new runs. Parsed from `aver.toml` (include a deterministic backward-compatible fallback reader for v0–v3 `aver.toml` files so all existing fixtures pass, plus `aver migrate`):
```rust
pub struct Contract {
    pub schema: ContractSchema,        // "aver.contract/v0"
    pub checks: Set<ContractCheck>,    // 1..=32, unique ids
    pub deps: Deps,
    pub judge: JudgeGlobs,
    pub ci: CiPolicy,
}

pub struct ContractCheck {
    pub id: CheckId,
    pub run: ShellLine,                // run via `sh -c` in cwd; $AVER_OUT injected
    pub cwd: Option<Path>,
    pub prepare: Option<ShellLine>,    // optional setup/build command run before each side
    pub report: ReportSpec,            // Junit (*.xml in $AVER_OUT) | Stdout(Libtest|GoJson|Tsc|JestJson) | Exit
    pub min_tests: Option<u32>,        // 90% of base test count when approved; required unless Exit
    pub paths: Option<Set<PathGlob>>,  // affected-check selection; None = always run
    pub serial: bool,                  // default true when services/ports used; runs sides sequentially
    pub timeout_s: Option<u32>,
}

pub struct Deps {
    pub keep: Set<PathGlob>,           // e.g. ["node_modules", ".venv", "target"]
    pub seed: Set<PathGlob>,           // CoW-cloned once per slot (e.g. ["target"])
    pub install: Option<ShellLine>,    // run only when `inputs` differ between base and head
    pub inputs: Set<PathGlob>,         // e.g. ["package-lock.json", "pnpm-lock.yaml", "uv.lock", "Cargo.lock"]
}

pub struct JudgeGlobs {
    pub tests: Set<PathGlob>,          // added to default test globs
    pub harness: Set<PathGlob>,        // added to default frozen harness globs
    pub built_tests: Set<PathGlob>,    // default: ["dist/**/test/**", "build/**/test/**", "**/*.test.js"]
    pub protected: Set<PathGlob>,      // read ONLY from base aver.toml, never from PR body
}

pub struct CiPolicy {
    pub review: ReviewPolicy,          // Approve (default: blocks until non-author human approves) | Neutral
    pub require_recorded_base: bool,
}

pub enum Approval {
    Base { blob: BlobId },             // from base tree's aver.toml (human merged)
    Proposed { blob: BlobId },         // from head tree's aver.toml on bootstrap PR
    Adhoc { line: ShellLine },         // from local `aver check -- <cmd>`
}

pub enum Trust {
    Trusted,                           // Approval::Base && Place::Ci && no unapproved workflow edits
    Advisory(AdvisoryKind),            // Local | Proposed | Adhoc
}
```

### 2.3 Run Cache Key (`aver-core/src/run.rs`)
- Key runs by `RunKey { tree: TreeId, check: ContractCheckId, place: PlaceKey, deps: DepsState, index: RunIndex }`.
- `PlaceKey::Host(HostKey)` hashes the OS, arch, and the realpath + `--version` hash of tools invoked by the check so a cached base run from a different machine/toolchain is never reused.
- Never let a host-probing test (e.g., free disk space check) cause a false `NEW` against an older cached base.

---

## 3. Execution Places (`crates/aver/src/place/`)

Extract the ~700 lines of generic process management from `exec/native.rs` (process group spawn, timeout, descendant reaping, bounded output keeping head + tail so Python tracebacks never cut off the final exception line, `$AVER_OUT` directory creation, and report parsing) into `crates/aver/src/place/proc.rs`.
Commands inherit the caller's environment (`PATH`, `HOME`, `HTTPS_PROXY`, `SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`, `VIRTUAL_ENV`, etc.) unchanged, with `AVER_OUT=<fresh_dir>` added and GitHub Actions token/step-output vars (`GITHUB_OUTPUT`, `GITHUB_ENV`, `GITHUB_PATH`, `GITHUB_STEP_SUMMARY`, `ACTIONS_RUNTIME_TOKEN`, `ACTIONS_CACHE_URL`, `ACTIONS_ID_TOKEN_REQUEST_*`) stripped.

### 3.1 `place/workdir.rs` (Single-Directory Tree Swapper for CI and Slots)
```rust
pub struct Workdir {
    root: PathBuf,
    keep: Set<PathGlob>,
    at: TreeId,
    journal: PathBuf, // .git/aver-swap.json
}
```
- `adopt(root, git, head, keep)`: Verifies tracked files equal `head` (else `Blocker::DirtyCheckout`) and deletes untracked files outside `keep` and `.aver`.
- `become_tree(&mut self, tree)`: Rewrites only differing tracked files from blobs with fresh `mtime`s, deletes untracked files outside `keep` (so build outputs like `dist/` never leak between sides), and writes `.git/aver-swap.json`.
- `derived(&self, globs)`: Hashes untracked files written during the run that match `judge.tests` or `judge.built_tests`.
- `probe_copies(&self, changed)`: Checks `keep` directories for non-editable copies of changed files (e.g. `.venv/.../site-packages/pkg/foo.py` matching head's `src/pkg/foo.py` -> `Blocker::StaleCopy`).
- `restore(root)`: Restores the adopted head tree byte-for-byte from `.git/aver-swap.json`.

### 3.2 `place/sides.rs` (Local CoW Slots + Path Relocation)
- Locally, `aver check` and `aver verify` never mutate the agent's working directory. They prepare slot directories under `$AVER_HOME/sides/<repo16>/{B-<tree16>, J-<ws>, T-<ws>}` (or run sequentially when `serial = true`).
- When creating a workspace (`aver workspace new`) or preparing slots, CoW-clone (`clonefile` on macOS, `reflink` on Linux) or symlink `deps.keep` (`node_modules`, `.venv`) and CoW-seed `deps.seed` (`target`) from the user's checkout or agent workspace when `deps.inputs` match.
- **Relocation (`relocate`):** Scan text metadata files in shared `.venv` / `node_modules` (`*.pth`, `direct_url.json`, `.modules.yaml`) for the source directory's absolute path and rewrite those files per slot so editable installs resolve to the slot's own source tree, not the agent's workspace. Preserve relative symlinks (`pnpm` workspace links).

---

## 4. The 6 Hard Verdict Invariants (`aver-core/src/evidence.rs` & `rules.rs`)

1. **Zero Evidence is NEVER `PASS` (`Blocker` -> `BLOCKED`, exit 1):**
   - Compute `SideCount { ran, passed, failed, skipped }` per check.
   - Emit a `Blocker` (`BLOCKED_NOT_OWN` / `BLOCKED`, exit 1 in both local verify and `aver ci`) whenever:
     - `base.ran == 0` or `head.ran == 0` (unless `report == Exit` was explicitly approved in base `aver.toml`).
     - `base.ran < check.min_tests`.
     - `base.ran > 0 && base.passed == 0` (`AllRed`).
     - `pytest` exits with codes 2, 3, or 4 (internal/usage/config/collection error) or a Go package stream ends in a `panic` / `FAIL <pkg>` with truncated tests (compare declared `Test*` functions in the package against reported ones).
     - `install` or `prepare` fails (`SetupFailed`), or `probe_copies` detects a stale non-editable package copy (`StaleCopy`).
2. **Deleting or Skipping a Failing Test is `FAIL_OWN` (Exit 1), Not `REVIEW`:**
   - When an existing test is deleted or skipped in `head_tests`, check its outcome on `judged_head` (which runs the base's frozen test file against head code).
   - If the base's test **fails** on `judged_head`, classify it as **`NEW` (`FAIL_OWN`, exit 1)**: *"you deleted/skipped `<test>`, which fails on your change"*.
   - If an agent skips an unrelated **passing** test (`base == pass`, `judged_head == pass`, `head_tests == skip`), classify it as **`CHANGED` (`REVIEW`)** (never silent `PASS`), and penalize it in `aver run` ranking behind clean `PASS` attempts.
3. **Compiled / Derived Test Fingerprinting (`built::<path>` Closes G8 & G8b):**
   - Compare `Workdir::derived` hashes between `base` and `judged_head`. Because both compile identical frozen test sources, any difference in compiled test files (`dist-node/test/*.js`) produces a `built::<path>` entry with cause `CHANGED` ($\rightarrow$ `REVIEW` with diff).
4. **Rust & TypeScript API Changes on Frozen Inline Tests (`CHANGED`, Not `FAIL_OWN`):**
   - Freeze Rust `///` and `//!` doctests in `inline_tests.rs` just like `#[cfg(test)]` blocks.
   - Resolve `libtest` target paths against Cargo workspace member directories in `verify.rs` `EntryContext::defines`.
   - Pass `--keep-going` to `cargo test` so one non-compiling test target does not hide per-test results from other targets.
   - When a base test target or inline `#[cfg(test)]` block fails to compile on `judged_head` because the head changed a struct/function signature, check `head_tests`: if the head's updated version of that test target compiles and its tests pass (or match `base` per-test failures), classify that target as **`CHANGED` (`REVIEW`)**, never command-level `NEW` (`FAIL_OWN`).
   - In `aver-core/src/tag.rs`, match `crit_<id>` as a whole word anywhere in a test name (prefix `crit_foo_...` or suffix `..._crit_foo`).
5. **Dynamic Test IDs, Agent-Added Flakes & 1× Transplant:**
   - Replace slot/workdir absolute paths in test IDs with `<src>` before comparison (fixes `requests` `__file__` IDs).
   - If a test ID in an **untouched** test file disappears and the same test function appears with a new parameter/subtest suffix (timestamps in `marshmallow`, pointers/random ports in `testify`/`ky`), pair them and run `confirm_fresh` (1 fresh base run) rather than reporting `NEW` ("missing from head report").
   - Any test added by the head (`split.added_tests`) that is flaky across runs is classified **`NEW` (`FAIL_OWN`)**, never `FLAKY ("not yours")`.
   - Run the `transplant` side **once** (`runs = 1`); never rerun expected failures on the transplant 3×. Never rerun a base side 3× when it fails the exact same way at command/compile level.
6. **Weak Proof Detection (`WeakProof` -> `UNPROVEN`):**
   - Extract modified function/method/type identifiers from diff hunk headers (`git diff` `xfuncname` heuristic). If a criterion's proving test body contains none of those identifiers (or failed on the transplant only due to a missing import/module/build/undefined symbol), mark it `UNPROVEN (weak_proof)` with the 1-line pushback: *"your test `<test>` never calls `<fn_name>`, the code you changed; make it call and assert on `<fn_name>`."*

---

## 5. First-Line Diagnostics, Dialogue & Full Agent VCS Superpowers

### 5.1 First-Line Counts, Raw Error Tails & Report Hygiene (`run/report.rs`, `ci/summary.rs`)
- Every CLI report and `summary.md` must lead with the verdict and exact test counts:
  `Base: 1,204 passed, 0 failed | Yours: 1,206 passed, 0 failed`
- Whenever any check is `BLOCKED`, `ENV`, or fails at command level, print the **last 15 lines of the command's stderr/stdout** inline right under the headline in the plain-text report. Never print `"a sandbox timed out"` or `"claimed_test_not_found"` when a command failed or ran exit-only.
- **Report & CLI Hygiene:**
  - Deduplicate `node:test` assertion failure blocks so each failure prints once (not 3×) and strip internal slot paths.
  - Unescape HTML entities (`&quot;` -> `"`) in JUnit test names and preserve file/class names in `pytest` JUnit entries.
  - Handle `SIGPIPE` / `BrokenPipe` cleanly in CLI stdout writes so `aver verify | head` exits 0/1 instead of panicking with exit 101.
  - Allow `aver verify --last --json` to print the cached JSON report without error.
  - Scope `aver status`, `aver diff`, `aver accept`, and `aver discard` to the **current repository** by default; add a `repo` column to `aver status --all` and human-readable labels/titles to `aver workspace ls`.

### 5.2 First-Run Contract Bootstrap & Targeted Pushbacks (`aver/src/dialogue.rs`, `aver-core/src/pushback.rs`)
- If a repo has no `aver.toml`, running `aver check` or `aver verify` prints:
  ```
  NOT CHECKED: Aver does not know this repository's test command yet.
  Run once: aver check -- <the test command you use>
  ```
  (listing up to 3 candidate commands extracted from `.github/workflows/*.yml`, `Makefile`, or `package.json`).
- When the agent or user runs `aver check -- <cmd>`:
  - Reuse `adhoc::check_for` to auto-attach JUnit (`--junitxml=$AVER_OUT/junit.xml`, `--reporter=junit`, etc.), `libtest`, or `go-json` reporters, and split compound `runner && tsc` scripts into two checks.
  - Run on the agent copy and base slot.
  - If `0` tests ran or all failed on base, print the 1-line refusal + stderr tail and write nothing.
  - If `>0` tests ran, save `aver.toml` in the workspace (`min_tests = 90% of base.ran`, `deps.keep` detected from workspace `node_modules`/`.venv`/`target`) and print the report.
- When `aver check <crit>` is called with a criterion ID, use `adhoc::narrowed` to run **only** the test file/target claiming `<crit>` so the inner loop finishes in seconds.
- **Targeted Pushbacks (`aver-core/src/pushback.rs`):** Emit at most one precise 1-sentence pushback at the top of `aver check` / `aver verify` when triggered (`Breaks`, `WeakProof`, `FlakyAdded`, `Skipped`, `ZeroEvidence`, `MissingEnv`, `Overlap`, `NoContract`).

### 5.3 The 12 Agent VCS & Git Targets (`workspace/`, `run/`, `coord.rs`, `disk/gitview.rs`)
1. **Fixture Secret Policy (`cli/workspace.rs:201`):** Change `SecretPolicy::Reject` to `SecretPolicy::Warn` on low-severity test/fixture paths (`tests/`, `testdata/`, `*fixture*`, `.env` templates), matching `aver ci`.
2. **Automatic Micro-Checkpointing & Rewind (`workspace/mod.rs`, `cli/restore.rs`):**
   - Every `aver check` and `aver verify` automatically records a lightweight content-addressed micro-checkpoint in the workspace history annotated with its test delta (`+N passing, -M NEW failures`).
   - Add `aver restore --last-green` to roll back the working tree in `<50 ms` to the most recent checkpoint with `0` `NEW` failures (while keeping `aver restore --undo` to step back/forward).
3. **Git View Remote (`disk/gitview.rs`):** Set the bare view repository `$AVER_HOME/git/<ws>`'s `origin` remote to the user's main checkout path so `git push` and `git fetch` work natively.
4. **Non-Conflicting Fetch Ref (`cli/workspace.rs`, Review M4):** When a workspace has unfetched commits in its Git view after `aver accept`, `aver workspace rm` must print a fetch command targeting `refs/heads/aver/<dir>-commits` (never the branch `aver/<dir>` that `accept` just wrote, which Git rejects as `non-fast-forward`).
5. **Human-Ready Commit Messages, Branch Names & PR Bodies (`run/accept.rs`, Review L6):**
   - Priority order for commit subject on `aver accept`: (1) `--title` flag if passed, (2) the agent's latest Git commit subject in the workspace view if the agent committed, (3) the task title or first criterion text (`fix: <criterion text>`). Never use `"Work on <repo> at <sha>"` when any of those exist.
   - Priority order for branch name: `aver/<slugified-title-or-criterion>-<4char-id>` (preventing cross-batch branch collisions and stale branch reuse).
   - Embed the full `summary.md` evidence block (test counts, criteria proven + proving test assertion lines, and `CHANGED` test diffs) into the commit body and `--pr` body.
6. **Close `REVIEW` Self-Accept (`run/accept.rs`):** `aver accept` must exit `2` on a `REVIEW` verdict when invoked under an agent key (`ws-*`, `run-*`) or without `--approve-review`. Never print `DONE: REVIEW` or `Hand-off: aver accept` in agent reports; print `NEEDS HUMAN REVIEW: REVIEW`.
7. **Union Merge for Appended Tests (`workspace/merge.rs`):** Implement `MergeRule::UnionInsertions`. When two tasks insert lines at the same base anchor without deleting/modifying base lines, merge both insertions in task order and verify the combined tree against all tasks in `aver combine`.
8. **Solo Workspaces in `aver combine` (`run/combine.rs`):** Allow `aver combine` to accept workspace labels, attempt IDs, or branch names from `aver workspace new` / `claude --worktree`, not just `aver run` batches.
9. **Greedy `aver combine --partial` (Review M2):** Keep tasks in number order whenever task `i` merges and verifies cleanly with the subset kept so far; never exclude task `i` for conflicting with an already-excluded task `j < i`; export the branch even if 1 task remains; record every task in `combine.json`; print each failure once (Review L2).
10. **Hand-Merged `workspace rebase`, Stale Notice Pruning & Semantic Collision Radar (`workspace/mod.rs`, `coord.rs`):**
    - Write standard conflict markers into workspace files on a conflicting `workspace rebase` and accept the resolved working tree on the next `rebase` / `check`.
    - Exclude `FAIL_OWN`, discarded, accepted, and superseded (`rerun`) workspaces (and their leases/directories) from `aver notices` and `aver check --combined`, while keeping accepted tasks in batch overlap summaries (Review L1).
    - **Semantic Collision Radar:** In `aver check --combined` and `aver notices`, use `UnionInsertions` on the trial merge and warn if another active task's changes break this task's proving tests even when they touch different files.
    - Delete the `materializations` row on `workspace rm` so reusing `--label X` never hits `sqlite: UNIQUE constraint failed`.
11. **Multi-Attempt Ranking, Cross-Check & Budget Guards (`run/supervisor.rs`, `aver-core/src/rank.rs`):**
    - **Cross-check file collision fix:** When running the cross-check in `aver run -n K`, if two competing attempts both added a new test file with the same path (e.g. `tests/help.negate-default.test.js`), rename the donor's added test file to a temporary unique test filename (`tests/help.negate-default.aver-cross.test.js`) instead of skipping the cross-check.
    - **Hard verify budget & resume guard:** Enforce `max_verifies` strictly (refuse verify #11 of 10 with exit 2), penalize over-budget or scratch-file-polluted attempts in ranking, and stop `--resume-cmd` immediately if an agent exits twice without changing any file.
12. **Prompt-Injection Fencing for Issue Criteria (`run/brief.rs`, `REDTEAM3` #1):**
    - Wrap all GitHub issue titles and issue-sourced criteria in `.aver/BRIEF.md` inside `<repo-data>` fences using `run::report::fence` escaping so hostile community issues cannot prompt-inject the coding agent during `aver run --issues`.

---

## 6. Benchmark & Verification Suite (Must Pass 100%)

Build and run both `crates/aver/tests/verdict_integrity.rs` and `bench/verify-10-of-10.sh`:

| # | Benchmark / Target | What It Tests | Pass Criterion |
|---|---|---|---|
| **B1** | **Zero-Tests Blocker (`uuid` repro)** | `npm test` exits 0 but runs 0 tests (`if [ -d .git ]` skipped). | `aver verify` & `aver ci` return `BLOCKED` (**exit 1**), never `PASS`. Report line 1 prints `0 tests ran`. |
| **B2** | **Runner Crash Blocker (`click` repro)** | Test command exits 4 (`Unknown config option: cache_dir`) on both base and head. | `aver verify` & `aver ci` return `BLOCKED` (**exit 1**), printing the `cache_dir` stderr line in plain text. |
| **B3** | **Build-Output Tamper (`G8b` repro)** | Head breaks `v4()` and `build.mjs` `sed`-rewrites compiled `dist/test/v4.test.js`. | `Workdir::derived` flags `built::dist/test/v4.test.js` as `CHANGED` -> `REVIEW` (**exit 3** in CI, never `PASS`). |
| **B4** | **Delete/Skip Failing Test (`H2` repro)** | Head breaks `add()` and deletes or `it.skip`s the failing test. | Judged head runs base test, sees failure, returns **`FAIL_OWN` (exit 1)**—not `REVIEW` (exit 0). |
| **B5** | **Agent `REVIEW` Self-Accept Guard** | Head makes a valid expectation edit (`REVIEW`), then agent key runs `aver accept`. | `aver accept` refuses with **exit 2** until human runs `aver accept --approve-review`. |
| **B6** | **Weak Proof Detection (`M1` / `noisy-ledger`)** | Head adds `negate(x) { return x; }` and test `typeof negate === 'function'` (or test that never calls changed fn). | `aver verify` returns **`UNPROVEN (weak_proof)`** with the 1-line function-call pushback. |
| **B7** | **Rust Doctest & Struct-Change Handling** | (a) Break fn and rewrite `///` doctest; (b) Add field to pub struct and update inline `#[cfg(test)]` literal. | (a) Doctest splice catches regression as **`FAIL_OWN`**; (b) Struct change classifies as **`REVIEW` (`CHANGED`)**, not command-level `FAIL_OWN`. |
| **B8** | **Dynamic Test IDs (`marshmallow` / `requests`)** | Test parameterized with current timestamp and `__file__` absolute path; head makes docstring-only edit. | Path normalized to `<src>` + `confirm_fresh` rerun -> **`PASS` (exit 0)**, 0 false `NEW` or `REVIEW`. |
| **B9** | **Workspace Speed, Rewind & Fixture Secrets** | Repo containing `tests/fixtures/key.pem` and `.env`; create 5 workspaces, make green edit, make breaking edit, run `aver restore --last-green`. | Succeeds with `Warn` (no `IMPORT_REJECTED`); warm workspace creation **< 100 ms**; `--last-green` restores passing tree in **< 50 ms**. |
| **B10** | **Parallel Test-Append Combine (`UnionInsertions`)** | 4 solo `workspace new` workspaces each append a new test to the end of `test/math.test.js` and fix a function. | Single `aver combine ws1 ws2 ws3 ws4` merges all 4 via union merge, verifies all 4 criteria, and exports 1 branch (**0 reruns**). |
| **B11** | **Git Targets (Messages, Branches, Remotes, Fetch)** | Agent commits `fix: handle negative zero` in workspace view, then human runs `aver accept`. | Exported branch is `aver/fix-handle-negative-zero-<id>`, commit subject is `fix: handle negative zero`, body has `summary.md` table, and `git push origin` works inside workspace. |
| **B12** | **Cross-Check & Issue Fencing** | (a) `aver run -n 2` where both attempts add `tests/new.test.js`, #1 is buggy, #2 catches it; (b) Issue body contains `</repo-data>` injection. | (a) Cross-check renames donor test file, runs #2's test against #1, and ranks #2 first; (b) `BRIEF.md` escapes the tag cleanly. |

---

## 7. Execution Sequence

1. **WP1 (Deletion & Core Schemas):** Execute §1 deletions. Implement `aver-core/src/{contract,evidence,pushback}.rs` and `Snapshot v1`. Gate: `cargo test -p aver-core`.
2. **WP2 (In-Process Runner & Places):** Implement `place/{proc,workdir,sides}.rs` (`adopt`, `become_tree`, `derived`, `probe_copies`, `relocate`, `restore`) and make `cli/ops.rs` run in-process without a daemon. Gate: `cargo test --test place`.
3. **WP3 (Unified Judge & 6 Hard Invariants):** Wire `judge/{plan,run,finish}.rs` into `verify.rs` and `cli/check.rs`. Implement all 6 Hard Invariants (§4), first-line counts + 15-line stderr tails + report hygiene (§5.1), and `aver check -- <cmd>` contract bootstrap (§5.2). Gate: `cargo test --test verdict_integrity`.
4. **WP4 (Git Targets & Multi-Agent VCS):** Implement all 12 VCS & Git targets in §5.3 (`Warn` on fixture secrets, micro-checkpoints + `aver restore --last-green`, `origin` remote, M4 fetch ref, commit titles/PR body, `--approve-review`, `UnionInsertions`, solo `combine`, `--partial` greedy fix, `workspace rebase`, cross-check temp file rename, verify budget/resume guard, and `BRIEF.md` issue fencing). Gate: `cargo test --test combine` and `cargo test --test workspace`.
5. **WP5 (CI Action & Cross-PR Cache):** Update `ci/` and `action/` for `merge_group`, `aver ci keys` (`actions/cache`), `.github/workflows/**` freezing, and `mode: approve`. Gate: `sh action/test/run.sh`.
6. **WP6 (Subagent Compat Matrix, Live Agent Evals & Launch Benchmark):**
   - Run `bench/verify-10-of-10.sh` (B1–B12) and `cargo test --workspace` until 100% green.
   - Spawn parallel Claude subagents to test the 26 OSS compatibility repositories (`uuid`, `commander`, `express`, `chalk`, `ky`, `hono`, `date-fns`, `swr`, `click`, `attrs`, `requests`, `tqdm`, `starlette`, `flask`, `typer`, `packaging`, `black`, `cobra`, `gin`, `testify`, `chi`, `fiber`, `fzf`, `anyhow`, `serde_json`, `ripgrep`, `clap`, `fd`, `just`) and verify **>=24/26** reach accurate verdicts on good, regression, and test-gaming patches.
   - Spawn live Claude subagents on the `EVALS.md` hard/falsedone scenarios (`noisy-ledger`, `three-way-overlap`, `fd-stale-search`) to verify **0% false done** and a **9.5+/10** agent rating.

---

## 8. Execution Notes (coordinator log)

These notes record how the sequence above is being carried out. The specification above is unchanged.

- **Integration branch.** All work lands on `redesign-0.4`, cut from `main` at `2980abe`. `main` stays shippable until WP6 passes. Each work package is its own branch off `redesign-0.4`, merged back after its gate.
- **§1.1 triage.** The doctest and Cargo member-path fixes were already committed in the `aver-rust-compat` merge (`2980abe`); there was no uncommitted work to keep. `cargo_features.rs` is deleted in WP1b.
- **Deletions move with their replacements.** WP1 deletes what has no replacement (MCP, the Docker sandbox and gVisor, `cargo_features.rs`, the five Claude hooks and editor writers). The native environment rebuilder (§1.2.3) is deleted in WP2 together with `place/`, because §3 extracts `place/proc.rs` from `exec/native.rs`. `check.rs` (§1.2.4) and `detect.rs` are deleted in WP3 together with `judge/` and the contract bootstrap, so `aver check` and the fixtures without `aver.toml` keep working between packages. The daemon removal (§1.2.5) is WP2.
- **Unmerged branches.** `claude/aver-newcomer-fixes` and the go-compat work (`fb/go-compat`) overlap §4.1, §4.5 and §5.3.1. They are not merged; their tests are mined as fixtures for WP3 and WP4.
- **Disk.** On 2026-10-15 the machine had 15 GiB free. The stale native pools under `~/.aver/native` and `target/debug` were removed (66 GiB free after). Agents build with `CARGO_PROFILE_DEV_DEBUG=0`, and at most two code-writing agents build at once.

### Carried forward from WP1

- WP3: the Evidence and Weak proofs paragraph of ARCHITECTURE.md §1.9.4 is garbled on `main` ("entroof", "something_namexists"); rewrite it with the weak-proof rules.
- WP3: `verify.rs`'s unbuilt-dependency rule (rule 7) and `REBUILD_FAILED` are dead since the sandbox went; delete them with the judge. `check.rs`'s "the sandbox" wording goes with `check.rs`.
- WP3: the root `aver.toml` rust check does not build `examples/`, so an example-only edit verifies UNPROVEN; the converted contract should cover it. Its sandbox-memory comments are stale.
- WP3: `contract.rs` converts legacy checks with `min_tests = 1`, and `/aver/out` becomes unquoted `$AVER_OUT`; pytest-junit and mocha-xunit read as plain JUnit.
- WP2: the exec routes return 503 on the real daemon; they go with the daemon.
- WP5/WP6: `bench/eval/run.ts`, `bench/replay/replay.ts` and `ci/summary.rs`'s Docker and gVisor strings.
- `SKILL.md` still says `crit_neg_two` carries `neg-two`, not `neg`; true now only when the task has both criteria.

### WP3 decisions that depart from the specification

- **B7(a).** A rewritten `///` doctest is `CHANGED` (`REVIEW`), not `FAIL_OWN`, by the owner's decision of 2026-10-16 (docs/redesign/wp3-plan.md §1, decision 1): a rewritten test is for a human to read. A doctest the change breaks without rewriting it is `FAIL_OWN`. Either way it never passes (`verdict_integrity::b7a_a_doctest_is_judged_from_the_base`).
- **§4.6 "never calls `f`".** Not built. A test that fails on the original code on an assertion and passes with the change proves its criterion even when it reaches the changed function through a wrapper, so a name match on the test body would mark real proofs weak. A weak proof is one whose transplant failure is only a missing module, a missing name the change adds, or a build error; the pushback names the missing name (`Pushback::MissingName`).
- **§4.1 floors.** `min_tests` applies to the base only: a head that runs fewer tests than the base already has a `NEW` entry per lost test, which outranks any blocker. Type checks are exempt. In zero-config CI a detected check is approved only when it runs a script the repository's own `package.json` declares.

### WP4 decisions that depart from the specification

- **Who is an agent (§5.3.6).** The CLI cannot tell a person's shell from an agent's by key: a run accept signs with the admin key and a solo accept's owner key is always `ws-*`. `--approve-review` is refused when `AVER_RUN`, `AVER_WORKSPACE`, `CLAUDECODE` or `CODEX_SANDBOX` is set (`run::accept::AGENT_ENV`); a REVIEW is refused without the flag for everyone. A person running `aver` from inside Claude Code approves from another terminal.
- **Approvals are bound to one receipt.** `--approve-review` approves the REVIEW receipt the accept verified for the exported snapshot, recorded in the accept journal and as an `Aver-Approved-Review:` trailer; a resumed accept honours it only for that receipt and snapshot. The CI half (`mode: approve` bound to the head commit and tree) is WP5.
- **Caps (§5.2 holes).** A head-proposed contract caps `PASS` at `REVIEW`. With no check that ran the repository's tests on the base (type checks only) `PASS`/`REVIEW` become `UNPROVEN` (`no_runtime_tests`). An approved exit-only check counts as running them: it is named under "What Aver could not check", not capped. `aver check -- <cmd>` writes an exit-only check only with `--allow-exit-only`.
- **Sensitive paths.** `[judge] sensitive` (default `.github/workflows/**`, `.github/actions/**`), read from the base's contract only: each changed match is a `sensitive::<path>` `CHANGED` entry (REVIEW). The contract schema is `aver.contract/v1`.
- **Fixture secrets (§5.3.1).** Not `Warn` everywhere: `SecretPolicy::RejectHigh` warns on low-severity paths (fixtures, testdata, templates, test files, docs) and still refuses high-severity findings.
- **Label reuse.** A non-live `materializations` row holding the same directory is deleted when a new one is inserted, rather than on `rm`, which also heals homes with stale rows and keeps K4's release order.
- **Run branches** take the task title, not the agent's commit subject, because `aver combine` predicts a run's branch before it accepts it; the commit subject still prefers the agent's commit.
- **Union insertions** apply to `aver combine` and `aver check --combined`; `aver workspace rebase` and the notice classifier stay strict, so a rebase marks same-place insertions as a conflict.
- **Superseded reruns** abandon the old run's open attempts once the new run is recorded (no schema change), so notices and trial merges drop them.
- **Gate names.** §7 names `cargo test --test combine` and `--test workspace`; those binaries do not exist. The gate is `run_combine`, `combine_tools`, `combine_solo`, `workspace_rebase`, `workspace_lifecycle`, `cli_workspace`, `accept_workspace` within the full suite. B12 is gated by `verdict_integrity.rs` alone: it is an `aver run` property, not a verdict repeated ten times.
- **Timing targets (B9)** are measured, not asserted: `restore --last-green` about 31 ms with the process in a debug build; warm `workspace new` 60 to 114 ms per directory, above the 100 ms target.

### WP5 decisions that depart from the specification

- **Adopt stashes, never deletes (§3.1).** `aver ci` in place moves the checkout's untracked files outside `deps.keep` into `.git/aver-stash/` (journaled first) and `restore` returns them, so a local `aver ci` in a developer's checkout destroys nothing. The swap lock and run record live in the home, not `.git`.
- **Run order in place** was the serial executor's queue (the warm runs the base first), not head-tests, judged head, base, transplant: every run swaps to its own tree and drops unkept build output. WP6 groups it by code (below). A base run after any head code first deletes the untracked `deps.seed` directories (`target`), so the head cannot shape what the base sees; keep directories are withheld from the run user under `--run-as`. The transplant is not scrubbed: its tests are the head's own anyway.
- **Regression-only is a fail-on mode.** `--fail-on regressions` is `unexplained` without exit 3; `--regression-only` sets it and reads no criteria. The action uses it on `merge_group`.
- **Records are the head warmed as a base.** `aver ci --record` runs the head's checks as a later job's base would and writes those runs, so they are keyed exactly as a pull request's base lookups; a push may run a check once more. merge_group records are saved but scoped to the queue's ref by GitHub, so pull requests restore the record of the push that lands. The fresh confirmation covers tests the record's base passed and the head fails; a test the recorded base failed is judged from the record like any cached run.
- **The CI approval is unsigned** (`aver-out/approval.json`: repository, pull request, reviewer, review id, head commit and tree, receipt when given), not a new signed object: the CI key is throwaway, and the review it names is GitHub's own record. It binds only a reviewer with write access (`author_association` `OWNER`, `MEMBER` or `COLLABORATOR`), not the author, not a bot, on the current head commit, as the reviewer's latest decision. `review: approve` posts a check run named `Aver` (`Aver (<check>)` per matrix job) that branch protection requires instead of the job.
- **The TTY guard** (`--approve-review` needs a terminal on stdin) is a speed bump: `AVER_TEST_HUMAN=1` stands in for a terminal in tests and scripts and never lifts the agent-environment refusal.
- **Import memo.** A warm `aver workspace new` reuses the import of a commit already recorded in the home (`import_memo`, store schema v8) when the environment it derives and the secret policy agree, and writes the Git view's config in Rust: no Git process on the warm path.

### WP6 speed decisions

- **Home records (package records).** A local `aver ci` (none of `CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `BUILDKITE`, `CIRCLECI`, `JENKINS_URL`, `TF_BUILD`, `TEAMCITY_VERSION` set; no `--record` or `--base-record`) reads its base from `$AVER_HOME/records/<cache key>` and writes the base runs it made or loaded back there, in the CI record format, under the base's own identity (the runner key, the base tree, the head's dependency inputs): no format change, and no extra head run. `--fresh` writes without reading; `--no-records` does neither. Record errors in this mode are stderr lines, never a failed run. `aver gc` prunes records unwritten for 14 days and beyond the 64 newest (age only; records carry no repository, so no per-repository count).
- **A record no longer explains a head failure alone.** WP5 judged a test the recorded base failed from the record like any cached run, so a base recorded while failing for a reason of the moment (the network down, a busy port) made a real break `PREEXISTING` for as long as the record lived. Now a head failure the record failed too gets the same one fresh base run as a `NEW` on a recorded base: a fresh pass reruns the base side fresh and judges from it. This applies to `--base-record` in CI too. The cost is one base run per check whose failures the head shares with a recorded base.
- **Record blobs are additive.** `record::write` no longer empties `blobs/` first: blobs are written atomically if missing, `record.json` replaced atomically, and unreferenced blobs removed after. Concurrent writers end with the last `record.json`; a blob another writer removed only drops rows, which rehashing on load already rejects.
- **Narrowed sides are Cargo's alone.** The transplant and the head-tests run of a `libtest` check run only the test targets the head's test files make (`--test <name>`, `--lib --bins` for inline tests) and read the rest from the base and the judged head. pytest, node:test, Jest, Vitest and Go run in full: none was shown equivalent here (conftest and fixture scope, module-level side effects, package-wide `TestMain`), and the hard rule is zero verdict change.
- **Filled, not merged after.** A narrowed run is filled with the source side's run at the same index before the rerun policy reads it, so a side reruns exactly when the full run would; any doubt at that point (no report, a binary that did not finish, a changed target missing, an exit code the tests do not explain, a test on both sides) reruns the side with the full command. The fill trusts that a target the head did not touch reports on the transplant what it reports on the base; flakiness in such a target is sampled from the base's run instead of a fresh one.
- **The package selection stays.** The narrowed command keeps the check's `--workspace`, `-p` and `--exclude`, flags and filters, so it builds what the full command builds; only the targets run change. `--examples` is dropped (it only builds). Doc-tests cannot be selected alongside units, so a changed `src/` file under a command that runs doc-tests runs in full.
- **Recorded unsigned.** The diagnostics' `narrowed` and the scope's `Where:` line say which sides were narrowed; the receipt is unchanged, since it is the full runs' result.
- **Feature-gated Rust code is uncovered again (RUST-3).** WP1b deleted `cargo_features.rs` on the premise that a contract's own command builds the right features; in CI generic mode, with no `aver.toml`, nothing did, and a regression in clap's `#[cfg(feature = "env")]` `Parser::add_env` was a `PASS`. The rule is back (manifests read from the base), with the diagnostics' `unbuilt` and a remedy line naming the feature and a command. An extra `--all-features` run in generic mode was measured and not adopted: clap's base fails 275 of 969 `builder` tests and hangs in REPL doc-tests under it, so it would block verdicts instead of judging the change.
- **In place, sides run grouped by code**: base, transplant, judged head, head tests, awaited one after another, with a rank per side in the slot queue so reruns join their group. Ranks alone did not hold it: the base was usually a warm cache hit, and the head took the free slot first.

### WP6 matrix fixes

The express and swr reruns of the WP6 matrix exited 0 on real regressions because their checks could not run. The goal of these fixes is that no regression exits 0 in `aver ci` for that reason.

- **Every blocker explained, none `ENV` (JS-5).** `ci::blocked_by_the_base` explained a `BLOCKED_NOT_OWN` when any one blocking entry failed on the base, so a red lint beside an `ENV` build exited 0. It now requires every blocking entry (`ENV`, `FLAKY`, inconclusive) to fail or flake on the base and none to be `ENV`. The exit code, `summary.md` and the action all read this one function.
- **A missing command is `ENV` on the base.** `exec::toolchain::missing_command` reads a shell's `command not found` (bash, zsh, dash, script prefixes) and `env: x: No such file or directory` as a toolchain refusal, so a base run that reported no test is `ZeroTests` and its entry `ENV`, per-test or exit-only; a setup command's `(evidence) setup failed` entry names it. The fix line says the dependencies are not installed. Only the base is classified: a tool missing on the head alone stays `FAIL_OWN`, which already exits 1, and a runner that reported tests is never a missing command.
- **Installed dependencies kept whatever installed them (JS-2).** `place::sides::deps_of` keeps `node_modules` and `.venv` at any depth for every contract, and without an npm or Python layer takes `pnpm-lock.yaml`, `yarn.lock`, `bun.lock`, `bun.lockb` and `package.json` (or `pyproject.toml`, `setup.py`, `setup.cfg`) as inputs. In-place adoption, `--slots` and `seed_workspace` all read it. `venv` is not kept: the name is common for source and nothing marks it as an environment. A tracked file under a kept directory survives a tree swap, as it already did for npm.
- **Record identity covers every lockfile and manifest.** `record::deps_state` hashes the contract's inputs together with every lockfile and Node or Python manifest in the tree. A fallback used only when the contract named nothing never applied, since a legacy `aver.toml` always names `package-lock.json`.
- **Proof.** `verdict_integrity` B14 (a tool missing on both sides is `ENV` and exits 1; a pnpm `node_modules` survives in place and in slots, so the regression is `FAIL_OWN`) and `bench/verify-10-of-10.sh` scenario `b14`. Real reruns with the release build, in place with `--no-records`: express (npm install, no lockfile, `res.links` joining with a space) `FAIL_OWN`, exit 1; swr (pnpm, `mergeConfigs` merging `cacheData` in the wrong order) `FAIL_OWN`, exit 1; `node_modules` intact after both.

### Status

| Package | Branch | State |
|---|---|---|
| WP1a core types (`contract`, `evidence`, `pushback`, snapshot v1, `RunKey`, `tag`) | `redesign/wp1a-core` | merged (`2d10ff7`) |
| WP1b deletions (MCP, Docker sandbox, gVisor, `cargo_features`, hooks) | `redesign/wp1b-delete` | merged (about 24,000 lines removed) |
| WP2 places, in-process runner, native rebuilder deleted | `redesign/wp2-places` | merged |
| WP3 judge, invariants, reports, bootstrap, `check.rs` and `detect.rs` deleted | `claude/project-thread-zjy48z` | landed (plans: `docs/redesign/wp3-plan.md`, `wp3-plan-2.md`) |
| WP4 VCS and Git targets, trust guards | `claude/project-thread-zjy48z` | landed (packages judge, accept, ws, combine, coord) |
| WP5 CI action, cross-PR cache, workspace fast path | `claude/project-thread-zjy48z` | landed (packages fast, place, cache, approve) |
| WP6 benchmarks, compat matrix, live evals | | not started |

### Tests to rebuild in WP3

WP1b deleted these tests with the Docker sandbox. Each covered verdict behavior that still matters; WP3 rebuilds it on the unified judge, natively. Tests of Docker mechanics alone (`sandbox_security.rs`, `exec_docker.rs`, the dependency-layer and Cargo build-cache tests of `exec_cache.rs`, the forged-result test of `ci_docker.rs`) are gone with nothing to rebuild.

- `verify_docker.rs`, on the calc fixtures end to end:
  - `a_change_that_breaks_an_existing_test_is_fail_own_with_cause_new`: a broken existing test is `NEW`, the verdict `FAIL_OWN`.
  - `fixing_everything_passes_and_a_second_verify_starts_no_sandbox`: the fix is `PASS`, and a second verify of the same trees starts no run.
  - `a_test_failing_on_both_sides_is_preexisting_and_does_not_block_pass`: `PREEXISTING` never blocks `PASS`.
  - `a_test_keyed_to_the_run_index_is_stable_on_both_sides`: no run index reaches the code under test.
  - `a_check_that_exceeds_its_timeout_is_env_and_blocked_not_own`: a timeout is `ENV`, the verdict `BLOCKED_NOT_OWN`.
  - `a_task_check_proves_its_criteria_with_source_task`: a task-defined check proves its criteria with source `task`.
  - `a_test_deleted_on_head_runs_as_the_base_has_it_and_is_reported`: a test the head deletes runs from the base's copy and is reported.
  - `a_head_environment_cannot_change_how_task_checks_run`: task checks run in the base environment, whatever the head's `aver.toml` says.
- `generic_docker.rs`, generic mode end to end: `python_detects_pytest_and_judges_test_by_test`, `go_detects_go_test_json_and_vet` (per-finding `go vet` entries), `rust_inline_tests_and_a_lost_report_over_a_red_base`, `rust_detects_cargo_test_and_reads_libtest_output`.
- `flow_docker.rs`, `an_agent_takes_a_task_from_import_to_an_exported_commit`: the whole agent flow over signed HTTP, import to `FAIL_OWN` to `PASS` to submit to export, and the receipt's `reproduce` command replayed.
- `ci_docker.rs` (the rest is now `ci_cli.rs`, native):
  - `a_regression_only_pr_that_breaks_a_test_fails_own_with_exit_1`: natively, the `NEW` failure's diagnostics lose `location.file`.
  - `existing_test_files_prove_with_the_heads_cases_and_judge_with_the_bases`: natively `UNPROVEN` instead of `PASS`; a tagged case added to an existing test file must prove through the head-tests run.
  - `without_aver_toml_the_default_check_builds_and_compares_test_by_test`: the default check runs the build as setup and judges test by test; it used `--home` without `--run-as`, which native CI refuses.
  - `concurrent_runs_on_one_home_are_serialized`: two `aver ci` on one `--home` run one after the other and the second is all cache hits; the same `--home` restriction.
- `run_docker.rs`, `run_e2e_calc`: `aver run` with three scripted agents ranks a `PASS` winner and `aver accept` exports it; natively every attempt is `UNPROVEN` (`weak_proof`, `passes_without_change`). It ran only under `--ignored`, so whether it already failed before WP1b is not known.
- `test_changes.rs`, `adding_a_dependency_and_code_that_uses_it_passes`: a head that adds a dependency and code using it passes; head runs install the head's lockfile, base and transplant runs keep the base's.
- `judging.rs`, `a_head_cannot_choose_the_install_command`: kept for its `judging::aver.toml` `CHANGED` entry; the half that checked the install ran the base's command with scripts off went with the layers.
