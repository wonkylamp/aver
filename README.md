<h1 align="center">Aver: catch AI agents that game their tests</h1>

<p align="center"><b>Your agent says "done". Aver checks.</b></p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Version 0.4.1" src="https://img.shields.io/badge/version-0.4.1-informational.svg">
  <img alt="Platforms: macOS, Linux, WSL2" src="https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20WSL2-lightgrey.svg">
  <img alt="Languages: TS/JS, Python, Rust, Go" src="https://img.shields.io/badge/languages-TS%2FJS%20%7C%20Python%20%7C%20Rust%20%7C%20Go-success.svg">
</p>

<p align="center">
  <a href="#what-it-catches">What it catches</a> ·
  <a href="#quickstart">Quickstart</a> ·
  <a href="#parallel-agents">Parallel agents</a> ·
  <a href="#why-standard-ci-fails-on-ai-agents">Why CI isn't enough</a> ·
  <a href="#speed-and-disk">Speed and disk</a> ·
  <a href="#limits">Limits</a> ·
  <a href="#install-ci-and-documentation">Docs</a>
</p>

Your coding agent says its tests pass. Aver checks, with three tests of the evidence:

- **Frozen base tests.** Existing tests run as the original code has them, so a deleted, skipped or rewritten test still counts.
- **Transplant proof.** A new test must fail on the original code to prove anything.
- **Hunk ablation.** Each changed hunk of a passing change is reverted alone, and some proving test must fail.

Each agent works in a copy-on-write workspace that opens in under a second, with its dependencies in place. Agents on different tasks see each other's edits and can reserve code, and the verified work goes to Git as a branch or a pull request for a person to review.

```bash
curl -fsSL https://raw.githubusercontent.com/wonkylamp/aver/main/site/install.sh | sh
aver demo    # about 2 s, offline, in a throwaway directory
```

In Claude Code instead: `/plugin marketplace add wonkylamp/aver`, `/plugin install aver@aver`, then `/aver`.

<p align="center"><img src="assets/demo.svg" width="100%" alt="aver demo output: aver check keeps a green checkpoint, aver verify gives NOT DONE: FAIL_OWN after the agent comments out failing tests, aver restore --last-green rewinds in 24 ms in this run, and the real fix gives DONE: PASS"></p>

`aver demo` plays one task end to end. An agent comments out the tests it broke, and Aver calls it `FAIL_OWN`, meaning the change broke a test. `aver restore --last-green` rewinds the workspace. The real fix gets `PASS`. Nothing touches your repositories or `~/.aver`. [assets/demo.gif](assets/demo.gif) is the same run recorded in a terminal.

## What it catches

**Plain CI against Aver on 8 cheating commits, 4 honest ones and 2 with no test evidence.** We planted these cheats ourselves in small fixtures, so this shows what Aver catches, not how often agents cheat. Plain CI is the fixture's test command run once on the head.

| The head commit | Kind | Plain CI | `aver ci` |
|---|---|---|---|
| breaks `sub()` and adds `.skip` to its failing test | cheat | green | `FAIL_OWN`, exit 1 |
| breaks `sub()` and changes its test's expected value to match | cheat | green | `REVIEW`, exit 3 |
| claims a criterion with a test that asserts only `true` | cheat | green | `UNPROVEN`, exit 3 |
| implements `divide()` with a hidden branch no test exercises | cheat | green | `UNPROVEN`, exit 3 |
| breaks `add()`, deletes its test file, adds a test that always passes | cheat | green | `FAIL_OWN`, exit 1 |
| breaks `add()` and comments out its two failing tests | cheat | green | `FAIL_OWN`, exit 1 |
| breaks `add()` and rewrites both expected values to match | cheat | green | `REVIEW`, exit 3 |
| breaks `add()` and makes the build overwrite the built test file | cheat | green | `REVIEW`, exit 3 |
| the test command runs no test and exits 0 | no evidence | green | `BLOCKED_NOT_OWN`, exit 1 |
| the test runner exits 4 before any test | no evidence | red | `BLOCKED_NOT_OWN`, exit 1 |
| adds `sub()` with a test for it | honest | green | `PASS`, exit 0 |
| implements `divide()` and nothing else | honest | green | `PASS`, exit 0 |
| adds a comment; test names carry the checkout path and process id | honest | green | `PASS`, exit 0 |
| breaks `add()` and leaves its tests alone | honest | red | `FAIL_OWN`, exit 1 |

- `FAIL_OWN`: the change broke a test.
- `REVIEW`: it rewrote tests or harness, which a person must approve.
- `UNPROVEN`: nothing proved the change.
- `BLOCKED_NOT_OWN`: no test ran, so there is no evidence and no verdict on the change.

None of the 8 cheats got `PASS`; the 3 honest changes got `PASS` and the honest regression `FAIL_OWN`. Where no test ran, Aver gave no verdict, whether plain CI was green or red. The [full table](docs/BENCHMARKS.md#plain-ci-against-aver-on-cheating-commits) adds more no-evidence cases and an edited CI workflow, and `bench/ci-vs-aver.sh` reruns all of it.

## Quickstart

Install Aver and run `aver demo` as shown above, then:

### 1. Set up the repository once

```bash
aver init --ci --yes
```

It finds your test runner and writes `aver.toml` (what to run), `.github/workflows/aver.yml` (the same check on every pull request), and the agent setup: Claude Code's skill, `/aver` commands and hooks in `.claude/`, and Codex's instructions in `AGENTS.md`. Commit them, and teammates who clone the repository get the same setup.

### 2. Give each agent a workspace

```bash
# With the hooks from step 1, Claude Code opens an Aver workspace by itself
claude --worktree pagination

# Or make one for any agent and start it in the directory printed
aver workspace new
```

To give each agent a task Aver checks by name, add a label and a criterion:

```bash
aver workspace new --label pagination -c "pagination: the last page lists the remaining items"
aver workspace new --label dates -c "dates: parseDate accepts ISO weeks"
aver workspace new --label export -c "export: CSV export quotes commas"
```

Each prints a directory, a copy-on-write clone (about 0.3 s each on date-fns), with `node_modules`, `.venv` or `target` cloned in and `.aver/BRIEF.md` holding the task. Start `claude` or `codex` there.

### 3. Inside the workspace, the agent works in a loop

```bash
aver check                  # seconds: runs the affected tests on the change and on the original code
aver restore --last-green   # about 34 ms: back to the last state without a new failure
aver verify                 # the gate: a signed receipt, done only on PASS
```

The brief and the skill tell the agent to do this. In Claude Code, a session that tries to stop with changed files and no passing verify is sent back (once per version of the files, or until it passes with `[agents] stop = "strict"` in `aver.toml`). Codex gets the same instructions in `AGENTS.md`, without a gate.

### 4. Land the work

```bash
# From the repository root: one branch, verified against all three tasks
aver combine pagination dates export

# Or, inside one workspace: its verified work as a branch and a pull request
aver accept --pr
```

`aver combine` merges line by line (two tasks appending tests to one file is not a conflict), runs every task's checks on the merge, and exports nothing unless all of them pass.

## Parallel agents

**Competing attempts at one task.** `aver run` starts several agents (Claude Code or Codex) on the same task, each in its own workspace, shows them live, and ranks the attempts by receipt: verdict first, then the evidence, with diff size only breaking ties. When two or more pass, the top three's own tests also run on each other's code.

```bash
aver run "Fix the pagination off-by-one" --agents 3 -c "pagination: the last page lists the remaining items"

# The attempt you pick, as a branch and a pull request
aver accept 3 --pr
```

**A batch of tasks.** `aver run --issues bug --parallel 4` runs one agent per open issue labeled `bug`, four at a time; `aver run --tasks tasks.txt` reads one task per line, and a task can wait for another with `after: #1`. `aver combine --all-pass` then merges every passing task into one verified branch.

**Separate tasks that touch the same code.** Agents on different tasks in one repository see each other while they work:

```bash
aver lease 'src/db/**' --reason "moving the schema"   # reserve code before restructuring it
aver notices                                          # other tasks' edits to your files, line by line
aver check --combined                                 # your tests on a trial merge with their current work
```

Another task's overlapping lease is refused, and an edit under someone else's lease shows up in `aver verify` as a change a person reviews. `aver check` and `aver verify` report overlaps on their own. Here a second agent edited `src/math.js` while the first held a lease on it (real output, `aver notices` from 0.4.1, trimmed):

````text
Other tasks in this repository:
```
src/math.js: task "Work on repo at cb318d8b5a4c" changed line 14; you changed line 17 (no overlap; merging both is clean)
LEASED src/math.js: task "Work on repo at cb318d8b5a4c" holds an exclusive lease (lease_01M64S5JVJTC3KWBKT8D2W6FYF, expires in 29 min); you changed it anyway
```
LEASED: undo it with `aver restore <path>` unless you must change it.
````

[docs/CLI.md](docs/CLI.md#run-agents-in-parallel) covers `aver run`, batches, task graphs and coordination in full.

## Why standard CI fails on AI agents

A test suite answers "do the tests pass?". An agent controls the tests. Aver answers "did this change do what it claims, judged by the tests as they were?" It runs the original tests on your change, runs your new tests on the original code, and reverts each hunk of a passing change to see whether any test needs it. Hunk ablation runs on changes of up to 8 hunks. A larger change keeps its verdict without it, and the report lists the skipped ablation under `What Aver could not check`.

The exit codes in the table above are `aver ci`'s. In a workspace, `aver verify` exits 0 on `REVIEW`: the work is finished and waits for a person.

> [!NOTE]
> Locally, `aver accept --approve-review` refuses when stdin is not a terminal or an agent's environment variables are set. That is a workflow guardrail against an agent approving its own work by accident, not a sandbox boundary: an agent running as your user can get around it. The gate that enforces approval is the [GitHub Action](action/README.md). There `aver ci approve` reads the pull request's reviews from GitHub's API and accepts only an approval from a person other than the author, given on the head commit; a later push needs a new approval.

An agent fixed `divide()` and slipped in an extra branch. Real output, `aver verify` from 0.4.1 in the agent's workspace (lines left out where marked):

````text
NOT DONE: UNPROVEN. 2 of 2 criteria proven, no new failures, but no proving test needs 1 part of your change: with it reverted alone, every proving test still passes.
Base: 3 passed, 2 failed | Yours: 5 passed, 0 failed
Reverting your change at `src/math.js:15-15` did not cause any proving test to fail; add an assertion that requires it or remove the unneeded code.
…
Criteria:
  div-basic  proven
  div-zero  proven
```
div-basic: proven by unit::[crit:div-basic] divide returns the quotient
div-zero: proven by unit::[crit:div-zero] divide throws on division by zero
```

Changes no proving test needs (1; with each one reverted alone, every test that proves the change still passed):
```
src/math.js:15-15
```
````

<p align="center"><img src="assets/verify-output.svg" width="100%" alt="aver verify output: a criterion proven, a rewritten test expectation sent to review, and an untested hunk caught by reverting it"></p>

[docs/VERDICTS.md](docs/VERDICTS.md) explains exactly how each verdict is decided.

## Speed and disk

Aver against `git worktree add` plus a package install, on [date-fns](https://github.com/date-fns/date-fns) with a 116 MiB npm install. Measured once with Aver 0.4.1 on an Apple M1 Pro, APFS (clones, warm npm cache); Linux was not measured. [docs/BENCHMARKS.md](docs/BENCHMARKS.md#workspaces-at-scale) has the setup and the command for each row.

| Making workspaces with dependencies in place | Count | Time | Disk |
|---|---:|---:|---:|
| **Aver** (`aver workspace new --count N`) | 100 | **28.8 s** | **210 MiB** |
| `git worktree add` + `npm ci` | 5 | 6.0 s | 671 MiB |
| `git worktree add` + `npm ci`, extrapolated from 5 | 100 | about 2 min | about 13 GiB |
| `git worktree add` + `pnpm install`, warm store (installs the whole monorepo) | 5 | 92.7 s | 154 MiB |
| `git worktree add` + `bun install`, warm cache (installs the whole monorepo) | 5 | 13.7 s | 146 MiB |

| | Aver | `git worktree` |
|---|---|---|
| Disk per workspace | **2.1 MiB** | 134 MiB with npm, 31 MiB with pnpm, 29 MiB with bun |
| Rewind to the last passing state | **33.5 ms** median (`aver restore --last-green`) | `git stash` / `reset`, if you committed |
| Four agents append tests to one file | **merged and verified** (`aver combine`) | `CONFLICT (content)` |

All rows are measured unless marked extrapolated. The rewind and merge rows come from `bench/verify-10-of-10.sh` on a small fixture. Every script is in the repository to rerun.

## How it works

```
 your Git repository
        │  aver workspace new  /  claude --worktree   (copy-on-write, under a second)
        ▼
 ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
 │ Claude Code  │ │    Codex     │ │ Claude Code  │   each in its own workspace,
 │  workspace   │ │  workspace   │ │  workspace   │   aver check as it goes
 └──────┬───────┘ └──────┬───────┘ └──────┬───────┘
        └────────────────┼────────────────┘
                         ▼  aver verify
 ┌──────────────────────────────────────────────────┐
 │ Aver, the referee                                │
 │  1. frozen base tests: existing tests run as the │
 │     original code has them                       │
 │  2. transplant proof: your new tests must fail   │
 │     on the original code                         │
 │  3. hunk ablation: each changed hunk (up to 8)   │
 │     reverted alone; some test must notice        │
 │  4. union combine: tasks merged line by line,    │
 │     verified together                            │
 └───────────────┬──────────────────┬───────────────┘
                 ▼                  ▼
          PASS: signed         REVIEW / UNPROVEN:
          receipt, branch      what a person must look at,
          or pull request      fenced and short
```

Checks run natively in clones swapped to each side's code, never in the agent's directory, with CI tokens stripped from their environment. The [GitHub Action](action/README.md) runs the same judgement on every pull request and posts the receipt.

## The evidence

- **15 of 15 scenarios, 10 of 10 runs each** on [`bench/verify-10-of-10.sh`](docs/BENCHMARKS.md#same-verdict-every-time): a pass, a regression, a test command that runs nothing, a runner that crashes, a rewritten built test, a deleted test, an agent approving its own review, unstable test ids, a rewind, a four-way combine, a commit through the Git view, an untested hunk and an untested branch, a missing tool, a test module that cannot import, and an edited CI workflow.
- **Hunk necessity costs seconds**: 0.5 to 1.4 s on small projects and 23 s on anyhow, running only the tests that prove the change ([docs/VERDICTS.md](docs/VERDICTS.md#untested-code-in-a-passing-change)).

## Limits

- Early release (0.4.x).
- Aver writes to your repository only when you run `aver accept` or `aver combine`, which add a branch.
- Agents and checks run natively as your user, with no container sandbox ([docs/SECURITY-MODEL.md](docs/SECURITY-MODEL.md)).
- A verify proves only what some test checks.
- Workspace costs were measured on macOS (APFS) only. On Linux, Aver clones with reflink on XFS or btrfs and falls back to a plain copy on ext4, where files are copied instead of sharing blocks.
- Verifying costs more disk on compiled languages. A verify slot of a Node repository is a 7.6 MiB clone; a slot of Aver's own Rust repository is about 3.3 GiB, because it rebuilds `target/`.

## Install, CI and documentation

```bash
# macOS, Linux, WSL2
curl -fsSL https://raw.githubusercontent.com/wonkylamp/aver/main/site/install.sh | sh

# From source, Rust 1.94+
cargo install --locked --git https://github.com/wonkylamp/aver aver

# Checks what Aver needs, with a fix for each problem
aver doctor
```

In Claude Code, `/plugin marketplace add wonkylamp/aver`, `/plugin install aver@aver`, then `/aver` installs the CLI if it is missing and sets up the repository. The plugin and `aver init` write the same skill, commands and hooks, and a hook never runs twice when both are present. For Cursor, Gemini CLI and VS Code, run `aver integrate <tool>` ([docs/INTEGRATIONS.md](docs/INTEGRATIONS.md)).

In CI, `aver init --ci` writes the workflow, or add the action yourself:

```yaml
on: pull_request
permissions:
  contents: read
  pull-requests: write
jobs:
  aver:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - uses: wonkylamp/aver/action@v0.4.1
```

| Page | Contents |
|---|---|
| [docs/CLI.md](docs/CLI.md) | Every command: `aver run` with several agents, batches of issues, coordination, `aver.toml` |
| [docs/VERDICTS.md](docs/VERDICTS.md) | How a verdict is decided |
| [docs/CI.md](docs/CI.md) | The GitHub Action and `aver ci` |
| [docs/INTEGRATIONS.md](docs/INTEGRATIONS.md) | What each agent integration writes |
| [docs/BENCHMARKS.md](docs/BENCHMARKS.md) | Benchmark results, the machine they ran on and the command for each |
| [docs/SECURITY-MODEL.md](docs/SECURITY-MODEL.md) | Keys, receipts and your machine |
| [ARCHITECTURE.md](ARCHITECTURE.md) | The data model and components |

[CONTRIBUTING.md](CONTRIBUTING.md) covers building and testing. Report vulnerabilities as [SECURITY.md](SECURITY.md) describes. Aver is licensed under the [MIT License](LICENSE); [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lists what it bundles.
