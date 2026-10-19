# WP2 plan: in-process runner and execution places

Planning only. Read at `redesign-0.4` = `840756c`. Paths are under `crates/aver/src/` unless they start with `tests/` (`crates/aver/tests/`), `aver-core/` or a repository-root file. Line numbers are at `840756c`. **[inference]** marks a reading the code does not state.

WP1b is assumed merged first: `mcp/`, `cli/mcp.rs`, `exec/{sandbox,materialize,deps,slots}.rs`, `cargo_features.rs`, the five hooks and their tests are gone. WP1a is assumed merged: `aver-core/{contract,evidence,pushback}.rs`, snapshot v1, `RunKey`/`PlaceKey`/`DepsState`.

## 0. Scope, bridge, and the decisions this plan makes

**What WP2 ends with.** `place/{proc,workdir,sides}.rs` exist and are tested by `cargo test --test place`. Every check run, local or `aver ci`, goes through `place::proc` in a `place::sides` slot. `exec/native.rs` and the pools are gone. No command starts or talks to a daemon. `verify.rs`, `judging.rs`, `rules.rs`, the `Environment`-based plan and `check.rs`/`detect.rs` are untouched apart from call sites (WP3 owns them).

**The test seam.** 45 test files build a `World` whose `Executor::new(store, Arc<dyn Sandbox>)` gets the `Scripted` fake (tests/world/mod.rs:11, :51-62; tests/scripted/mod.rs; tests/run_support/mod.rs:61). WP1b deletes `exec/sandbox.rs` but must leave that seam. WP2 names it `place::Runs` (`fn run(&self, req: RunRequest<'_>) -> BoxFuture<Result<proc::Output, ProcFailure>>`) with two implementations, `place::SlotRunner` (sides plus proc) and `Scripted`. `Executor` holds `Arc<dyn Runs>`; `Runner::{Sandbox,Native}` (exec/mod.rs:171-174), `is_native`, `native_runner` (236-247) go. If WP1b named the seam differently, keep its name.

**The bridge between WP2 and WP3.** `verify.rs` still drives `Executor::run_from(snapshot, deps, env, check, index)` (exec/mod.rs:357) with `Environment` and `CheckDef`. WP2 keeps that signature and replaces only what is under it:
- `Executor` holds a `place::Sides` instead of `Runner::Native(Arc<NativeRunner>)` (exec/mod.rs:171-186).
- `verify.rs::pools()` (verify.rs:396-416) and `warm_pool()` (421-434) return `place::SidesFor { base: SlotName::Base(tree16), head: SlotName::Judged(ws), transplant: SlotName::Transplant(ws) }` instead of `(Pool, Pool)`. `run_checks` (verify.rs:542-549) picks the executor clone per side exactly as today, with `in_slot(..)` in place of `in_pool(..)`.
- `Deps` for v0-v3 environments comes from one bridge function, `place::deps_of(env: &Environment, ws: &WorkspaceConfig) -> contract::Deps`: `keep` = `<dir>/node_modules` for the root lockfile and each npm layer, `<dir>/.venv` for each uv/pip layer, plus `[workspace] seed`; `seed` = `<dir>/target` for each Cargo layer plus `[workspace] warm`; `inputs` = every lockfile path; `install` = none. WP3 deletes it when the contract replaces `Environment`. If WP1a's v0-v3 fallback reader already produces `Deps`, call that instead.

**Decisions (each reversible; the reversing word is in brackets).**
1. **Slots serialize per slot.** One exclusive `flock` per slot directory for a whole run. Two checks of one side run one after the other; sides still run concurrently. Today each check has its own pool directory (native.rs:821-824), so Aver's own `aver.toml` (rust, site, e2e) loses cross-check parallelism on one side. The upgrade path is a downgrade to a shared lock after `become_tree` plus a re-check of the journal's `at` (about 20 lines) if `make bench-native` regresses. [`shared-slots`]
2. **`aver ci` runs in slots in WP2, not in place.** In-place CI (`Workdir::adopt` on the CI checkout, side order head-tests, judged, base, transplant, restore) needs the judge to order sides, and today's `run_checks` runs sides concurrently (`try_join!`). WP2 builds and tests `Workdir::adopt`/`restore`; WP3 wires `Place::Ci`. Until then `aver ci` clones the CI checkout into slots under its throwaway home, which costs a full copy of keep dirs on Linux runners without reflink. [`ci-in-place-now`]
3. **A private `TMPDIR` per run stays** (native.rs:2405-2430). It is the one deviation from "the caller's environment unchanged": concurrent base and judged-head runs of one suite would otherwise collide on fixed names in `/tmp` (sockets, lock files). [`inherit-tmpdir`]
4. **`CARGO_TARGET_DIR` set by the caller is overridden to `<slot>/target`.** Otherwise every slot shares one target directory, head-built units are reused on the base side, and invariant I1 fails. This is the second deviation. [`inherit-target-dir`]
5. **`--run-as` survives as process management** (sudo wrapper, hand-over, kill by user, temp sweep). SYNTHESIS §5.5 keeps an `isolate-user` action input for WP5, and `aver ci --home` without it refuses for a reason (cli/ci.rs:140-142). It moves to `proc.rs` intact (about 80 lines). The lazier path drops it, removes the guard and edits action/run.sh:26,75, action/test/run.sh:431-452 and docs/VERDICTS.md:181-182; then a kept `--home` key is readable by PR test code. [`drop-run-as`]
6. **Network is on.** `exec/netns.rs`, `NetWrap` and `sandbox-exec` go; provenance says `network: on`.
7. **No forced `CI=1` or `NODE_ENV=test`.** The spec says inherit unchanged. stdin is `/dev/null` and stdout is a file, so watch modes stay off **[inference]**. [`force-ci-env`]

## 1. `place/proc.rs`

### 1.1 What moves from `exec/native.rs`

| Moves | native.rs lines | Becomes in `proc.rs` |
|---|---|---|
| `RUN_TAG_VAR` | 71 | `RUN_TAG_VAR = "AVER_RUN"`; the tag every child inherits, for reaping |
| `is_executable` | 528-530 | unchanged |
| `version_output` | 534-567 | runs with the **inherited** env and `cwd = slot` (so a version-manager shim answers as it will for the run), feeds `host_tools` |
| `NativeFailure` + `From<io::Error>` (ENOSPC → `DISK_FULL`) | 661-689 | `ProcFailure` |
| `run`'s command loop: setup commands, then argv, setup exit codes folded into the last exit, `Ended` → `Exit` | 1031-1146 (from `let cwd =` to the end of the final match) | `proc::run(&Spawn) -> Result<Output, ProcFailure>` |
| `output` (bounded log + every file under `out`) | 1149-1167 | `Output { exit, output, files, duration }` (today `SandboxOutput`) |
| `spawn_with`: program lookup on `PATH`, log file as stdout+stderr, `stdin(null)`, `process_group(0)`, nice via `setpriority`, pid record + `fault::point("native.spawned")`, 10 ms poll, deadline → SIGKILL the group, `track_descendants` | 1740-1876 | `spawn` (minus the netns branch 1769-1776 and 1807-1810) |
| `reap`, `kill_group`, `kill_tagged`, `tagged_pids` (Linux `/proc/*/environ`, macOS `KERN_PROCARGS2`), `Tracked`, `track_descendants`, `kill_tracked`, `proc_table` | 1883-1906, 2145-2365 | unchanged |
| `--run-as`: `sudo`, `serial`, `sweep_tmp`, `hand_over`, `sudo_works` | 1757-1768, 1908-1964, 2023-2037, 2445-2453 | `RunAs` (decision 5) |
| `Spawn`, `Ended` | 1998-2021 | `Spawn` loses `offline`; `Ended` gains `Interrupted` (§3.6) |
| `PidFile`, `kill_leftover`, `kill_leftovers` | 2039-2104 | keyed by slot: `<slot>.pid` beside `<slot>.lock`; `kill_leftovers(sides_root)` walks `sides/*/*.pid` |
| `remove_runs`, the fresh `run-<ulid>/out` directory | 2106-2115, 854-863 | `out_dir(slot) -> PathBuf`, outside the slot tree: `<sides>/<repo16>/<slot>.runs/run-<ulid>/{out,log}` |
| `disk_guard`, `disk_low`, `report_written` | 2117-2143 | `report_written` takes the declared report path (WP3: "any `*.xml` in `$AVER_OUT`") |
| `private_tmp` | 2405-2430 | unchanged (decision 3) |
| `lock_in_place` | 2431-2443 | the slot lock |
| `status_signal`, `append`, `cap_lines`, `collect_files` | 2455-2516 | unchanged |
| `rehome` (`/aver/out/` → the run's `out/`) | 2466-2469 | kept for v0-v3 `CheckDef` report paths until WP3 |

From `exec/sandbox.rs` (deleted by WP1b): `Exit`, `SandboxOutput`, `SandboxLimits` (only `timeout` and `output_bytes` stay meaningful), `bound_output`, `output_split`, `OUTPUT_HEAD_BYTES`, `MAX_LINE_BYTES`, `DISK_FULL`, `MIN_FREE_BYTES`, `free_bytes`, `says_disk_full` (sandbox.rs:27, 102, 149-182, 258-327, 456-476). If WP1b parked them in a stub module, WP2 moves them into `proc.rs` and deletes the stub. `bound_output` already keeps head 8 KiB plus tail, so a Python traceback's last line survives.

From `exec/mod.rs`: `conclude` (690-755) and `build_run` (757-815) stay in `exec/mod.rs` and read `proc::Output` instead of `SandboxOutput`; they are the report-parsing hookup (`junit::parse_as`, `toolchain_error`, `toolchain::classify`). `natively` (527-564) becomes `in_slot` (§1.3). `sandboxed`, `run_command`'s sandbox branch and `Runner::Sandbox` are WP1b's.

### 1.2 What is deleted from `exec/native.rs`

| Deleted | Lines |
|---|---|
| module doc of the clean-room contract | 1-19 |
| `SANDBOX_OUT` stays only inside `rehome`; `TOOLS`, `SYSTEM_PATH`, `KEPT_VARS`, `dropped`, `is_shim_dir` | 50-105 |
| `NativeConfig` (host env snapshot, network_off, root, mode) | 107-142 |
| `Pool`, `PoolSide` | 144-253 |
| `Found`, `Toolchain`, `Rust`, `probe`, `rust_toolchain`, `resolve`, `is_rustup_proxy`, `probe_rust` | 255-527 |
| `NetWrap` | 569-645 |
| `NativeRunner` fields and accessors `toolchain`, `executor`, `network`, `net_wrap`, `strategy`, `check_dir` | 647-824 |
| `run`'s pool part: pool-dir mtime touch, `cargo_config_above` refusal, `sync_src`, rust channel and toolchain PATH, `base_vars`, `BuildCacheKey`, `cache_dir`/seeding, `prepare_layer` loop, resolved `Cargo.lock` fetch, farm cache sweep, run-as `CARGO_HOME`, `OVERRIDABLE`/`managed` env merge, `OFFLINE_ENV`, clean `PATH` | 827-1030 |
| the `toolchain.resolve` precheck of each command | 1053-1063 |
| `base_vars` | 1170-1225 |
| `cache_dir`, `seed_source`, `newest_base_cache`, `seed_from_base`, `CacheState`, `Written`, `SrcState` | 705-730, 1227-1384 |
| `prepare_layer`, `resolved_cargo_lock`, `fetch_once`, `venv_once`, `install` | 1386-1738 |
| `cache_executor` | 1966-1996 |
| `default_root`, `PoolClaim`, `claim_pool` | 2367-2403 |
| `join_path`, `cargo_config_above`, `remove_all` (moves to `place/workdir.rs` without `disk::deps::make_writable`) | 2460-2464, 2518-2541 |
| `tree_files`, `rust_channel` | 2543-2595 (`tree_files` moves to `place/workdir.rs`) |
| `sync_src`, `write_state_farms`, `mtime_ns`, `remove_extras` | 2597-2737 (the algorithm moves to `Workdir::become_tree`, §2.1) |

`exec/netns.rs` (103 lines) goes whole. `exec/toolchain.rs` stays: `classify` feeds `rules::base_ran_no_tests`.

### 1.3 Signatures

```rust
// place/proc.rs
pub struct Spawn<'a> {
    pub commands: &'a [Vec<String>],     // setup..., then the check; WP3 passes ["sh","-c",line]
    pub cwd: &'a Path,
    pub env: &'a Env,                    // built by Env::inherit, never by hand
    pub out: &'a Path,                   // $AVER_OUT, fresh per run
    pub log: &'a Path,
    pub timeout: Duration,
    pub output_bytes: u32,
    pub nice: bool,                      // the transplant
    pub record: Option<&'a Path>,        // <slot>.pid
    pub run_as: Option<&'a RunAs>,
}
pub struct Output { pub exit: Exit, pub output: Vec<u8>, pub files: BTreeMap<String, Vec<u8>>, pub duration: Duration }
pub enum Exit { Exited(i32), Timeout(i32), Interrupted }   // Oom is WP1b's to drop with the sandbox
pub struct ProcFailure { pub message: String, pub output: Vec<u8> }
pub fn run(s: &Spawn<'_>) -> Result<Output, ProcFailure>;

pub struct Env(BTreeMap<OsString, OsString>);
impl Env {
    pub fn inherit(out: &Path, tmp: &Path, slot: &Path) -> Self; // §1.4
}
pub fn host_tools(commands: &[Vec<String>], env: &Env, cwd: &Path) -> Vec<HostTool>; // realpath + --version, cached
pub fn kill_leftovers(sides_root: &Path) -> Vec<String>;
pub mod interrupt { pub fn install(); pub fn requested() -> bool; }
```

`host_tools` resolves `commands[i][0]` on the inherited `PATH`, follows a `#!/usr/bin/env X` or `#!/abs/X` shebang one level (so `npm` brings `node`), canonicalizes, and caches `--version` output in `$AVER_HOME/sides/versions.json` keyed by `(realpath, size, mtime)`, the cache pattern of `Toolchain::probe` (native.rs:289-438). The executor string in the run-cache key becomes `HostProfile::of(os, arch, &host_tools(..)).executor()` + `"+" + mode` + `"." + DepsState hex16` (aver-core/host.rs keeps `HostProfile::of`; WP1a's `PlaceKey::Host` replaces the string when `RunKey` lands in WP3). Provenance (exec/mod.rs:282-299) becomes `{ mode, host_profile: Some(that profile), network: On, uid: run_as.map_or(Same, Separate) }`.

### 1.4 The environment policy (§3)

`Env::inherit` starts from `std::env::vars_os()` of the `aver` process, which now is the caller's shell (no daemon), then:
- **adds** `AVER_OUT=<out>`, `AVER_RUN=<ulid>` (the reaping tag), `TMPDIR=<private tmp>` (decision 3);
- **overrides** `CARGO_TARGET_DIR` to `<slot>/target` when it is set and points outside the slot (decision 4);
- **strips** `GITHUB_OUTPUT`, `GITHUB_ENV`, `GITHUB_PATH`, `GITHUB_STEP_SUMMARY`, `GITHUB_STATE`, `ACTIONS_RUNTIME_TOKEN`, `ACTIONS_RUNTIME_URL`, `ACTIONS_RESULTS_URL`, `ACTIONS_CACHE_URL`, every `ACTIONS_ID_TOKEN_REQUEST_*`, and `AVER_RUN`/`AVER_NATIVE_RUN` from an outer run. `GITHUB_TOKEN` and `GH_TOKEN` are stripped too, because the action passes `github-token` to `aver` itself: beyond the spec's list, flagged for WP5. `AVER_FAULT` is stripped so a custody test's fault never fires inside a check.

The strip list is one `const STRIPPED: &[&str]` plus a prefix list, tested by name (§5). Under `run_as`, `sudo -n -u <user> -- /usr/bin/env -i <Env>` carries the same map (native.rs:1757-1768), minus `HOME`, which becomes the run's own directory because the run user cannot read the caller's.

### 1.5 Reports

`proc::run` returns every regular file under `$AVER_OUT` (`collect_files`, native.rs:2497). `exec/mod.rs::build_run` (757-815) keeps choosing the declared report path (`rel = path - "/aver/out/"`) and parsing it. Nothing in parsing moves in WP2; `aver/reports/` is WP3.

### 1.6 Other callers of the native runner

- `exec/mod.rs`: `key()` (306-329) drops `cache_executor` and `Pool.base` as `toolchain_from` (536-539); the rust-toolchain component goes because the caller's rustup resolves `rust-toolchain.toml` itself. `provenance()` (282-298) as in §1.3. `natively()` (527-564) becomes `in_slot`; a run with no slot (an `exec_run`-style run of a snapshot) gets `S-<snap16>`, so a head run never writes a `B-` slot (today's `Pool::of_snapshot`).
- `verify.rs`: `use exec::native::Pool` (:63), `pools()` (396-417), `warm_pool()` (422-434), `run_checks` (532, 540-548). `Disk::repo_of_base` and `repo_with_commit` (disk/pools.rs:213, :236) are plain materialization and `git_map` queries; they move to `disk/mod.rs`. The transplant keeps `nice`.
- `cli/ci.rs`: `sudo_works` (137) and the `--home` guard (140-141) stay with `RunAs`; the pool root `TempDir` (185-194) becomes the slots' root under the throwaway home; `NativeConfig::for_process` (196-206) becomes `services.executor(ProvenanceMode::Ci)`. `ci/mod.rs:386` `is_native()` gate goes (always true). `ci/summary.rs:560` "a cleaned environment" is reworded to "the job's own environment".
- `detect/suggest.rs` stays (§1.2.3) and must keep `aver init`'s dry run: `dry_run` (405-445) builds `NativeRunner` with `Pool { repo: "init", side: Base }`. It becomes `Sides::enter(.., SlotName::Snapshot(head), source = repo)` plus `proc::run`. `judge` (450+) matches "is not installed on this machine", today from `Toolchain::resolve` (native.rs:460-485); `proc::spawn`'s `PATH` lookup (native.rs:1778-1797) keeps that exact wording.
- `check.rs` uses only `sandbox::RESERVED_VAR` (:601) and `toolchain_error` (:534); `RESERVED_VAR` moves to `proc.rs`.
- `recover.rs:194` calls `proc::kill_leftovers(&home.join("sides"))`; the `killed` docs (recover.rs:54-56, 71-74) say `<slot>` instead of `<pool>/<check>`.
- `disk/cargo.rs:146` calls `exec::deps::cargo_package_name`, which WP1b deletes: WP1b stubs or WP2 lands the move of `is_target`/`last_built` first.

## 2. `place/workdir.rs` and `place/sides.rs`

### 2.1 `Workdir`: one directory swapped between trees

```rust
pub struct Workdir {
    root: PathBuf,
    keep: GlobSet,                 // deps.keep ∪ deps.seed ∪ {".aver"}
    journal: PathBuf,              // CI: <root>/.git/aver-swap.json; slot: <sides>/<repo16>/<slot>.swap.json
    state: Journal,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    adopted: Option<Adopted>,      // CI only: what restore puts back
    at: Option<TreeId>,            // None: unknown, the next become_tree hashes every tracked path once
    swapping_to: Option<TreeId>,   // set before the first write, cleared after the last
    files: BTreeMap<Path, Written>,// blob, kind, size, mtime_ns, ino (today's Written, native.rs:705-713)
    touched: BTreeSet<Path>,       // every path become_tree wrote or deleted since adopt
}
struct Adopted { commit: GitSha, tree: TreeId }

pub enum Blocker2 { Dirty(Vec<Path>) }   // WP1a's Blocker::DirtyCheckout when it exists

impl Workdir {
    pub fn adopt(root: &Path, store: &Store, head: (GitSha, TreeId), keep: GlobSet) -> Result<Self, WorkdirError>;
    pub fn open(root: &Path, journal: &Path, keep: GlobSet) -> Result<Self, WorkdirError>; // a slot
    pub fn become_tree(&mut self, store: &Store, tree: TreeId) -> Result<Swap, WorkdirError>;
    pub fn derived(&self, store: &Store, globs: &GlobSet) -> Result<Vec<(Path, BlobId)>, WorkdirError>;
    pub fn probe_copies(&self, store: &Store, changed: &[Change]) -> Result<Vec<StaleCopy>, WorkdirError>;
    pub fn restore(root: &Path) -> Result<Restored, WorkdirError>;   // no store: reads Git
}
pub struct Swap { pub written: u32, pub removed: u32 }
pub struct StaleCopy { pub keep: Path, pub tracked: Path }
```

**`adopt`.** Walks `head`'s tree (`tree_files`, moved from native.rs:2543), hashes each tracked path on disk through `aver_core` (rule 2), and returns `Dirty(paths)` when any differs. Then deletes untracked files outside `keep` and `.git`. It writes the journal with `adopted`, `at = head` and the real inodes. **[inference]** the deletion means a `dist/` the user's setup step built is gone before the first side; the contract's `keep` must name what CI setup leaves that matters.

**`become_tree`.** Today's `sync_src` (native.rs:2597-2684) with `remove_extras` (2704-2737), changed in three ways:
1. Writes `swapping_to = tree` through `atomic::write` before the first file change, and `at = tree, swapping_to = None, files = …` after the last.
2. On open, a journal with `swapping_to` set (a crash mid-swap) drops `files` entries for every path in `diff(at, swapping_to)`. The next `become_tree` rewrites them. A file is kept only if blob, kind, size, mtime and inode match (the rule at 2626-2632), so a torn file never survives.
3. Untracked files outside `keep` are deleted, not just files under directories the tree lacks: `remove_extras` already walks the directory. `.git` is untracked and outside `keep`, so it goes. Every written file gets a fresh mtime (a plain `fs::write`), which is what keeps Cargo and `.pyc` honest (grounding §9.7).

With `at = None` (a fresh slot clone, §2.2) the first `become_tree` hashes every tracked path present instead of trusting `files`. Paths equal to the target keep their cloned mtime; paths that differ are written fresh. That is invariant I1 in one rule.

**`derived`.** After a run, before the next `become_tree`: every regular file not in the current tree, not under `keep`, matching `globs` (`judge.tests ∪ judge.built_tests`), put as a blob, sorted by path. WP3's pair rule compares the vectors.

**`probe_copies`.** For each changed text path `p` with ≥ 2 components, every file under a `keep` directory whose path ends with the last two or more components of `p`, and whose bytes equal `p`'s base or head blob, is a `StaleCopy`. The keep index (file name → paths) is built once per `Workdir` and cached by the keep roots' `(dev, ino, mtime)`. Files over 1 MiB and `node_modules/.cache` are skipped. Editable installs leave no copy, so they never match.

**`restore`.** Reads the journal, runs `git ls-tree -r -z <adopted.commit>` and one `git cat-file --batch` (the streaming reader in git/import.rs:795-860) for every path in `touched`, writes each back, deletes `touched` paths the commit lacks, then removes the journal. It needs no Aver store, so `aver ci restore` works after a killed job whose throwaway home is gone. It never touches paths outside `touched`.

### 2.2 `Sides`: local slots

```rust
pub struct Sides { root: PathBuf /* $AVER_HOME/sides */, store: Arc<Store>, strategy: CloneStrategy }
pub enum SlotName { Base(TreeId), Judged(WorkspaceId), Transplant(WorkspaceId), Snapshot(SnapshotId) }
pub struct SlotSource<'a> { pub dir: &'a Path, pub deps: &'a Deps }   // agent dir, user checkout, or CI checkout
pub struct Slot { pub dir: PathBuf, pub work: Workdir, _lock: FileLock, pub relocated: Relocated }

impl Sides {
    pub fn new(home: &Path, store: Arc<Store>) -> Self;
    pub fn enter(&self, repo: &str, name: &SlotName, source: &SlotSource<'_>, tree: TreeId) -> Result<Slot, PlaceError>;
    pub fn remove_for(&self, ws: &WorkspaceId) -> io::Result<u32>;          // release
    pub fn stale(&self, live_bases: &BTreeSet<TreeId>, idle: Duration) -> Vec<PathBuf>; // gc
}
pub fn relocate(slot: &Path, from: &Path, keep: &[Path]) -> io::Result<Relocated>;
pub struct Relocated { pub rewritten: Vec<Path>, pub binary: Vec<Path> }
pub fn deps_state(dir: &Path, inputs: &GlobSet) -> DepsState;
pub fn deps_of(env: &Environment, ws: &WorkspaceConfig) -> Deps;     // the WP2-WP3 bridge
```

**Layout.** `$AVER_HOME/sides/<repo16>/` with `B-<tree16>`, `J-<ws>`, `T-<ws>` (and `S-<snap16>` for a run outside a verification, today's `Pool::of_snapshot`, native.rs:228-238). Beside each: `<slot>.lock`, `<slot>.pid`, `<slot>.swap.json`, `<slot>.source.json` (source dir, its `DepsState`), `<slot>.runs/`. `repo16` is `aver_core::host::local_name` of the repository directory, as `Pool::of_verification` uses today (native.rs:197-218). The judged head and the head-tests tree share `J-<ws>` (as `H<ws>` does today).

**`enter`.** Takes `<slot>.lock` exclusively (`lock_in_place`), kills a leftover (`proc::kill_leftover`). If the slot directory is missing, or `<slot>.source.json` names another source dir or a different `DepsState`, it clones the source:
1. `disk::clone::clone_tree(strategy, source.dir, <slot>.tmp-<ulid>)` (disk/clone.rs:79), then `rename` into place, then `fault::point("slot.cloned")`. A crash leaves only a `.tmp-` directory, which `stale` sweeps.
2. Under `CloneStrategy::Copy`, `keep` directories are symlinked instead of copied (read-only by intent), and `seed` directories are not seeded (a cold build). **[inference]** on Linux without reflink this is the only affordable choice; it is SYNTHESIS §10's open question.
3. `relocate(slot, source.dir, keep)`.
4. Writes `<slot>.source.json` with `atomic::write`, and opens the `Workdir` with `at = None`.

Then `become_tree(tree)`. Sources: `B-*` clones the user's checkout (`Materialization.repo`) when known, else `checkouts/<tree>` (`Disk::ensure_checkout`, disk/checkout.rs:56) with no keep dirs; `J-*` and `T-*` clone the agent's workspace directory; in `aver ci` every slot clones the CI checkout. When the side's tree's `deps.inputs` blobs differ from the source's recorded `DepsState`, `deps.install` runs once in the slot through `proc::run` (network on, inherited env) and the result is recorded; with no `install`, the run gets an `[aver] keep dirs were installed from another lockfile` line in its log (WP3 turns it into `NeedInstall` / `InlineHeadDeps`).

**`relocate`.** Walks `keep` directories in the slot for text files named `*.pth`, `direct_url.json`, `.modules.yaml`, `*.egg-link`, and for `.venv/bin/*` scripts whose first line is a `#!` naming the source's absolute path. Each occurrence of `source.dir` becomes the slot path, written in the slot's clone only (breaking CoW for that file). Absolute symlinks into `source.dir` are re-pointed; relative symlinks (pnpm workspace links) are left alone. A binary file containing the path is listed in `binary` (WP3's `PathInKeep` ask). The `bin/` shebangs are beyond §3.2's list: without them `.venv/bin/pytest` runs the agent's venv and imports the agent's editable source **[inference from C1 transcript B]**; a test pins it (§5).

**Reuse.** `disk/clone.rs` (all of it), `disk/checkout.rs::ensure_checkout`, `disk::FileLock` (disk/mod.rs:397-460), `atomic::write`, `fault::point`, `git::Repo` and the import's `cat-file --batch` streamer, `aver_core::host::local_name`. `Workspaces::tree_dir` (workspace/disk.rs:944-1023) loses its farm and warm blocks and keeps working for `check.rs` until WP3.

### 2.3 Workspace materialize without farms

`finish_materialize` (workspace/disk.rs:318-404) replaces the npm farm loop (330-359) and the seed/warm loop with `clone_warm` (361-388, 1025-1050) by one call, `place::sides::seed_workspace(repo, dir, &deps, base_files)`:
- for each `keep` path present in the user's checkout whose `deps.inputs` blobs equal the base tree's: `clone_entry` it, then `relocate(dir, repo, keep)`;
- for each `seed` path likewise, then **fresh mtimes on every tracked file of `dir` whose bytes differ from the user's checkout file at the same path**, hashing the user's tracked files once (rayon). That keeps the round 12 hazard closed (disk/cargo.rs:1-16) without Cargo-specific code, so `disk/cargo.rs` can go.

`Materialization.farms: Vec<Farm>` (disk/mod.rs:281) goes; keep dirs join `seeded`, which `exclusions` (disk/mod.rs:1136-1145) already leaves out of sync and `write_git_view` (workspace/disk.rs:488-515) already writes to `info/exclude`. The SQL `farms` column stays, written `[]` (disk/mod.rs:1026, :1035, :1070, :1122): an index table, so no schema bump. `WorkspaceConfig.node_modules` and `clone_packages` (disk/mod.rs:299-301) stay parseable and ignored, because the parser is `deny_unknown_fields` and removing them would make an existing `aver.toml` fail. `DepsPolicy` (`--no-deps`, `--adopt-only`, disk/deps.rs) collapses to `bool deps`; `--adopt-only` becomes a hidden no-op.

The plain alternative (clone `target` whole, no mtime pass) reopens the round 12 hazard: a checkout file older than a unit the user built from different content reads as fresh. The mtime pass costs one hash of the user's tracked files per `workspace new`.

`Disk::reclaim_builds` (disk/mod.rs:815-865) stays because seeded `target` dirs stay. `is_target`, `profile_dirs`, `last_built` (disk/cargo.rs:30-62, 154-162, about 45 lines) move into `disk/mod.rs`, and so do `remove_tree`/`make_writable` (disk/deps.rs:634-660), which `empty_trash` (disk/mod.rs:633) needs for read-only trees.

### 2.4 Crash safety

| Point (new in `fault::POINTS`) | Where | Matrix row (tests/custody/matrix.rs) |
|---|---|---|
| `slot.cloned` | after the rename in `enter` | `Op::Verify`, `slot.cloned=abort`: `(Exit::Fault, CLEAN)`, rerun `(Exit::Ok, CLEAN)` |
| `swap.journaled` | after `swapping_to` is written | `Op::Verify`, abort: fault then ok; the rerun's `become_tree` rewrites the diff |
| `swap.partial#2` | after the second file written | `Op::Verify`, abort and `enospc`: ENOSPC reads as `ENV` (not done), rerun ok |
| `swap.done` | after the journal's final write | `Op::Verify`, abort: fault then ok |
| `relocate.written` | after the first rewritten file | `Op::WorkspaceNew`, abort: I4 holds (a half-relocated slot is recloned because `source.json` is written last) |
| `native.spawned` → `proc.spawned` | `proc::spawn` | the two existing rows (matrix.rs:352-365) renamed |

I4's audit (tests/custody/audit.rs) adds: no `sides/**/*.tmp-*` and no `*.swap.json` with `swapping_to` set after the rerun. `recover::on_open` (recover.rs:194) calls `proc::kill_leftovers(&home.join("sides"))` instead of the native root.

## 3. Daemon removal

### 3.1 Today's call paths

Two transports reach the services, and only one has an in-process twin.
- **`Ops` (cli/ops.rs, 451 lines).** `Ops::via` (ops.rs:50-57) sends a `Backend::Daemon` to `run::api::Api` over signed HTTP and a `Backend::Local` to `&Services`. `Backend::open` (cli/backend.rs:45-103) connects (`daemon::connect`, daemon.rs:199-220), spawns with `Start::Auto` (backend.rs:71-80, `daemon::spawn` daemon.rs:335-403), or takes the exclusive home lock (backend.rs:87-94) and runs `recover::on_open` (backend.rs:96). Methods and routes: claim (`/v0/tasks/{id}/claim`), task, materialize, sync, rebase, diff, restore, restore_undo, remove (discard then release), prepare_check, warm and prefetch (both **no-ops in process**, ops.rs:197-215), receipts, snapshot, verify (`/v0/verify` plus `/v0/verify/progress` polling, ops.rs:293-411), leases, notices. `Start::Auto` callers: cli/verify.rs:97 and :162, cli/check.rs:166 and :298, cli/workspace.rs:198, cli/hook.rs:810 (Stop). Raw `Remote::post` arms with in-process twins: import.rs:82, export.rs:65, agent.rs:76, task.rs:90, notices.rs:95, release.rs:65, gc.rs:71, verify.rs:109.
- **`run::api::Api` (run/api.rs, 927 lines, 44 methods) has no in-process twin.** `aver run` (bails under `AVER_NO_DAEMON`, cli/run.rs:334-338), `rerun` (rerun.rs:134-138), `accept` including solo accept (accept.rs:101-128, :420), `combine` (combine.rs:61), `status` (spawns a daemon, status.rs:34-42), `discard` (discard.rs:38), batch `diff` (diff.rs:83), and the WorktreeCreate/Remove hooks (hook.rs:253, :268, :392, :414) call `daemon::connect_or_spawn` (daemon.rs:406-415). `run/{supervisor,accept,combine,cross,compare,rerun}.rs` and `run/batch/{mod,schedule}.rs` take an `&Api`. The supervisor strips `AVER_NO_DAEMON` from agents (run/supervisor.rs:644).
- **Nothing else uses HTTP.** `aver ci` runs in process under the home lock (cli/ci.rs:146-163); the action runs `aver ci` (action/run.sh:2); `tests/e2e` and `scripts/up.sh` go with MCP (WP1b).

### 3.2 Target: one in-process service layer

1. **Move `Services` into the library** as `services.rs` (from cli/daemon.rs:47-160, which is misnamed: it is the service wiring, not the daemon). It owns `Store`, the server key, `Workspaces`, `Coord`, `Tasks`, `Verifier` and the `Executor` over `place::Sides`. `detect_docker`, `sandbox_probe`, `executor_with` are WP1b's to delete; `native_executor[_with]` becomes `executor(mode)`.
2. **`Api` becomes in-process with its signatures kept.** `Api { services: Arc<Services> }`; each method's body is its route handler's body (http/tasks.rs, http/disk.rs, http/workspaces.rs, http/coord.rs, http/objects.rs, http/git.rs) called directly, `&SigningKey` reduced to `AgentId` for ownership checks. So `run/*` and its tests change only their constructors. Then `Ops` collapses into `Api`: `Ops`'s Local arm and `Api` were two copies of the same calls (http/tasks.rs:176-224 duplicates ops.rs:357-411). `cli/ops.rs` and `cli/backend.rs` are deleted, and each command calls `Api` (migrate callers, then delete, in one commit).
3. **Error codes survive.** `run/*` matches on `ApiError::code()` (`SNAPSHOT_MOVED`, `BRANCH_EXISTS`, `NOT_RECEIPT_HEAD`, …). The `From<…Error>` impls of http/error.rs:73-263 move to `run/api.rs` as `ApiError::Refused { code, message }` constructors without the HTTP status. `ApiError::{Unreachable, Stopped, Decode}` go.
4. **The route capability checks** (`Need<cap::…>`, e.g. http/tasks.rs:260) were the only authorization layer, and the in-process path already skips them. They go. WP4's "refuse REVIEW accept under an agent key" reads the acting key name directly.
5. **`warm` and `prefetch` stop being no-ops.** `warm` (verify.rs:740) runs in the calling process in a detached child (`aver internal warm <task>`, `process_group(0)`, stdout to `$AVER_HOME/logs/warm-<ulid>.log`), because `workspace new` and WorktreeCreate must return before the base is built. **[inference]** without it, the first verify after `workspace new` pays the cold base build that the daemon used to hide. `prefetch` (sandbox only) is WP1b's to delete.

### 3.3 SQLite and locks

- `store/mod.rs:284`: `busy_timeout` 5 s → 30 s. WAL is already on (store/mod.rs:75) and every write transaction is `IMMEDIATE` (store/mod.rs:182, :289), so concurrent processes queue on the busy handler instead of failing on upgrade.
- **The exclusive home lock stops being taken per command.** Today `Backend` holds `<home>/lock` (keys.rs:83-99) for the whole command, test runs included (keys.rs:85-88 says it on purpose, because single-flight is per process). After WP2:
  - `aver gc`, `aver key` and `keys::init` take it exclusively; nothing else takes it.
  - Cross-process single-flight for runs comes from the slot lock: `Executor::execute` (exec/mod.rs:482-516) looks the run cache up **again after taking `<slot>.lock`**, so a second process verifying the same base finds the first one's run.
  - A host-wide run limit replaces `exec/slots.rs` (deleted by WP1b): `$AVER_HOME/sides/run-slots/<0..K>.lock`, K = cores / 2, a run takes the first free one with `try_exclusive`, else blocks on `pid % K`. About 30 lines. Round-robin fairness between requesters is lost.
  - `work.lock` (disk/mod.rs:497-505), `mat/<ws>/lock` and the slot locks already cover workspace and directory state.
- **`recover::on_open` (recover.rs:146-205) becomes per process.** `<home>/running` (recover.rs:101-103) holds one owner, which only the exclusive lock made true. It becomes `<home>/running/<pid>-<start>`; `on_open` recovers each record whose process is dead (`run::state::same_process`), and scopes each step: `proc::kill_leftovers` already skips slots whose lock a live run holds (the `try_exclusive` pattern, native.rs:2089); `mat/<ws>/stat` and `.aver/tmp` are swept only for workspaces whose `mat/<ws>/lock` it can take; torn `.aver/workspace.toml` rewrite is unchanged. `WorkspaceFile.daemon` (workspace/disk.rs:39-40, always `None`) and its salvage (recover.rs:317) go.
- `aver ci` (cli/ci.rs:155) keeps its home lock: its home is a throwaway or an explicit `--home`.

### 3.4 What of `http/` survives

Nothing. `http/{mod,client,coord,disk,error,exec,git,objects,tasks,workspaces}.rs` (2,700 lines), `auth.rs` (344; outside `http/` only tests use it), `daemon.rs` (415), `cli/serve.rs` (206), `cli/daemonctl.rs` (79), the `/healthz` api handover (daemon.rs:224-280, `API_VERSION` http/mod.rs:375) and the progress `Registry` (http/mod.rs:68-69) are deleted. The `requests` audit table stays in the schema unread (rule 1; dropping it needs schema_v5, not worth it now). CLAUDE.md rule 6 ("every route except `/healthz` is signed") is deleted with the routes. `axum`, `hyper`, `hyper-util`, `http-body-util` leave `Cargo.toml`. `aver serve`, `aver daemon start|status|stop` and `AVER_URL` print a one-line "removed in 0.4: commands run in process" and exit 2 for one release. `bench/eval/run.ts:498,578` and `scripts/try-run.sh:22` stop calling `aver daemon`.

### 3.5 Progress

Only the in-process path stays: the `Verifier` gets `progress: Some(Log)` and a scoped thread prints with `progress::print_until` (progress.rs:404-417; ops.rs:360-371). `POST /v0/verify/progress` and the 1 s polling (progress.rs:27, ops.rs:328-352) go.

### 3.6 Ctrl-C

Today `aver verify` and `aver check` install no handler. SIGINT kills `aver`, but the test child lives in its own process group (`process_group(0)`, native.rs:1807) and runs on until its deadline. Through the daemon the `spawn_blocking` run cannot be cancelled and the daemon keeps the home lock for 15 idle minutes (daemon.rs:26, serve.rs:54-81).

`place::proc::interrupt` owns the one SIGINT/SIGTERM handler, installed in `cli::main` (cli/mod.rs:185) for every command:
- A lock-free registry `LIVE: [AtomicI32; 64]` of running process groups. `spawn` registers the pgid, `reap` clears it.
- First signal: if `LIVE` is empty and the command is not in graceful mode, `_exit(130)` at once (so `aver status` still dies on Ctrl-C). Otherwise set `STOP`, and `kill(-pgid, SIGTERM)` every registered group from the handler (`kill` is async-signal-safe).
- The 10 ms poll loop in `spawn` sees `STOP`, waits up to 2 s, then SIGKILLs the group, reaps tagged descendants and returns `Ended::Interrupted`. `Executor::execute` stores nothing for an interrupted run (not even `infra`), `Verifier::verify` returns `VerifyError::Interrupted`, the CLI exits 130. `aver ci` restores the checkout on that path (WP3, once it runs in place).
- Second signal: SIGKILL every registered group, `_exit(130)`.
- `aver run` and `aver rerun` set graceful mode and read `interrupt::requested()` instead of their own `STOP` statics (cli/run.rs:209-215, :312-320; cli/rerun.rs:80-85, :223), so a first Ctrl-C stops the supervisor cleanly as today.

## 4. Deletion list

`detect.rs`, `detect/presets.rs`, `detect/languages.rs` and `check.rs` are §1.2.3/§1.2.4 files, but REDESIGN §8 moves them to WP3. WP2 leaves them compiling.

### 4.1 Source files

| Delete | Lines | Notes |
|---|---|---|
| `exec/native.rs` | 2,841 | about 700 move to `place/proc.rs`, the tree-writing ~140 to `place/workdir.rs` (§1.1-1.2) |
| `exec/netns.rs` | 103 | |
| `disk/deps.rs` | 784 | `remove_tree`, `make_writable` move to `disk/mod.rs` |
| `disk/cargo.rs` | 193 | `is_target`, `profile_dirs`, `last_built` move to `disk/mod.rs` |
| `disk/pools.rs` | 319 | `repo_of_base`, `repo_with_commit` move to `disk/mod.rs` |
| `disk/usage.rs` | 101 | doctor and gc print apparent sizes (`gib` inlined) |
| `daemon.rs` | 415 | |
| `cli/serve.rs`, `cli/daemonctl.rs` | 206, 79 | stubs that exit 2 for one release |
| `cli/backend.rs`, `cli/ops.rs` | 141, 451 | callers move to `run::api::Api` |
| `http/*.rs` | 2,700 | error codes move to `run/api.rs` |
| `auth.rs` | 344 | |
| `cli/daemon.rs` | 160 | renamed to `services.rs` in the library, not deleted |

Inside surviving files:
- `exec/mod.rs`: `mod native`, `mod netns` (29, 31), `Pool`/`NativeRunner` imports (53), `pool` field (161), `Runner` (171-174), `Executor::native` (201-206), `is_native`/`native_runner`/`in_pool` (236-257).
- `disk/mod.rs`: `mod cargo|deps|pools|usage` (17, 20, 22, 24), `pub use deps::…` (40), `Materialization.farms` (281), `GcReport` pool fields (371-391), `release_pools_later` call in `release` (710; replaced by `Sides::remove_for`), host-deps store pruning (979-1009; the `host_deps` table stays unread), `gc_pools` (1011-1014; replaced by `Sides::stale`).
- `workspace/disk.rs`: `npm_layers` (162-188), the farm loop (330-359), `clone_warm`'s dependency-unit cloning (1025-1050), `tree_dir`'s farm block (996-1018).
- `cli/doctor.rs`: `pools()` (363-428) and `POOLS_WARN_BYTES` (430), its call at 927, the no-lockfile Cargo note (~748). `workspaces()` (296-361) keeps counting `target` with the moved `is_target`.
- `cli/gc.rs`: the pool lines (86-101).
- `cli/serve.rs` `gc_pools_later` (85-87) goes with the file.
- `m.farms` readers: run/supervisor.rs:502, cli/workspace.rs:313 and :336 (the `"farms"` JSON key).
- `store/mod.rs:284`: busy timeout. `keys.rs:83-99`: `Home::lock` callers shrink (§3.3). `recover.rs:101-205`: per-process running records.
- `Cargo.toml`: `axum`, `hyper`, `hyper-util`, `http-body-util` (`ed25519-dalek` stays: receipts are signed). The last `axum::http::Method` uses outside `http/` are raw posts in run/accept.rs:1056 and cli/release.rs:64, which go with `Api`.
- `CLAUDE.md`: rule 6 deleted; rule 10 becomes "Check commands run in the caller's environment plus `AVER_OUT`, minus CI token variables (`place/proc.rs`); changes to `place/` need a test in `tests/place.rs`"; the commands table loses `make up`'s daemon, `aver serve`, `aver daemon`, `AVER_NO_DAEMON`, and gains `cargo test --test place`; Phase status gains a WP2 line. `ARCHITECTURE.md` §1.4.3 (executor ids), §1.8.5 (pools → slots), §2.4.6 (native runs), §2.7 (daemon) are rewritten in the commit that changes each.

### 4.2 Tests that die

| File | Why |
|---|---|
| `tests/native_env.rs` (1,283) except the fns listed under rewrites | cleaned env, shims, user Cargo config, pool caches, netns loopback, installs and farm caches, read-only venv, gc of pools, preload hooks, `sandbox_verifies_as_before`, real disk use, seeded head pool |
| `tests/daemon.rs` (584) | daemon start/stop/handover |
| `tests/auth.rs`, `tests/auth_routes.rs`, `tests/http_support/` | signed routes |
| `tests/http_tasks.rs`, `tests/http_disk.rs`, `tests/http_workspace.rs`, `tests/http_agent_ux.rs`, `tests/flow_docker.rs` | the same behaviors over HTTP; what survives is covered by `run_*`, `cli_workspace`, `disk` and the new `tests/api.rs` |
| `tests/home_lock.rs` (76) | asserts the second process waits; replaced by its opposite (§5) |
| `tests/recover.rs` `a_daemon_killed_mid_verify_says_so_plainly` (147), `an_upgrade_while_a_check_runs_completes_the_verify` (183) | daemon-bound |
| `tests/custody/k2.rs` `a_daemon_killed_at_verify_planned` (97), `an_upgrade_at_native_spawned_completes_the_verify` (131) | daemon-bound |
| `tests/disk.rs` `adopted_store_is_read_only_and_farm_links_resolve` (692), `host_node_resolves_through_farm` (1530), `every_npm_layer_gets_a_farm_beside_its_lockfile` (1589) | farms |
| `tests/rust_compat.rs` `…crit_rust_no_lockfile` (327), `a_rust_test_on_loopback_passes_with_the_network_off` (370) | Aver-resolved `Cargo.lock`, network off |
| `tests/doctor.rs` `a_cargo_package_without_a_lockfile_is_told_aver_resolves_one` (428) | |
| `disk/cargo.rs` unit test `unit_names_and_artifact_hashes` (164-193) | dependency-unit cloning |

### 4.3 Tests to rewrite or touch

**Rewrite against `place/` (into `tests/place.rs`):**
- native.rs unit tests (2738-2841): `a_leftover_run_is_killed_on_taking_the_lock_crit_k2_stale_reporter`, `a_record_naming_a_reused_pid_is_left_alone`, `enospc_in_the_runner_reads_as_a_full_disk_crit_k2_disk_full`, `earlier_run_directories_are_removed`, and the `running(pgid)` helper; they become `proc.rs` unit tests.
- native_env.rs: `a_leaked_listener_never_reaches_a_later_run` (451), `a_run_killed_from_outside_is_infra` (518), `each_run_has_its_own_unpredictable_tmpdir` (535), `long_lines_are_cut_as_in_the_sandbox` (991) → proc; `a_seeded_cache_never_hides_the_heads_sources` (367), `each_run_starts_from_exactly_its_tree` (793) → workdir; `a_run_outside_a_verification_never_writes_a_base_pool` (1269) → sides; `provenance_is_part_of_the_cache_key` (559), `the_host_profile_is_part_of_the_cache_key` (825) → executor key (network and uid axes dropped); `the_environment_is_cleaned` (120) is inverted (inherited, stripped by name); `native_ci_with_a_kept_home_needs_a_separate_user` (1004) and `ci_run_as_verifies_a_cargo_crate` (1026) stay with `RunAs`. `check_then_verify_starts_zero_runs` (855) moves to WP3's suite as is.
- `tests/disk.rs`: `a_warm_cargo_target_brings_dependency_units_only_and_gc_reclaims_it` (505) becomes "a seeded target rebuilds every file that differs from the user's checkout, and gc reclaims it"; `farm_and_seed_never_synced` (426) drops the farm asserts (448-449, ~469).
- `tests/recover.rs` (71, 100) and `tests/custody/procs.rs` (32-75: `pid_files` reads `<home>/native/*/*/*.pid`, `plant_leftover` writes `<home>/native/repo/B/…`) → the `sides/<repo16>/<slot>.pid` layout.
- `tests/custody/k2.rs` (16, 19) and matrix rows 352-365 for the `native.spawned` → `proc.spawned` rename, and src/fault.rs:55.
- `tests/run_support/mod.rs` (49-118: a daemon on a Unix socket) builds an in-process `Api` over `Services` with `Scripted`; every `run_*`, `codex_regressions`, `combine_tools`, `agent_truth`, `integrate`, `gitview` test keeps its body.
- `tests/custody/world.rs` (139, 156, 301) and `lifecycle.rs` (12, 22, 184): no daemon branch.
- `tests/verify_adhoc.rs`: the "adhoc without dry run is refused" guard (grounding §10.5, enforced at the HTTP boundary today) moves into `Api::verify` and is tested there.

**Touch (signatures only):** `DepsPolicy` → `bool` in tests/run_support/mod.rs:11,241, run_accept.rs:17,60, notices_delta.rs:16,74, coord.rs:13,85, run_scripted.rs:15,73,475, run_cross.rs:15,50, run_coord.rs:18,55, run_orchestrator.rs:16,190, disk_safety.rs:13,68, disk.rs (11 sites). `NativeConfig` helpers in build_proof.rs:17-52, rust_proofs.rs:16-52, rust_compat.rs:15-51, check_adhoc.rs:21,85-100 become `services.executor(Local)`. check_adhoc's `py_calc` (220) and ci_trust's ignored native tests (429-532) now need `pytest`/`node` on `PATH` (they ran through Aver-installed venvs before): an `#[ignore]` reason update. Every binary-driven verify on shell fixtures (custody/world on `fixtures/sh-calc`, accept_workspace, workspace_rebase, handoff, transplant, ci_scripted, init's dry run at init.rs:264, cli_workspace's `provenance.mode == "local"` at 836-849) should pass unchanged **[inference]**.

## 5. Tests and commits

### 5.1 `cargo test --test place` (crates/aver/tests/place.rs)

Each test names the defect it catches. None needs Docker; the ones that need a real compiler are `#[ignore]` under `make test-native`.

**proc**
1. A variable set in the test process (`AVER_TEST_MARK=1`) and `PATH` reach the command unchanged; `AVER_OUT` is a fresh empty directory on each run, and a file the command writes there comes back in `Output.files`.
2. `GITHUB_OUTPUT`, `GITHUB_ENV`, `GITHUB_PATH`, `GITHUB_STEP_SUMMARY`, `ACTIONS_RUNTIME_TOKEN`, `ACTIONS_CACHE_URL`, `ACTIONS_ID_TOKEN_REQUEST_URL`, `ACTIONS_ID_TOKEN_REQUEST_TOKEN`, `GITHUB_TOKEN` set in the parent are absent in the child (`env` output), each asserted by name.
3. `CARGO_TARGET_DIR=/elsewhere` in the parent is `<slot>/target` in the child.
4. A command that starts `sleep 300 &` and then `sleep 300` with a 1 s timeout returns `Timeout` within 3 s, and no process carrying the run's tag is left (`pgrep -f` on a unique marker).
5. A command that backgrounds a listener and exits 0: after `run` returns, the listener's pid is dead (descendant reaping, macOS and Linux).
6. 2 MiB of output ending in `ValueError: last line`: `Output.output` holds the first 8 KiB and ends with that line; a 10 KiB line is cut with the `[aver] line cut` marker.
7. A failing setup command and a passing check give the setup's exit code; a missing program gives "… is not installed on this machine" (the wording `aver init` matches).
8. A `<slot>.pid` naming a live group is killed when the slot is next entered; one naming a reused pid (different start time) is left alone.
9. `aver verify` on a fixture whose check writes its pid and sleeps: SIGINT to `aver` exits 130 within 3 s, the check's pid is gone, and no run row is stored for it. `aver status` exits 130 on SIGINT at once.

**workdir**
10. `become_tree(t)` leaves exactly `t`'s tracked files with their bytes and modes, removes an untracked `dist/x.js` and a `.git` file, and leaves `node_modules/` (in keep) byte-identical.
11. Swapping `a → b`: files equal in both keep inode and mtime; each changed file's mtime is newer than before the swap.
12. With `swap.partial#2=abort` (the `faults` feature, so this one runs under `make test-custody`), the next `become_tree(b)` produces exactly `b`.
13. `adopt` on a checkout with one edited tracked file returns `Dirty([that path])` and changes nothing; on a clean one it deletes untracked files outside keep.
14. After `adopt(head)` and swaps to base and transplant, removing `$AVER_HOME` entirely, `Workdir::restore` leaves `git status --porcelain` empty and every tracked file byte-equal to `HEAD`.
15. `derived` lists `dist/test/v4.test.js` written by a run (matching `built_tests`), with its blob; a second run writing different bytes gives a different blob; files under keep are never listed.
16. `probe_copies`: `.venv/lib/python3.12/site-packages/pkg/foo.py` equal to the base's `src/pkg/foo.py` is a `StaleCopy`; an editable install (only a `.pth`) gives none.

**sides**
17. **The I1 mtime test.** A fixture whose "build" copies `src/a.txt` to `out/a.txt` only when `src` is newer (`[ src/a.txt -nt out/a.txt ]`). Build in the agent directory with head content, then run the base tree in `B-`: `out/a.txt` holds the base content. Same for `J-` after the agent edits again.
18. A slot cloned from a directory with `node_modules/` sees it, and a write into the slot's `node_modules` does not reach the source (clonefile or reflink; skipped under `Copy`).
19. `relocate` rewrites the source path in `.pth`, `direct_url.json`, `.modules.yaml` and a `.venv/bin/pytest` shebang to the slot path; a relative symlink is unchanged; a binary file holding the path is listed; the source files are unchanged.
20. A slot is reused while the source's `DepsState` is unchanged (a sentinel file keeps its inode) and recloned after the source's lockfile changes.
21. Two `aver verify` processes from two workspaces on one base, started together: both finish, the base side's check runs once (it appends to a counter file outside the slot), and neither prints "waiting for another aver process".
22. `Sides::remove_for(ws)` after release leaves no `J-<ws>` or `T-<ws>`; `stale` returns `B-` slots of no live base idle past the limit and `*.tmp-*` leftovers.
23. A run outside a verification (`SlotName::Snapshot`) never creates or writes a `B-` slot.

**no daemon**
24. After `aver workspace new`, `aver verify` and `aver accept`, no `aver serve` process exists and `$AVER_HOME` has no `daemon.json` or socket; `aver serve` exits 2 with the removal line.

### 5.2 Commits, each ending green

Gate for every commit: `make lint test` and `cargo test --test place`; `make test-custody` from commit 4 on; `make test-native` at commits 4, 7 and 8. Each commit migrates its callers and deletes what it replaced in the same commit.

1. **`place/proc.rs`, extracted.** Move §1.1's process code and the WP1b-orphaned output types into `place/proc.rs`; `NativeRunner::run` calls `proc::spawn`/`reap` unchanged in behavior. Move native.rs's 4 unit tests. Tests 4-8 land. Pure move, so the existing suites prove it.
2. **`place/workdir.rs`.** `Workdir` with the journal, `adopt`, `become_tree`, `derived`, `probe_copies`, `restore`; `NativeRunner` uses `become_tree` for `src/` in place of `sync_src` (pools' farm dirs passed as keep). Fault points `swap.journaled`, `swap.partial`, `swap.done` with matrix rows. Tests 10-16.
3. **`place/sides.rs`, not yet wired.** `Sides`, slot layout, `enter`, `relocate`, `deps_state`, `deps_of`, `seed_workspace`, `remove_for`, `stale`, run-slot limiter; `slot.cloned`, `relocate.written` rows. Tests 17-20, 22-23 (17 and 20 against `Sides` directly).
4. **The flip.** `Executor` over `Arc<dyn Runs>` (`SlotRunner`, `Scripted`), `Env::inherit` (§1.4), executor ids and provenance (§1.3), the run-cache re-lookup after the slot lock; `verify.rs` slots; `cli/ci.rs`, `detect/suggest.rs`, `recover.rs`; `native.spawned` → `proc.spawned`. Delete the rest of `exec/native.rs` and `exec/netns.rs`, `tests/native_env.rs` (after its rewrites land in `tests/place.rs`), and the native helpers of the verdict suites. CLAUDE.md rule 10 and ARCHITECTURE.md §1.4.3, §1.8.5, §2.4.6 change here. Tests 1-3.
5. **Workspaces without farms.** `finish_materialize` and `tree_dir` through `seed_workspace`; `DepsPolicy` → `bool`; delete `disk/{deps,cargo,pools,usage}.rs` after moving the helpers; doctor and gc lines; `Sides::remove_for` in `release`, `Sides::stale` in `gc`; the `tests/disk.rs` and `tests/doctor.rs` changes.
6. **`Services` in the library, `Api` in process.** `services.rs`; `run::api::Api` over `Services` with the error codes; `tests/run_support` without a daemon; `aver run`, `rerun`, `accept`, `combine`, `status`, `discard`, batch `diff` and the worktree hooks stop calling `connect_or_spawn`. `tests/api.rs` takes the adhoc guard and the ownership refusals from `http_tasks.rs`.
7. **Daemon gone.** Commands call `Api`; delete `cli/ops.rs`, `cli/backend.rs`, `daemon.rs`, `cli/serve.rs` and `cli/daemonctl.rs` (one-release stubs), `http/`, `auth.rs`, the HTTP crates, the daemon and HTTP tests; `Home::lock` only for gc and keys; per-process `running/`; busy timeout 30 s; detached `warm`; CLAUDE.md rule 6 and the commands table; ARCHITECTURE.md §2.7. Tests 21, 24.
8. **Ctrl-C.** `proc::interrupt`, installed in `cli::main`; `aver run`/`rerun` in graceful mode; `Exit::Interrupted` stored as nothing. Test 9.

Commits 1-3 add code beside the old path, so a bad one reverts alone. Commit 4 is the point of no return for pools; commit 7 for the daemon. **[inference]** commit 4 is the largest (about 3,500 lines removed, 1,200 added) and the one to split if review needs it: 4a (seam and `Scripted`), 4b (the flip).

### 5.3 Risks the next engineer inherits

- No `.git` in slots. uuid's `pretest` (`if [ -d .git ]`) runs nothing in a slot; SYNTHESIS P3 expects a write-only Git view per side. That is WP3 or WP4 (`disk/gitview.rs`), and WP3's zero-evidence blocker reports it instead of a false PASS.
- Slots serialize checks per side (decision 1). Measure with `make bench-native` after commit 4.
- `aver ci` copies the checkout per slot on Linux without reflink until WP3 runs it in place (decision 2).
- Lost fairness between tasks in `aver run -n K` without `exec/slots.rs` round-robin.
- Workspace edit and search code (`workspace/edit.rs`, `search.rs`) loses its HTTP callers; `run/cross.rs` still uses `read_file`/`edit`. Whatever else is now dead there is a deletion for WP4, not WP2.
