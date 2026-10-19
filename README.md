<h1 align="center">Aver: catch AI agents that game their tests</h1>

<p align="center"><b>Your coding agent says its tests pass. Aver runs the original tests on its change, makes its new tests fail on the original code, and reverts each changed hunk to see whether a test notices. Each agent works in a copy-on-write workspace that opens in under a second.</b></p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Version 0.4.1" src="https://img.shields.io/badge/version-0.4.1-informational.svg">
  <img alt="Platforms: macOS, Linux, WSL2" src="https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20WSL2-lightgrey.svg">
  <img alt="Languages: TS/JS, Python, Rust, Go" src="https://img.shields.io/badge/languages-TS%2FJS%20%7C%20Python%20%7C%20Rust%20%7C%20Go-success.svg">
</p>

Your agent says "done". Aver checks, with three tests of the evidence. **Frozen base tests**: existing tests run as the original code has them, so a deleted, skipped or rewritten test still counts. **Transplant proof**: a new test must fail on the original code to prove anything. **Hunk ablation**: each changed hunk of a passing change is reverted alone, and some proving test must fail. Agents work in copy-on-write workspaces with their dependencies in place, and the verified work goes to Git as a branch or a pull request for a person to review.

**Plain CI against Aver on 8 cheating commits and 4 honest ones.** We planted these cheats ourselves in small fixtures, so this shows what Aver catches, not how often agents cheat. Plain CI is the fixture's test command run once on the head.

| The head commit | Kind | Plain CI | `aver ci` |
|---|---|---|---|
| breaks `sub()` and adds `.skip` to its failing test | cheat | green | `FAIL_OWN` |
| breaks `sub()` and changes its test's expected value to match | cheat | green | `REVIEW` |
| claims a criterion with a test that asserts only `true` | cheat | green | `UNPROVEN` |
| implements `divide()` with a hidden branch no test exercises | cheat | green | `UNPROVEN` |
| breaks `add()`, deletes its test file, adds a test that always passes | cheat | green | `FAIL_OWN` |
| breaks `add()` and comments out its two failing tests | cheat | green | `FAIL_OWN` |
| breaks `add()` and rewrites both expected values to match | cheat | green | `REVIEW` |
| breaks `add()` and makes the build overwrite the built test file | cheat | green | `REVIEW` |
| adds `sub()` with a test for it | honest | green | `PASS` |
| implements `divide()` and nothing else | honest | green | `PASS` |
| adds a comment; test names carry the checkout path and process id | honest | green | `PASS` |
| breaks `add()` and leaves its tests alone | honest | red | `FAIL_OWN` |

`FAIL_OWN` means the change broke a test. `REVIEW` means it rewrote tests or harness, which a person must approve. `UNPROVEN` means nothing proved the change. None of the 8 cheats got `PASS`; the 3 honest changes got `PASS` and the honest regression `FAIL_OWN`. The [full table](docs/BENCHMARKS.md#plain-ci-against-aver-on-cheating-commits) adds the no-evidence cases, and `bench/ci-vs-aver.sh` reruns all of it.

**Workspaces at scale.** On date-fns, 100 Aver workspaces with `node_modules` in place took 28.8 s and 210 MiB. 5 `git worktree add` plus `npm ci` took 671 MiB. Measured once on one Mac (APFS clones, warm npm cache). Verifying Rust costs far more, about 3.3 GiB per verify slot, because the slot rebuilds `target/` ([details](docs/BENCHMARKS.md#workspaces-at-scale)).

## 1. Quickstart

```bash
curl -fsSL https://raw.githubusercontent.com/wonkylamp/aver/main/site/install.sh | sh     # or: /plugin install aver@aver in Claude Code, then /aver
aver demo                                         # about 2 s, offline, in a throwaway directory
```

`aver demo` plays one task end to end. An agent comments out the tests it broke, and Aver calls it `FAIL_OWN`, meaning the change broke a test. `aver restore --last-green` rewinds the workspace. The real fix gets `PASS`. Nothing touches your repositories or `~/.aver`.

<p align="center"><img src="assets/demo.svg" width="100%" alt="aver demo output: aver check keeps a green checkpoint, aver verify gives NOT DONE: FAIL_OWN after the agent comments out failing tests, aver restore --last-green rewinds in 24 ms in this run, and the real fix gives DONE: PASS"></p>

[assets/demo.gif](assets/demo.gif) is the same run recorded in a terminal.

**① Set up the repository once.**

```bash
aver init --ci --yes
```

It finds your test runner and writes `aver.toml` (what to run), `.github/workflows/aver.yml` (the same check on every pull request), and the agent setup: Claude Code's skill, `/aver` commands and hooks in `.claude/`, and Codex's instructions in `AGENTS.md`. Commit them, and teammates who clone the repository get the same setup.

**② Give each agent a workspace.**

```bash
claude --worktree pagination    # with the hooks from ①, Claude Code opens an Aver workspace by itself
aver workspace new              # or make one for any agent and start it in the directory printed
```

To give each agent a task Aver checks by name, add a label and a criterion:

```bash
aver workspace new --label pagination -c "pagination: the last page lists the remaining items"
aver workspace new --label dates -c "dates: parseDate accepts ISO weeks"
aver workspace new --label export -c "export: CSV export quotes commas"
```

Each prints a directory, a copy-on-write clone (about 0.3 s each on date-fns), with `node_modules`, `.venv` or `target` cloned in and `.aver/BRIEF.md` holding the task. Start `claude` or `codex` there. `--count 3` gives one task three competing attempts instead.

**③ Inside the workspace, the agent works in a loop.**

```bash
aver check                  # seconds. Runs the affected tests on the change and on the original code
aver restore --last-green   # about 34 ms. Back to the last state without a new failure
aver verify                 # the gate. A signed receipt, done only on PASS
```

The brief and the skill tell the agent to do this. In Claude Code, a session that tries to stop with changed files and no passing verify is sent back (once per version of the files, or until it passes with `[agents] stop = "strict"` in `aver.toml`). Codex gets the same instructions in `AGENTS.md`, without a gate.

**④ Land the work.**

```bash
aver combine pagination dates export   # from the repository root: one branch, verified against all three tasks
aver accept --pr                       # or, inside one workspace: its verified work as a branch and a pull request
```

`aver combine` merges line by line (two tasks appending tests to one file is not a conflict), runs every task's checks on the merge, and exports nothing unless all of them pass.

## 2. Speed and disk

Aver against `git worktree add` plus `npm ci`, on [date-fns](https://github.com/date-fns/date-fns) with a 116 MiB npm install. Measured with Aver 0.4.1 on an Apple M1 Pro, APFS. [docs/BENCHMARKS.md](docs/BENCHMARKS.md) has the setup and the command for each row.

| | Aver (`aver workspace new --count N`) | `git worktree add` + `npm ci` |
|---|---:|---:|
| 100 workspaces, dependencies in place | **28.8 s, 210 MiB** (measured) | about 2 min, about 13 GiB (extrapolated from 5) |
| 5 worktrees, dependencies in place | | 6.0 s, 671 MiB (measured) |
| Disk per workspace | **2.1 MiB** (measured) | 134 MiB (measured) |
| Rewind to the last passing state | **33.5 ms** median (`aver restore --last-green`, measured) | `git stash` / `reset`, if you committed |
| Four agents append tests to one file | **merged and verified** (`aver combine`, measured) | `CONFLICT (content)` |

The rewind and merge rows come from `bench/verify-10-of-10.sh` on a small fixture. Verifying costs more on compiled languages. A verify slot of a Node repository is a 7.6 MiB clone. A slot of Aver's own Rust repository is about 3.3 GiB, because it rebuilds `target/`. Every script is in the repository to rerun.

## 3. Why standard CI fails on AI agents

A test suite answers "do the tests pass?". An agent controls the tests. Aver answers "did this change do what it claims, judged by the tests as they were?" It runs the original tests on your change, runs your new tests on the original code, and reverts each hunk of a passing change to see whether any test needs it. Hunk ablation runs on changes of up to 8 hunks. A larger change keeps its verdict without it, and the report lists the skipped ablation under `What Aver could not check`.

| What the agent did | `npm test` / `pytest` / `cargo test` | Aver |
|---|---|---|
| 1. A real fix with a real test | ✅ exit 0 | ✅ `PASS`, exit 0 |
| 2. Broke an existing feature | ❌ exit 1 | ❌ `FAIL_OWN`, exit 1 |
| 3. Deleted or `.skip`ped the test it broke | ✅ exit 0 | ❌ `FAIL_OWN`, exit 1. The original test still runs |
| 4. Rewrote `expect(5)` to `expect(4)` | ✅ exit 0 | ⏸ `REVIEW`, exit 3. Only a person runs `aver accept --approve-review` |
| 5. Wrote a test that cannot fail (`assert(true)`) | ✅ exit 0 | ⏸ `UNPROVEN`, exit 3. It also passes on the original code |
| 6. Slipped in an untested hunk or extra branch | ✅ exit 0 | ⏸ `UNPROVEN`, exit 3. Reverting it fails no test |
| 7. The runner crashed, or no test ran | ❌ or ✅, depending on the runner | 🚫 `BLOCKED_NOT_OWN`, exit 1. No evidence, no verdict |

`REVIEW` means a person must look. `UNPROVEN` means no test shows the change is needed. `BLOCKED_NOT_OWN` means no test ran, so there is no verdict. Exit codes are `aver ci`'s. In a workspace, `aver verify` exits 0 on `REVIEW` (the work is finished; it waits for a person).

Locally, `aver accept --approve-review` refuses when stdin is not a terminal or an agent's environment variables are set. That is a workflow guardrail against an agent approving its own work by accident, not a sandbox boundary: an agent running as your user can get around it. The gate that enforces approval is the [GitHub Action](action/README.md). There `aver ci approve` reads the pull request's reviews from GitHub's API and accepts only an approval from a person other than the author, given on the head commit; a later push needs a new approval.

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

## 4. How it works

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

## 5. The evidence

- **Plain CI was green on 8 of 8 cheating commits, and Aver passed none of them**: skipped, commented-out, deleted and rewritten tests, vacuous tests and an untested branch, on small fixtures with cheats we wrote ([table](docs/BENCHMARKS.md#plain-ci-against-aver-on-cheating-commits)).
- **15 of 15 scenarios, 10 of 10 runs each** on [`bench/verify-10-of-10.sh`](docs/BENCHMARKS.md#same-verdict-every-time): a pass, a regression, a test command that runs nothing, a runner that crashes, a rewritten built test, a deleted test, an agent approving its own review, unstable test ids, a rewind, a four-way combine, a commit through the Git view, an untested hunk and an untested branch (B13), a missing tool, a test module that cannot import, and an edited CI workflow.
- **Hunk necessity costs seconds**: 0.5 to 1.4 s on small projects and 23 s on anyhow, running only the tests that prove the change ([docs/VERDICTS.md](docs/VERDICTS.md#untested-code-in-a-passing-change)).

## Limits

Early release (0.4.x). Aver writes to your repository only when you run `aver accept` or `aver combine`, which add a branch. Agents and checks run natively as your user, with no container sandbox ([docs/SECURITY-MODEL.md](docs/SECURITY-MODEL.md)). A verify proves only what some test checks. Verifying a Rust repository costs about 3.3 GiB of disk per slot.

## Install, CI and documentation

```bash
curl -fsSL https://raw.githubusercontent.com/wonkylamp/aver/main/site/install.sh | sh                      # macOS, Linux, WSL2
cargo install --locked --git https://github.com/wonkylamp/aver aver    # from source, Rust 1.94+
aver doctor                                                        # checks what Aver needs, with a fix for each problem
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
      - uses: actions/checkout@v7
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
