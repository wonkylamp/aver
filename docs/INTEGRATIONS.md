# Integrations with agent tools

Aver's workflow is the same in every tool: edit in a workspace directory, run `aver check` while iterating, run `aver verify` to finish, and prove each criterion with a test tagged `[crit:<id>]`. Agents run the `aver` CLI directly. Each integration gives the tool's agent that workflow.

There are two ways in, and they give the same experience. Install the Claude Code plugin and run `/aver`, or install the `aver` CLI and run `aver init` in the repository. Both end with the same files in the repository, because the plugin's skill and commands are the files `aver init` writes.

| Tool | Command | What it writes |
|---|---|---|
| Claude Code | `/plugin marketplace add wonkylamp/aver`, then `/plugin install aver@aver`, then `/aver` | The plugin in [integrations/claude-code](../integrations/claude-code): the `aver` skill, `/aver`, `/aver-run`, `/aver-check`, `/aver-verify` and `/aver-status`, and the session hooks. `/aver` installs the CLI if it is missing and runs `aver init`. |
| Any repository | `aver init` | `aver.toml`, and for the agent tools the repository or machine shows: Claude Code's files below, and a block in `AGENTS.md` for Codex or an existing `AGENTS.md` |
| Claude Code, one repository | `aver integrate claude` | `.claude/settings.json` (the worktree hooks, the session brief and the verify gate), `.claude/skills/aver/SKILL.md` and `.claude/commands/aver*.md` |
| Cursor | `aver integrate cursor` | the rule `.cursor/rules/aver.mdc` |
| Codex CLI | `aver integrate codex` | a block in `AGENTS.md` |
| Gemini CLI | `aver integrate gemini` | a block in `GEMINI.md` |
| VS Code (Copilot) | `aver integrate vscode` | a block in `.github/copilot-instructions.md` |
| Anything that reads AGENTS.md | `aver integrate agents-md` | a block in `AGENTS.md` |
| Every tool found on this machine | `aver setup` | the targets above for `claude`, `cursor`, `codex`, `gemini` and `code` on `PATH` (or their macOS apps) that the repository shows it uses, after one question; `--tool <name>` picks instead |

`aver init` sets up Claude Code when `claude` is installed or the repository has `.claude/`, and the `AGENTS.md` block when the repository has `AGENTS.md` or `.codex/`, or `codex` is installed. `aver setup` sets up Claude Code wherever it is installed, and Codex when it is installed or the repository has `.codex/`. The other tools write files you may commit, so `aver setup` sets one up only when the repository shows it uses it: `.cursor/`, `.gemini/` or `GEMINI.md`, and for VS Code an existing `.github/copilot-instructions.md` (a `.vscode/` directory shows only the editor, not Copilot). On a terminal it asks about each other tool it found; `--tool <name>` (or `--tool copilot`) sets one up anyway.

## How `aver integrate` behaves

- **Project-local only.** It writes inside the repository and never touches global configuration such as `~/.cursor`, `~/.codex`, `~/.gemini` or `~/.claude`.
- **It shows, then asks.** It prints every file it will write with the file's full new contents, and writes only after you answer yes. `--yes` skips the question. Without a terminal and without `--yes`, it writes nothing and exits 2. `aver init` asks the same way and, without a terminal or `--yes`, lists the files and writes none of them.
- **It merges.** In `.claude/settings.json` it sets only Aver's hooks and keeps every other hook and setting (keys are written in sorted order). Markdown files get one marked block, from `<!-- BEGIN aver` to `<!-- END aver -->`, and the rest of the file is kept. Running the command again replaces the block, so you never get two. Codex and `agents-md` share the block in `AGENTS.md`. The skill and command files are Aver's own and are replaced with this version's.
- **Everything it writes is yours to commit.** No file holds a machine path. Each hook calls `aver` from `PATH` behind a guard, `command -v aver >/dev/null 2>&1 && aver hook <name> --rev N || true`, so a teammate without Aver gets no error and no block. The worktree hooks must print a directory, so without `aver` they fall back to a plain `git worktree` under `.claude/worktrees/`. The guard keeps a Stop block: Claude Code reads the block from the JSON `aver` prints with exit 0, not from an exit code.
- **It moves the hooks out of `.claude/settings.local.json`.** Earlier Aver builds wrote their hooks there with this machine's paths. Setting up again takes Aver's hooks out of that file, keeps everything else in it (and deletes it if nothing is left), and writes the hooks to `.claude/settings.json`.
- **It cleans up after earlier Aver builds.** They registered an MCP server (`aver mcp`) in `.mcp.json`, `.cursor/mcp.json`, `.codex/config.toml`, `.gemini/settings.json` and `.vscode/mcp.json`, approved it in the Claude settings, and installed the hooks `pre-edit`, `post-edit`, `pre-bash`, `post-bash`, `pre-agent` and a `SubagentStart` brief. Setting up again takes Aver's server and approval out (deleting a file that held nothing else; a file Aver cannot parse, such as JSON with comments, is left alone) and replaces every Aver hook with the current set. Until then, `aver hook <removed name>` exits 0 and prints nothing, so stale settings never fail a tool call.
- **It can be undone.** Each write is noted in `.git/aver/integrate.json` with the file as it was before Aver first wrote it. `aver setup --remove` puts back each file that still holds what Aver last wrote, byte for byte (deleting the ones Aver created, and the directories it created for them), takes the lines an older Aver added out of `.git/info/exclude`, and deletes the record. A file changed since, or one written by an older `aver` without a record, loses only Aver's parts: Aver's hooks (the removed names too), the old `aver` MCP server and its approval in JSON, the marked block in Markdown and TOML, and a skill or command file still equal to Aver's; a committed JSON file that is left as `HEAD` holds it gets `HEAD`'s bytes back, and a record that does not parse is deleted after the files are cleaned. An old MCP server is never put back, and neither are the hooks taken out of `.claude/settings.local.json`.
- **It says the same thing everywhere.** Every instruction file carries the body of the plugin's [`aver` skill](../integrations/claude-code/skills/aver/SKILL.md), so all tools teach one workflow. A test checks that every `aver` command and flag in the skill and the commands exists in `aver --help`.

## Claude Code plugin

The repository is a plugin marketplace ([.claude-plugin/marketplace.json](../.claude-plugin/marketplace.json)) with one plugin, `aver`:

- **The `aver` skill.** It teaches the workflow: the check and verify loop, the verdicts `PASS`, `REVIEW`, `FAIL_OWN`, `UNPROVEN` and `BLOCKED_NOT_OWN`, the causes `NEW`, `CHANGED`, `PREEXISTING`, `FLAKY` and `ENV`, and the rules (never edit existing tests or the harness to pass). Claude loads it when it works in an Aver workspace.
- **Commands.** `/aver` sets Aver up. Without the `aver` CLI it offers the two ways to install it, `curl -fsSL https://raw.githubusercontent.com/wonkylamp/aver/main/site/install.sh | sh` or `cargo install --path crates/aver` from a checkout of Aver, and runs the one you pick. With the CLI it runs `aver doctor`, says what `aver init --yes` writes, and runs it after you say yes. `/aver-run "<task>"` starts `aver run --detach` and follows it with `aver status`. `/aver-check`, `/aver-verify` and `/aver-status` run those commands and act on the result. You start these commands yourself; Claude does not start them on its own.
- **Hooks.** `SessionStart` gives a session working in an Aver workspace the short brief: done means `aver verify` prints `DONE: PASS`, check with `aver check`, verify to finish, where commits go. In the checkout of a repository set up with Aver it says to run `aver workspace new` before the first edit instead. `Stop` and `SubagentStop` run the verify gate: for an agent of `aver run`, a verify; elsewhere, one block when the workspace changed files and no `PASS` or `REVIEW` receipt is for them (see below). Each hook runs `aver hook <name> --plugin` and does nothing outside an Aver workspace or set-up repository. An ordinary session is never blocked.
- **Without the CLI.** When `aver` is not installed, `SessionStart` tells the session, from the shell alone: "Aver plugin is installed, but the `aver` CLI is missing. Run `/aver` to install the binary and initialize this repository." The other hooks print nothing. Every hook exits 0.

The hooks find `aver` through [`scripts/aver.sh`](../integrations/claude-code/scripts/aver.sh). Apps started from the Dock on macOS, the Claude desktop app among them, get launchd's `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), which holds none of the places `aver` is installed. So the script looks for `$AVER_BIN`, then `aver` on `PATH`, in `~/.local/bin` (where `install.sh` puts it), `/opt/homebrew/bin`, `/usr/local/bin` and `~/.cargo/bin`. `aver doctor` says whether GUI apps find `aver` themselves. To try the plugin from a clone, run `claude --plugin-dir integrations/claude-code`. `make plugin-check` checks its structure (see Tests).

**The plugin and the repository settings together.** With the plugin installed in a repository that `aver init` set up, both run `aver hook session-start` and `aver hook stop` for one event, at the same time and with the same input. The first claims the event and acts; the other exits 0 without output. A claim is a file under `$AVER_HOME/tmp/hook-claims/`, keyed by a hash of the hook name and its input, and expires 5 seconds after the first run finishes (at once when that run failed, so a refused `WorktreeRemove` runs again when retried), because Claude Code's hook input carries no per-event id and a later event can repeat the same bytes. Claims older than an hour are deleted. A duplicate `worktree-create` waits for the first and prints the same directory.

## Plain `claude` in the repository root

In a repository set up with Aver (an `aver.toml`, or Aver's hooks in the Claude settings), a session started in the checkout itself gets a short note at start: run `aver workspace new` before the first edit and make every change in the directory it prints, unless you ask for an edit of the checkout. This is an instruction to the agent, not enforcement. Nothing stops an agent from editing the checkout, and Aver does not see or verify such edits. For a guarantee, start sessions with `claude --worktree`, or spawn subagents with `isolation: "worktree"`.

## Worktree sessions and subagents

`aver integrate claude` installs `WorktreeCreate` and `WorktreeRemove`, so `claude --worktree` sessions and subagents started with `isolation: "worktree"` work in Aver workspace directories. Such an agent sees only its own prompt, so the integration and the plugin also install hooks that tell it how to finish and hold it to that:

| Event | Command | What the agent gets |
|---|---|---|
| `SessionStart` | `aver hook session-start` | `additionalContext` with the short brief: done means `aver verify` prints `DONE: PASS` (or `REVIEW`), `aver check` as you go, `aver verify` to finish, the branch commits land on, `aver giveup`. The full brief stays in `.aver/BRIEF.md`. |
| `Stop`, `SubagentStop` | `aver hook stop` | The verify gate. It runs no check: it syncs the directory and compares its snapshot with the heads of the workspace's newest receipts. When files changed and no `PASS` or `REVIEW` receipt is for exactly them, it blocks the stop once with a reason that says to run `aver check`, then `aver verify`. |

How the gate stays out of the way:

- An unchanged workspace never blocks, and neither does a session or subagent outside a workspace.
- It blocks each session or subagent at most once per version of the files and three times in all. A stop that follows a block (`stop_hook_active`) and a stop after `aver giveup "<reason>"` are always allowed.
- Any error lets the agent stop.

### Strict stop gate

A repository can make the gate hold until the work verifies:

```toml
[agents]
stop = "strict"
```

With `stop = "strict"` in `aver.toml`, every stop of a workspace with changed files is blocked until a `PASS` or `REVIEW` receipt covers exactly those files. It blocks again after a block, on the same files, and past three. `aver giveup "<reason>"` is the only other way out, and the block's reason says so. Aver reads the setting from the commit the workspace started from, so an agent cannot turn it off by editing its own `aver.toml`. Claude Code ends a run of consecutive Stop blocks after 8, so a strict gate holds an agent for at most 8 blocks in a row. The default, without the setting, is the once-per-version gate above.

A subagent's hooks may carry its parent's `cwd`, so the stop hook finds a subagent's workspace by its `agent_id`: Claude Code names the subagent's worktree `agent-<agent_id>`, and `aver hook worktree-create` makes it `$AVER_HOME/work/<repo>/wt-agent-<agent_id>`. A subagent without a worktree of its own is not gated; its parent's stop is.

Aver no longer isolates subagents itself or refuses their edits of the checkout. Ask for `isolation: "worktree"` when you spawn a subagent that writes, or run `aver workspace new` and work there. In an `[agents]` table in `aver.toml`, only `stop` is read; anything else is accepted and ignored.

To check the hooks against a real Claude Code by hand (it spends tokens), see `checks/worktree-verify-gate.sh` in the internal repository.

## Where each format comes from

Checked against each tool's documentation in October 2026. The same links are in `crates/aver/src/integrate/tools.rs`.

| Tool | Format | Source |
|---|---|---|
| Claude Code | Plugin manifest, `hooks/hooks.json`, skills, marketplace | [Plugins reference](https://code.claude.com/docs/en/plugins/manifest-reference.md), [marketplaces](https://code.claude.com/docs/en/plugins/marketplace-reference.md), [hooks](https://code.claude.com/docs/en/hooks.md), [skills](https://code.claude.com/docs/en/skills.md), [Agent Skills specification](https://agentskills.io/specification) |
| Cursor | Rules in `.cursor/rules/*.mdc` with `description`, `globs` and `alwaysApply` | [Rules](https://cursor.com/docs/context/rules) |
| Codex CLI | `AGENTS.md` | [AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md) |
| Gemini CLI | `GEMINI.md` | [GEMINI.md](https://geminicli.com/docs/cli/gemini-md/) |
| VS Code | `.github/copilot-instructions.md` | [custom instructions](https://code.visualstudio.com/docs/copilot/customization/custom-instructions) |
| AGENTS.md | Plain Markdown with no required structure | [agents.md](https://agents.md/) |

Tool-specific notes:

- **Cursor** pulls in the rule when it is relevant (`alwaysApply: false` with a description). You can also @-mention it.
- **Codex** reads `AGENTS.md` in any project.
- **Gemini CLI** reads `GEMINI.md` as context. To make Gemini read `AGENTS.md` as well, add `"context": {"fileName": ["AGENTS.md", "GEMINI.md"]}` to its settings.
- **VS Code** reads `.github/copilot-instructions.md` while `github.copilot.chat.codeGeneration.useInstructionFiles` is on.

## Tests

- `make plugin-check` (part of `make test`) runs `crates/aver/tests/plugin.rs`, `integrate_golden.rs` and `integrate_text.rs`, then `claude plugin validate integrations/claude-code` and `claude plugin validate .` when `claude` is on `PATH` (it prints a note and skips them otherwise). `plugin.rs` checks that the manifests parse with a name, version and description, that every skill and command has its frontmatter, that the plugin's `commands/` and `skills/` hold exactly the files `aver init` writes and that `aver init --yes` writes them byte for byte, that `hooks/hooks.json` is the one rendered from Aver's hook table (`AVER_UPDATE_GOLDEN=1 cargo test -p aver --test plugin` rewrites it), and that the plugin's hooks and the settings' hooks exit 0 with no `aver` installed, `SessionStart` printing the `/aver` message.
- `crates/aver/tests/integrate_golden.rs` compares each target's output against golden files in `crates/aver/tests/golden/integrate/`, for an empty repository and for one that already holds the tool's files. It then checks that a second run changes nothing, and that setting up over an older Aver's MCP entries takes them out and keeps everyone else's. After an intentional change, regenerate the files with `AVER_UPDATE_GOLDEN=1 cargo test -p aver --test integrate_golden` and review the diff.
- `crates/aver/tests/integrate_text.rs` checks every `aver` command and flag in the skill and the commands against `aver --help`. It also checks that the plugin's hooks, the removed names included, exit 0 and print nothing outside a workspace, and runs `scripts/aver.sh` under a GUI app's `PATH` with `aver` in `~/.local/bin`, and without any `aver`.
- `crates/aver/tests/integrate.rs` feeds Claude Code's hook input for each case to the real `aver hook` commands in a worktree workspace: the brief at session start, the note in a set-up repository's root, the gate on an unchanged directory, unverified changes, the stop after a block, a fresh `PASS`, a stale receipt, the block limit and `aver giveup`, a subagent's stop found by its `agent_id`, and the strict gate blocking five stops in a row until `aver giveup`. It runs the same hook twice in parallel with one input (from the settings and from the plugin): one `worktree-create` makes the workspace and both print its directory, one session brief, one stop block, and an expired or dead claim runs again. It also runs each removed hook name and checks it exits 0 silently. Unit tests in `crates/aver/src/run/hooks.rs` cover the gate's decisions, and those in `crates/aver/src/integrate/mod.rs` the merge that replaces old hooks and the move out of `.claude/settings.local.json`.
- `crates/aver/tests/integrate.rs` runs `aver integrate claude`, `aver integrate codex` and `aver setup` end to end in process: `aver setup` for every target and then `aver setup --remove` leave the repository byte for byte as it was, an old `.claude/settings.local.json` loses only Aver's hooks, and setting up over an old `.mcp.json` takes the `aver` server out.
- `integrate_golden.rs` also removes each target's writes, from the record (the seed comes back byte for byte) and without one (no trace of Aver is left and the other entries stay).
