# Dogfooding log: Aver on Aver

This log records what happened when an agent used Aver to develop Aver's own repository. Each entry gives the command, an excerpt of the output, a severity and a suggested fix. Entries marked **fixed** were fixed on the dogfooding branch, each with a test. The rest are open.

Paths are shortened: `<wt>` is the Git worktree the agent worked in, `$AVER_HOME` is a dedicated home under `<wt>/.aver-test/dogfood` (never `~/.aver`), and `<scratch>` is a temporary directory.

## Setup

- 2026-10-13, macOS on arm64, Docker with runc (no gVisor), Node 25 on the host.
- `aver` 0.3.0, built with `cargo build --release -p aver` from `origin/main` at `3eac3c3`, then rebuilt after each fix.
- The machine was heavily loaded: an evaluation, a batch build and a CI fix ran at the same time. The load average was 22 to 31 at the start and 70 to 90 at the end. Every time below is under that load, so a quiet machine is faster.

## What Aver can verify in this repository

The new root `aver.toml` pins `node:24-bookworm-slim` by digest, uses `mcp/package-lock.json` and declares two checks with `cwd = "mcp"`: the `node:test` suite with a JUnit report (per-test results), and `tsc --noEmit` (judged by its exit code). It also warms workspace directories with the checkout's Cargo `target/` directory.

What it cannot verify:

| Part | Size | Why not |
|---|---|---|
| `crates/` (Rust) | 65,662 lines; 924 of 1,908 file changes since 2026-10-01 | The sandbox runs Node images only. The dependency layer is npm-only, and the sandbox has no network, so `cargo test` cannot fetch crates. |
| `site/` | 2,370 lines of TypeScript; 142 file changes | An environment has one lockfile, so it has one `node_modules`. The site tests need `site/node_modules` (esbuild, linkedom) and also import `../../mcp/src/signing.ts`, which needs `mcp/node_modules`. |
| `tests/e2e/`, `bench/eval/` | | The same reason. The e2e test also needs Docker and a daemon. |
| `action/`, `packaging/` | | Shell tests that need tools the Node image does not have. |

`mcp/` is 3,492 lines and 104 of the 1,908 file changes since 2026-10-01. **Aver verifies about 5% of its own code.**

## Log

### 1. A nested lockfile made every check fail as ENV (blocker, fixed in `b1aa24e`)

```
$ aver ci --repo . --base <base+aver.toml> --head <43b769b+aver.toml>
BLOCKED_NOT_OWN rcpt_i36o… (exit 0)
- `mcp` ENV, base fail, head fail
  [aver] sandbox error: dependency layer: malformed sandbox result: no result; container exited 121;
  stderr: tar: mcp/package.json: Cannot open: Permission denied
  tar: mcp: Cannot utime: Operation not permitted
```

Every environment whose lockfile is not at the root failed. The install sandbox mounted the layer volume at `/work/mcp/node_modules`. Docker creates the missing parents of a mount point as root, so `/work/mcp` was not writable for the sandbox user, and the tar extraction failed. No test covered a nested lockfile in Docker.

Fix: the install now extracts `package.json` and the lockfile at the root of `/work`, with the volume at `/work/node_modules`. The layer does not depend on where the lockfile sits (`DepsKey` already left the path out). Check runs still link the layer beside the lockfile. The new test `a_nested_lockfile_installs_and_links_beside_itself` (in `exec_docker.rs`) failed with the same tar error before the fix.

### 2. The pull request that adds `aver.toml` cannot be judged by it (annoying, by design)

```
$ aver ci --repo . --base HEAD --head <HEAD+aver.toml>
BLOCKED_NOT_OWN rcpt_37y5… (exit 0)
No aver.toml at base: used the default `npm test` check …
Default check `test`: `npm test`, no test runner Aver can add a reporter to, so it is judged by its exit code alone
- `judging::aver.toml`: the PR changes the test harness; every check ran with the base's version.
```

The harness is frozen to the base, so this is correct. But this repository has no root `package.json`, so the first `aver ci` run on the PR that adopts Aver can only say `BLOCKED_NOT_OWN`. To test the configuration on real history, I grafted `aver.toml` onto both commits with `git commit-tree`.

Suggested fix: when the base has no `aver.toml` and the head adds one, say so in `summary.md` and print the `aver ci` command that previews the new configuration. That command could use a `--config-from head` flag, with the receipt marked as a preview.

### 3. A fresh home needed `aver init` first (annoying, fixed in `7f5f126`)

```
$ AVER_HOME=$AVER_HOME aver workspace new .
aver: $AVER_HOME/keys/admin.json not found; run `aver init` first
```

The README starts with `aver run` and never mentions `aver init`, so a new user's first command fails the same way. `aver run` and `aver workspace new` now initialize the home, as `aver ci` and `aver mcp` already did. The test `workspace_new_initializes_a_fresh_home` covers it.

### 4. Real history: the MCP lease-tools commit gets REVIEW, correctly (worked well)

```
$ aver ci --base <43b769b^+aver.toml> --head <43b769b+aver.toml>
REVIEW rcpt_56uc… (exit 0)        2:00 total, 45 s of it the dependency install
| Passing | NEW | CHANGED | PREEXISTING | FLAKY | ENV |
| 133     | 0   | 1       | 0           | 0     | 0   |
- `mcp::tool to HTTP mapping > every tool except task_create has a mapping case`:
  the old version of this test fails on the new code; the PR's version passes.
```

The verdict is right. The commit added MCP tools, so the exhaustive mapping test had to change. Aver shows that one test to a reviewer, and it judged the other 133 with the base's versions.

### 5. "Needs review" shows the diffs of every modified test file (annoying)

The same summary prints the full diffs of `client.test.ts` (42 added lines of new tests) and `helpers.ts`. Only `tools.test.ts` holds the `CHANGED` test. The heading says "A human reviews only those, below", and then the reviewer gets three diffs.

Suggested fix: show the diff of each file that holds a `CHANGED` entry first, and list the other modified test files by name, with their diffs collapsed.

### 6. The workspace loop on a trivial change: PASS in 6.4 s (worked well)

```
$ aver workspace new .                       7.0 s on a fresh home (import, then 3.0 s to materialize, with the mcp/ dependency store built on the host)
$ # add a comment line to mcp/test/repo-data.test.ts
$ aver check                                  6.6 s
aver check: no new failures.
The sandbox may differ from this machine: Node 25 here, Node 24 in the sandbox (node:24-bookworm-slim).
Verify should PASS, unless the sandbox differs (above): run `aver verify` for the receipt.
$ aver verify                                 6.4 s
DONE: PASS. No criteria (only regressions are checked), no new failures.
Took 6.4 s: sync 11 ms, plan 9 ms, original code cached, your code cached, your tests 2 runs in 6.4 s
```

The second workspace on a warm checkout took 385 ms. The base had been warmed in the background, and `aver check` started the verify's runs, so `aver verify` found them in the cache. The native suite takes 4.5 s on this machine, and a sandbox run took 20 to 30 s under load.

### 7. A criterion test that already passes is caught (worked well)

```
$ aver workspace new . --label crit -c "escape-upper: escapeRepoData escapes an upper-case </REPO-DATA closer"
$ # add "[crit:escape-upper] escapeRepoData escapes </REPO-DATA" (the code already does this)
$ aver check
  escape-upper  NOT YET: passes with your change ✓, but also passes on the original ✗, so it proves nothing.
Verify would stop at UNPROVEN: escape-upper's test also passes on the original code.
```

This is a mistake agents make: the criterion was already true. Then I wrote a test for a real change (escape NUL bytes) and implemented it. `aver check` said `WOULD PROVE`, and `aver verify` said `DONE: PASS. 1 of 1 criteria proven` in 97 s under load. Across the three cases, `aver check` predicted the `aver verify` verdict every time.

### 8. A test tagged with the wrong criterion id gets no hint (polish)

I first tagged the new test `[crit:escape-nul]`, but the task's criterion is `escape-upper`:

```
  escape-upper  NOT YET: no test tagged [crit:escape-upper] ran. Add one …
```

`aver check` saw a tag that names no criterion of the task and said nothing about it. Suggested fix: "a test is tagged `[crit:escape-nul]`, which is not a criterion of this task (criteria: `escape-upper`)".

### 9. `aver export` of a verify head failed (annoying, fixed in `57675c3`)

```
$ aver export --branch dogfood/doc-comment snap_ooux… <scratch>/export.git
aver: git fast-import failed: mark :692 was not exported (INTERNAL)
```

A snapshot with no transition (a verify head) is exported as a root commit. That commit gets a mark, but no transition commit is written, so the export skipped reading the marks file. The check now asks whether anything was streamed. The test `snapshots_without_lineage_and_bad_branches_are_rejected` now also exports the lone snapshot.

### 10. A rebuilt binary kept talking to the old daemon (annoying)

After the export fix, the same command failed the same way until `aver daemon stop`, because the home's daemon was still the old binary. `aver daemon status` shows the pid and socket, but not the build. Aver developers rebuild all the time, so this happens to them often.

Suggested fix: record the binary's path and build id in `daemon.json`. When they differ from the CLI's, restart the daemon, or at least warn.

### 11. No supported way back from a workspace to Git (blocker for the subagent workflow)

```
$ aver accept att_01M4VN63… --branch dogfood/ws-doc-comment
aver: no runs yet; start one with aver run
$ aver export --branch dogfood/doc-comment snap_ooux… <scratch>/clone.git      (after fix 9)
4a45c1e  aver  Thu Jan 1 00:00:00 1970 +0000  Aver-Snapshot: snap_ooux…   (no parent)
```

`aver accept` works only for runs. A workspace from `aver workspace new` or the `WorktreeCreate` hook can be exported only as a parentless commit of the whole tree, authored by `aver` at time 0. An agent's commits in the Git view stay in `$AVER_HOME/git/<ws>`, and `WorktreeRemove` deletes them. Two workarounds work today, described in [docs/DOGFOODING.md](../../docs/DOGFOODING.md): fetch from the view (`git fetch $AVER_HOME/git/<ws> HEAD:refs/heads/<branch>`), or apply the output of `aver diff`.

Suggested fix: let `aver accept --workspace <dir>` turn any workspace into a branch on its base commit, as the repository's identity. `WorktreeRemove` should also keep a workspace that has changes nobody accepted.

### 12. Commits in a workspace's Git view used the global identity (annoying, fixed in `42ae90e`)

```
$ git -C <workspace> commit -am "dogfood: subagent commit in an Aver workspace"
$ git -C <workspace> log -1 --format='%an <%ae>'
<the machine's global name and personal address>      the repository sets user.name wonkylamp locally
```

The view is a separate repository, so it did not see the user repository's local `user.name` and `user.email`. This repository forbids personal addresses in commits, so a subagent would have broken that rule without knowing. The view now copies the repository's local identity. The test `gitview_commits_as_the_repository_identity` failed without the fix.

### 13. The receipt id is hard to find after a workspace verify (polish)

The agent report prints no ids, by design. `aver status` says `no runs yet` for workspace flows, and `aver receipt` needs an id. I found the id only with `aver verify --json`, which runs verify again (from the cache).

Suggested fix: end the report with `Receipt: rcpt_…`, or add `aver receipt --last` inside a workspace.

### 14. `aver diff <path>` in a workspace is read as an attempt number (polish)

```
$ aver diff mcp/src/repo-data.ts        (inside a workspace)
aver: no runs yet; start one with aver run
```

Suggested fix: inside a workspace, treat an argument that is a path as a path filter.

### 15. `aver integrate claude` in a linked worktree writes the shared exclude file (annoying)

`integrate` finds `info/exclude` through `git rev-parse --git-common-dir`. In a linked worktree, that file is the main checkout's `.git/info/exclude`, which every worktree shares. So I ran the integration in a scratch clone instead of the worktree, because the main checkout must not change. Git has no exclude file per worktree, so the behavior is reasonable, but the prompt should say that the change also applies to the main checkout and the other worktrees.

### 16. Claude's MCP entry bypasses `aver mcp` and needs `npm ci` in `mcp/` (annoying)

```
$ aver integrate claude          (.mcp.json)
"command": "node", "args": ["<wt>/mcp/src/server.ts"], "env": {"AVER_KEY_FILE": …}
$ aver mcp                       (source checkout, before npm ci in mcp/)
Error [ERR_MODULE_NOT_FOUND]: Cannot find package '@modelcontextprotocol/sdk' imported from <wt>/mcp/src/server.ts   (exit 1)
$ <scratch>/bin/aver mcp         (binary copied out of the checkout)
aver: no Aver MCP server found: pass --server <aver checkout>/mcp/src/server.ts or set AVER_MCP_SERVER   (exit 2)
```

docs/INTEGRATIONS.md says each tool runs the server through `aver mcp`. That is true for Cursor, Codex, Gemini and VS Code, but the Claude integration runs `node` directly. Both ways need Node 24 or later on `PATH` and `npm ci` in the checkout's `mcp/`, and no page says so. With dependencies installed, `aver mcp` answered `initialize` and `tools/list` over stdio. A binary-only install has no MCP server, as expected, and the message says what to do.

Suggested fix: use `aver mcp` for Claude too, check for `mcp/node_modules` and print "run `npm ci` in <checkout>/mcp", and ship the server bundled with the release binary.

### 17. Small output problems in `aver integrate` (polish)

- One line of the prompt starts in the middle of a sentence: "and list .claude/settings.local.json and .mcp.json in .git/info/exclude, so Git ignores them".
- Hooks and `.mcp.json` name the binary by its absolute path (`<wt>/target/release/aver`). When that worktree is removed, every hook breaks. Suggested fix: warn when the binary is inside a Git checkout or a `target/` directory.

### 18. A home inside the repository nests workspaces inside the project (polish)

With `$AVER_HOME` under the worktree, workspace directories sit inside the project tree. Claude Code then loaded each workspace's `CLAUDE.md` as nested memory whenever the coordinating agent read a file there. Workspace labels are also named after the worktree directory (`work/agent-a8876e18bf9d051a1/`), not the repository. Suggested fix: warn when `$AVER_HOME` is inside the repository, and name a linked worktree's directory after its main repository.

### 19. `WorktreeRemove` deletes work without asking (annoying, fixed)

Fixed after the owner's manual test with `claude --worktree`. Claude Code offers Keep or Remove on exit, but it could not count the changes, because its own check could not read the view's layout. The view is now a linked worktree on the branch `aver/wt-<name>`, which Claude Code reads. The hook keeps a workspace whose view holds commits the repository lacks and prints the `git fetch` that takes them (`worktree_remove_keeps_unfetched_commits_and_says_how_to_fetch_them` in `crates/aver/tests/gitview.rs`). The original entry follows.

```
$ aver hook worktree-remove < remove.json       (a workspace with one unfetched commit)
aver: removed workspace ws_01M4VNTH…
```

The hook discards the workspace and its Git view even when they hold changes or commits nobody fetched. I could not check from the sandbox whether Claude Code calls `WorktreeRemove` only for a worktree with no changes. The manual check is in docs/DOGFOODING.md.

### 20. Every `aver ci` warns about gVisor (polish)

`aver: warning: Docker does not list the gVisor runtime (runsc), so checks run under plain Docker (runc)` appears on every run, and again in `summary.md`. Once per home would be enough on a developer machine.

### 21. A change that breaks the Rust build gets DONE: PASS (blocker for this repository)

```
$ printf '\nthis is not Rust;\n' >> crates/aver-core/src/lib.rs      (inside a workspace)
$ aver verify
DONE: PASS. No criteria (only regressions are checked), no new failures.
Next: nothing. You are done; stop here.
Your changes: 1 file (crates/aver-core/src/lib.rs), all recorded.
```

Aver did exactly what this repository's `aver.toml` asked: it ran the `mcp` checks, and nothing they cover changed. But the report tells the agent to stop, and nothing says that no check looked at the changed file. In this repository, that is most changes.

Suggested fix: see top fix 2. When no check covers a changed path, the verdict should not be plain `PASS`. The report should list those paths and say which commands the agent must still run.

## Installation paths

Each path was tried without touching `~/.claude` or any other global configuration. Before and after, `grep '"aver'` found nothing in `~/.claude/plugins/known_marketplaces.json`, `~/.claude/plugins/installed_plugins.json` or `~/.claude/settings.json`.

| Path | Result | Notes |
|---|---|---|
| 1. Skill only: copy `integrations/claude-code/skills/aver/` to `<project>/.claude/skills/aver/` | Layout correct (`SKILL.md` with `name` and `description` frontmatter, 6.2 KB); discovery not checked | `claude -p` cannot log in from the sandbox. No page documents this path. The manual test is in docs/DOGFOODING.md. |
| 2. Plugin from the repository's marketplace | **Works** | `claude plugin validate integrations/claude-code` and `claude plugin validate .` passed. With `CLAUDE_CONFIG_DIR=<scratch>/claude-cfg` (no TTY, no login): `claude plugin marketplace add <wt>` gave "Successfully added marketplace: aver", and `claude plugin install aver@aver` gave "Successfully installed plugin: aver@aver (scope: user)". `claude plugin details aver@aver` lists 5 skills (`aver`, `aver-check`, `aver-run`, `aver-status`, `aver-verify`), 3 hooks (PreToolUse, PostToolUse, Stop), 1 MCP server (`aver` → `aver mcp`) and about 236 always-on tokens. It writes `extraKnownMarketplaces` and `enabledPlugins` to the config dir's `settings.json` and a copy to `plugins/cache/aver/aver/0.1.0`. The MCP server fails on a binary-only install (entry 16). The commands are skills, not `commands/` files; they still appear as slash commands. |
| 3. `aver integrate claude --yes` | **Works** | In a scratch clone: writes `.claude/settings.local.json` (PreToolUse, PostToolUse, WorktreeCreate, WorktreeRemove, `enabledMcpjsonServers`) and `.mcp.json`, and adds both to `.git/info/exclude`, so `git status` stays clean. A second run says "already set up for Aver". A simulated `WorktreeCreate` made an Aver workspace in 6.8 s (with the integration's files copied in). `aver verify` there said PASS, and `WorktreeRemove` deleted it. |
| 4. Claude Code desktop app | Not verifiable here | Plugins that the CLI installs go to `~/.claude/plugins` and `enabledPlugins` in `~/.claude/settings.json`. The desktop app's Code tab reads the same config directory, so they should appear there, but I could not open the app. The manual check is in docs/DOGFOODING.md. |
| `aver integrate cursor`, `codex`, `gemini`, `vscode`, `agents-md` | **Work** | All exit 0 in a scratch repository. The formats match docs/INTEGRATIONS.md: `.cursor/mcp.json` (`mcpServers`, `type: "stdio"`) and `.cursor/rules/aver.mdc` (`description`, `alwaysApply: false`); `.codex/config.toml` (`[mcp_servers.aver]` inside a `# BEGIN aver` block); `.gemini/settings.json` (`mcpServers`, no `type`); `.vscode/mcp.json` (`servers`, `type: "stdio"`); and one marked block each in `AGENTS.md`, `GEMINI.md` and `.github/copilot-instructions.md`. The four files that name machine paths are in `.git/info/exclude`. The instruction files are left untracked for you to commit. `agents-md` after `codex` correctly says "already set up". |

**Uninstall.** The plugin removes cleanly: `claude plugin uninstall aver@aver` and `claude plugin marketplace remove aver` left `enabledPlugins: {}` and `extraKnownMarketplaces: {}`, plus a cache copy marked `.orphaned_at` for Claude Code to sweep. `aver integrate` has no remove command. You remove the integration by hand: delete the files or the marked blocks, delete the lines in `.git/info/exclude`, delete the key `$AVER_HOME/keys/<tool>-<repo>.json`, and revoke the agent. Merged JSON (`.claude/settings.local.json` with your own permissions in it) must be edited by hand. Suggested fix: `aver integrate <tool> --remove`, which undoes exactly what the command wrote.

## What worked well

- The verdicts were right on real history and on real changes: REVIEW for an intended test change (entry 4), UNPROVEN for a criterion that was already true (entry 7), and PASS for a proven criterion.
- `aver check` predicted `aver verify` every time, in 5 to 9 s, and its "sandbox may differ" line caught the host's Node 25 against the sandbox's Node 24.
- Prefetch and the base cache make `aver verify` cheap after `aver check`: 6.4 s with everything cached.
- Workspace directories are fast and clean: 0.4 to 3 s, `mcp/node_modules` farmed, `git status` clean, and a 7.2 GB `target/` cloned in 1.4 s with `[workspace] warm`. A `cargo build -p aver-core` there took 18 s and reused most compiled dependencies.
- Coordination works without setup. Verify in the second workspace reported that the first workspace's task also changed `mcp/test/repo-data.test.ts`.
- The integrations are project-local, idempotent and leave Git clean. The plugin installs and uninstalls without a login or a TTY.

## Rating

**3/10** against "I absolutely need this for developing Aver itself".

For a change to `mcp/`, Aver is already better than what we do now. The workspace is instant, `aver check` is fast and was right every time, and the receipt tells a reviewer exactly which test expectation changed. On that 5% of the code I would rate it 7.

But almost all of Aver's development is Rust, and Aver cannot run `cargo test`. A change that does not even compile gets `DONE: PASS. … You are done; stop here.` (entry 21), and a subagent will believe it. The second problem is the subagent workflow. A workspace has no supported way back into Git, and `WorktreeRemove` deletes unfetched commits. Until those two change, Aver is a nice extra for `mcp/` and a risk elsewhere, not something I need.

## Top 5 fixes that would most raise the rating

1. **Verify Rust.** Add a Cargo dependency layer (crates fetched once with network, like npm's), a `cargo test` check with per-test results (libtest JSON or nextest JUnit) and an image allowlist that includes `rust`. This alone moves the share Aver verifies from 5% to over 90%.
2. **Make an uncovered change visible.** When a change touches no file that any check reads (no test, source or harness under any check's `cwd`), say "no check covers your changes" and refuse plain `PASS`. Today a Rust change gets `DONE: PASS`.
3. **Get work back into Git from any workspace.** Add `aver accept --workspace <dir>`, which creates a branch on the base commit as the repository's identity, and make `WorktreeRemove` keep a workspace that holds changes or commits nobody accepted.
4. **Allow several dependency layers per environment** (one per lockfile, or npm workspaces at the root), so `site/` and `tests/e2e/` verify too.
5. **Restart the daemon when the `aver` binary changes**, and show the receipt id at the end of the agent report. Aver's own developers rebuild constantly, and a stale daemon silently runs old code.

## Round 10: Rust, site/ and the e2e typecheck

Generic mode addressed fixes 1, 2 and 4. The root `aver.toml` now declares four dependency layers (`mcp/`, `site/`, `tests/e2e/` and `Cargo.lock`) and runs `cargo test` in the pinned `rust` image beside the Node checks. Every run below is `aver ci --repo .` on a commit of this branch with a fresh home; the layers were already installed.

| Head | Verdict | Entries | Wall time | Load average |
|---|---|---|---|---|
| A comment in `crates/aver-core/src/lib.rs` | `PASS` | 643 passing | 7.3 min (two Rust builds of 7 min, in parallel) | 10 to 30 |
| `this is not Rust;` appended to the same file (entry 21) | `FAIL_OWN` (`rust` NEW: the build fails) | 189 passing, 1 NEW | 7.0 min | 20 to 50 |
| A comment, before two Node-dependent tests were skipped | `PASS`, 2 `PREEXISTING` | 623 passing | 28 min: the base's failing first run was rerun twice, three builds of 12 to 15 min | 60 to 130 |

What changed the numbers:

- The first attempt ran out of memory at 3.5 GiB: four parallel build jobs linking test binaries into a tmpfs `target/`. Two jobs, no debug info, stripped symbols and GNU ld's `--no-keep-memory` keep a build at about 2.2 GiB.
- Two tests run `node`, which the `rust` image does not have. They failed on every side, proved nothing and cost two extra base builds, so the check skips them; `make test` still runs them.
- The `bench` check runs `bench/eval`'s harness tests in the full `node:24-bookworm` image, because the scenarios create Git repositories and the slim image has no `git`.

Coverage: `aver verify` now gates the Rust crates, `mcp/`, `site/`, `bench/` and the `tests/e2e/` typecheck. A change to a source file none of them covers, such as a Rust example, is `UNPROVEN` with the file listed. Each Rust run still builds every dependency from scratch, which is most of its time: see ARCHITECTURE.md §2.4.2 for why there is no build cache yet.


## Round 11: dogfood polish

The first real subagent run inside an Aver workspace rated it 5/10. Its change was one line appended to `docs/DOGFOODING.md`. What it found, and what changed:

| Finding | Fix |
|---|---|
| `aver verify` took 12 min 48 s, all of it the Rust suite on both sides, for a documentation line. | **Affected checks.** A check whose `covers` (else its `cwd`), mounted lockfiles and harness the change does not touch runs on neither side and is listed in the receipt's new `skipped` field (v2, additive). `--all` runs every check. The rule is in docs/VERDICTS.md, "Which checks run". This repository's `aver.toml` now names in `covers` everything each check's tests read (`schemas/`, the plugin and the bundled MCP server for `rust`, `mcp/src/` for `site`). |
| Nothing printed for 10 minutes. | `aver verify` and `aver check` print progress on stderr: a line per sandbox run that ends, a count of cache hits, the runs still running after 2 s, a heartbeat every 30 s. `--quiet` stops it. |
| `aver check` said "cannot predict": `node_modules/.bin/tsc` was missing in `site/` and `tests/e2e/`, and the native `cargo test` reported "malformed libtest output". | Every npm layer gets a farm beside its lockfile. The native `cargo test` failed to link on macOS: the `rust` check passed GNU ld's `--no-keep-memory`, which Apple's linker rejects; the flags are now set on Linux only. The libtest parser names a build failure ("the build failed: error[E0425]: …"), and `aver check` reports a check it cannot run here ("can't preview `rust` here (…); verify will run it") and predicts the rest. |
| No `.aver/BRIEF.md` in the workspace `WorktreeCreate` made. | The hook and `aver workspace new` write one: the task, how to check and verify, the budget, the branch, the fenced-data note. |
| `DONE: PASS` twice; "Add a test" after a documentation change. | A report of at most 12 lines has no closing verdict line. The evidence warning is left out when every changed file is documentation, configuration or data, or no check ran. |
| 6 Rust tests fail in the sandbox (no Node, a 1.7 GiB tmpfs, ETXTBSY). | The five Node-dependent tests are `#[ignore]`d (`make test-docker` runs them), so the `--skip` flags left `aver.toml`; the doctor tests count the disk check's verdict; the MCP binary test retries an exec that a forking test thread made busy. |

The follow-ups:

- **Per-error type checks.** The `tsc` report format makes each type error an entry keyed by file, code and message (not line). The default type check and this repository's three type checks use it, so a base whose type check already fails no longer blocks every verify.
- **One MCP server.** `aver integrate claude` (and `aver setup`) leaves `.mcp.json` alone when Aver's Claude Code plugin is enabled.
- **A reinstalled binary.** An auto-started daemon records its binary and exits once it is idle after the file changes; the next command starts the new one. Background warms and prefetches count as busy, so they finish first. In the trial below, rebuilding `target/release/aver` while the demo daemon was idle restarted it.
- **Sibling tasks.** A batch agent's brief names the batch's other tasks from the start.
- **`aver workspace rm`** refuses a workspace with unfetched commits and prints the fetch commands; `--force` removes it.

While this round's agent worked, the `PostToolUse` hook said another task (a Claude Code worktree session) had changed `docs/DOGFOODING.md` too, with no overlapping lines: coordination worked without setup.

### Measurements

macOS arm64, Docker 29 (runc, 8 CPUs, 8 GiB), load average 2 to 6. `aver ci` with a new home each time unless noted; base commit `394165e`.

| Change | Before | After |
|---|---|---|
| One line in `docs/DOGFOODING.md`, `aver ci` | 12 min 48 s (the trial, in a workspace) | **1 s**: `PASS`, no entries, all 7 checks `skipped` |
| The same in a workspace, `aver verify` / `aver check` | | 24 ms / 0.1 s: `DONE: PASS. No check is affected by this change, so none ran.` |
| A comment in `crates/aver-core/src/lib.rs`, `aver ci` | 7.0 to 7.3 min (round 10, every check) | **6 min 15 s** (375 s): `PASS`, 501 per-test entries, 6 Node checks skipped; only `rust` ran, base and head in parallel, the Cargo layer fetched first |
| Another comment there, base already cached | | 5 min 39 s (339 s): only the head's build |
| The Rust comment in a workspace, `aver verify` | | 6 min 15 s, with a progress line every 30 s |
| A comment in `mcp/src/routes.ts`, `aver ci` (the npm layers installed first) | | **16 s**: `PASS`, 190 entries; `mcp`, `site` and their type checks ran, `rust`, `bench` and `e2e-typecheck` skipped. A clean type check has no entries (its log is empty), only runs |
| The Rust comment in a workspace, `aver check` (native) | "cannot predict" | 3 min 11 s: `rust` passed on both sides, "Verify should PASS"; the original's copy now starts with the workspace's `target/` |

The Rust time is the build itself: two jobs, no debug info, every dependency compiled in the sandbox (ARCHITECTURE.md §2.4.2). Selection removes the rest.

### Rating

**6/10** for working inside Aver on this repository. A documentation or `mcp/` change now verifies in seconds to a minute, and every report says what ran and what did not. A Rust change still costs six minutes per verify, which is the main thing between this and "I need it": a build cache for the `rust` check's sandbox would bring it down to the change's own crates.

## Round 12: build cache

Dogfooding found two Rust costs: workspace directories that grew to 14 to 17 GB each and took the laptop to 3 GB free, and a sandboxed Rust verify that spent about 6 minutes building per side. What changed (ARCHITECTURE.md §2.3.1, §2.4.2):

| Finding | Change |
|---|---|
| Each workspace cloned the main checkout's whole `target/` (14.1 GiB here). Copy-on-write makes the clone free when it is made, but it pins every block it holds, and the main checkout's own builds replace their artifacts, so the clone turns into unique disk. | A warm Cargo `target` is cloned with its dependency units only (1.4 GiB of the 14.1): never the workspace's own units, incremental state, test binaries or release output. The same filter applies to `aver check`'s copies. |
| The whole-`target` clone was also unsound: Cargo judges a workspace crate fresh by mtimes, and the clone carries artifacts built from another tree. | The workspace's units are left out, so Cargo rebuilds them from the workspace's own files. |
| Nothing reclaimed the build output of finished or idle workspaces. | `aver gc` moves the Cargo `target` directories and `aver check` copies of workspaces idle for a day to the trash; `aver gc --builds` does it after 10 minutes and removes all but the two newest build cache volumes. `aver doctor` reports the build artifacts in workspace directories and the cache volumes, with a warning when they are large or the disk is low. |
| Every sandboxed Rust run compiled every dependency. | A Cargo build cache per check and lockfile (`aver-build-<key>`, 499 MiB for this repository): the dependencies compiled once, without network, by the check's own command over the lockfile, the manifests and stub sources; the workspace's own units are deleted from it; each run copies it into its own scratch `target`. No run writes anything another run reads. |

### Measurements

macOS arm64, Docker 29 (runc, 8 CPUs, 8 GiB). The machine was shared with an evaluation and other agents' builds; the load average is given for each row, and it moves the times more than anything Aver does.

| What | Time | Load |
|---|---|---|
| Round 11 baseline: `aver ci`, a comment in `crates/aver-core/src/lib.rs`, no build cache | 375 s | 2 to 6 |
| The same with the build cache, which this run built first | 424 s: the cache 43 s, then base and head runs of 378 s and 375 s in parallel; each compiled only `aver-core` and `aver` (`Finished` after 5 min 38 s) | 5 to 10 |
| Again, cache built (a new home, so the base ran too) | 526 s: cache found in 1.7 s, runs of 522 s and 503 s, compiling only `aver-core` and `aver` (7 min 30 s) | 15 to 20 |
| Native `cargo test --workspace --lib --bins --tests`, warm (after `aver check` built it), after a one-line change to `aver-core`, 2 jobs | 522 s, 5 min 21 s of it compiling | 110 to 125 |
| `aver check` in this workspace (installed binary, no `target/`) | 1,237 s: three cold native builds in parallel (your copy, the judged head, the original code) | about 100 |
| A new workspace on a clone whose `target/` is 2.37 GiB, then a native `cargo build -p aver --lib --bins` | `target/` 0.60 GiB at creation (616 MB), 880 MB after; the build took 29 s and compiled only `aver-core` and `aver` | 15 to 20 |
| This agent's workspace, made with a whole-`target/` clone, after 40 minutes in which nothing built in it | deleting its `target/` freed 3.2 GiB | |

What the numbers say:

- **Disk per workspace.** A warm workspace now starts with 1.4 GiB of dependency units (apparent size, shared with the main checkout until it rebuilds them) instead of 14.1 GiB, and its growth is its own builds. A whole-`target/` clone pinned 3.2 GiB of unique disk within 40 minutes without a single build in it.
- **Verify time.** The build cache works: both sides compile only the workspace's crates. But the dependencies were not where the time went. They compile in 43 s with two jobs; the remaining 5.5 minutes per side is this repository's own code: `aver` (48,000 lines) compiled twice (library and unit tests), one binary, and about forty integration test binaries linked, all with two jobs to stay inside 3.5 GiB. A change to `aver-core` rebuilds all of it, natively too (5 min 21 s of the warm native run above, under heavy load). The target of a warm verify at most 1.3 times a warm `cargo test` is not met in the sandbox: a sandboxed run cannot reuse an earlier build of the workspace's own crates, and natively they are reused incrementally.
- **Where the time goes in a sandboxed Rust verify:** sandbox start and snapshot extraction, a few seconds; dependency layer, 0 (installed); build cache, 43 s once per lockfile and command, then 1.7 s; compiling the workspace, 5.5 to 7.5 minutes; the tests, about 40 s; the base side runs in parallel with the head and is cached for later verifies on the same base; the transplant side (when test files change) runs three times.
- **What would close the gap** (native verify, which the owner chose as the default, gets most of it for free): a base-side build of the workspace's own crates reused by head runs, with each file's mtime set to 0 when it matches the base and to the current time when it does not, so Cargo rebuilds exactly what changed; incremental compilation kept in that cache; more build jobs, now that dependencies no longer compete for memory; one transplant run instead of three for slow checks; and, for this repository, fewer integration test binaries.

### Dogfooding notes

- `aver check` with no `target/` in the workspace started three cold native builds of the whole repository at once and took 20 minutes under load; the original code's copy then failed to compile because a new test called a new method, and check reported "can't preview `rust` here" for the whole check instead of treating that test as evidence that only passes with the change.
- `aver verify` on a change that touches `bench/` (a new script) ran the `bench` check as well, and on a change that edits existing Rust test files it started the transplant side three times at once: twelve sandbox runs for one change. With the installed binary (no build cache) and the home's daemon shared with other agents' verifies, the run grew to 21 sandbox runs (a failing first run on the judged head was rerun twice) and was stopped after 2 h 43 min, each Rust run queued behind other agents' runs for the daemon's sandbox slots. No verdict.
- The `PostToolUse` hook reported that other tasks had leased `docs/CLI.md`, `crates/aver/src/cli/doctor.rs` and `crates/aver-core/src/lib.rs` after this agent had changed them. That was useful: the agent moved its re-export out of `lib.rs`.
