# Benchmarks

This page reports every measured number behind Aver's claims: an A/B evaluation with real coding agents, a replay of real agent pull requests through `aver ci`, Aver verifying its own repository, and workspace, disk and cache benchmarks. It gives aggregates and method only. The last section names the command that reproduces each one.

All measurements ran on one Apple M1 Pro laptop (8 cores, 16 GiB RAM, APFS, macOS 27) with Docker Desktop, from 2026-10-10 to 2026-10-14. Several ran while other builds loaded the machine; the sections say which, and those times are upper bounds. Most numbers predate Aver 0.4, which deleted the Docker sandbox and the MCP server (REDESIGN.md §1.2); a section that measured the sandbox says so.

## Headline numbers

| Measure | With Aver | Baseline | Baseline is |
|---|---:|---:|---|
| Picked attempts that deleted a test or rewrote an assertion | **0 of 23** | 2 of 23 | the same agents in a plain `git worktree` |
| Picked attempts that edited an existing test file at all | 1 of 23 | 4 of 23 | the same |
| Picked attempts that passed every hidden acceptance test | 22 of 23 | 23 of 23 | the same |
| Agents' rating of their environment, 1 to 10 | **6.8** | 5.5 | the same (46 agents per arm) |
| Agent cost, all 18 scenario runs | about 19% more | baseline | the same |
| Mean time per agent | 124 s (+51%) | 82 s | the same |
| Real agent pull requests with per-test results on both sides | **27 of 32 (84%)** | 0 of 32 | Aver's first build, same pull requests |
| `FAIL_OWN` verdicts that are right by the rules | **2 of 2** | 1 of 13 | the rules before round 6.1 |
| Verify of a documentation-only change, Aver's own repository | **1 s** | 12 min 48 s | Aver before affected-check selection |
| Create a workspace directory, 2,053-file repository, p50 | **78 ms** | 585 ms | `git worktree add`, before any install |
| Ready for 50 agents, warm package cache | **7.3 s** | ≈ 312 s | `git worktree add` + `npm ci` per agent (extrapolated from N = 5) |
| Disk for 50 agents' dependencies | **319 MiB** | ≈ 13.5 GiB | the same (extrapolated from N = 5) |
| Second run of the same checks | 0.4 ms, no run started | 2 s | the first run |

Read these with the caveats in each section. In short: on our scenarios, Aver did not make agents more correct, because both arms' picks passed the hidden tests on 22 or 23 of 23 tasks. What it changed is what an accepted attempt does to the tests, how the agents rated the work, and how much setup N agents cost. It costs more agent time and somewhat more money.

## A/B evaluation: plain worktree against `aver run`

### Method

- **Arms.** The same Claude Code agents (`claude -p`) get the same task text. Arm A works in a plain `git worktree` with the project's test suite. Arm B works under `aver run`, with `aver check` and `aver verify`. Two agents per task per arm, each with a fixed spending cap and a 20- or 30-minute limit.
- **Picks.** Each arm picks one attempt per task by its own rule. Arm A takes the attempt whose `npm test` is green, else the fewest failures, then the smallest diff. Arm B takes Aver's rank 1.
- **Hidden tests.** Each scenario has acceptance tests that no agent sees. They score every attempt and the pick.
- **Scenarios.** Each scenario is a small repository with its own history, built for one failure mode. The **easy set** has six: a flaky neighbouring test, two tasks on the same functions, failures that predate the task, a refactor, a test that is tempting to skip, and a vague edge case. The **hard set** has six built so that a strong agent with plain tools fails some hidden tests:
  - a 312-test suite with 14 unrelated failures and four flaky tests;
  - a red CI whose easy fix is to edit the tests;
  - a format that only a document specifies;
  - a refactor whose duplicate helpers are not duplicates;
  - a regression that only a slower integration suite sees;
  - three tasks in one function, run in parallel.
- **Validation.** Before agents run, `node bench/eval/validate.ts` checks every scenario: the reference solution passes the hidden tests, a scripted naive solution fails them, declared flaky tests fail at their stated rate, and the base has an `aver.toml` contract that runs the test command with a per-test report and a `min_tests` floor of 90% of the base's tests (`node bench/eval/contracts.ts` writes it into the first history layer; Aver detects no tests outside CI, so arm B needs it). The contract is part of the base, so it is never in an attempt's diff.
- **Runs.** The easy set and the hard set ran on Claude Code's default model (Opus), and the hard set again on Sonnet: 18 scenario runs, 23 tasks, 92 agent sessions. Aver's build changed between runs (the round-11 build for the Sonnet run).
- **Not yet run.** two more sets measure what is still unproven. The scale set (`--set scale`, scenario `parcel-desk`) runs ten tasks from one planned release on one repository at once: five on disjoint files, three pairs that overlap on purpose, and one that depends on another's result (`after` in its task.json, which the harness does not act on yet: every task starts on the base). Its picks are merged across all of them, not just in sequence: every pair, then all ten with each conflicting pick skipped, and the tasks whose hidden tests still pass on that merge. The falsedone set (`--set falsedone`) holds four tasks that look finished to a naive agent: a form-decoding fix that breaks a documented public API, a report bug whose obvious fix is in the wrong layer, a rounding bug whose refund case the issue mentions in passing, and a stale-cache bug in a Python repository (tests with `unittest`). Each has a scripted naive solution whose tests are green and whose hidden tests fail; results.md reports each scenario's false-done rate, the attempts that claimed done (green tests in arm A, a PASS receipt in arm B) and failed hidden tests.

### Results

| | Easy set, Opus | Hard set, Opus | Hard set, Sonnet |
|---|---|---|---|
| Tasks (agents per arm) | 7 (14) | 8 (16) | 8 (16) |
| Picks passing every hidden test, A / B | 7 / 7 | 8 / 8 | 8 / **7** |
| Attempts flagged for test edits, A / B | 2 / 1 | 4 / 0 | 2 / 2 |
| Picks flagged for test edits, A / B | 1 / 0 | 2 / 0 | 1 / 1 |
| Picks that deleted a test or rewrote an assertion, A / B | 1 / 0 | 1 / 0 | 0 / 0 |
| Picks its arm called done that fail hidden tests, A / B | 0 / 0 | 0 / 0 | 0 / **1** |
| Wall time of the arm, A / B | 9.2 / 12.2 min | 10.9 / 16.8 min | 8.0 / 12.9 min |
| Mean time per agent, A / B | 84 / 113 s | 95 / 149 s | 67 / 108 s |
| Test runs per agent, A / B | 4.2 / 2.1 | 3.1 / 1.2 | 3.6 / 2.1 |
| Mean rating, A / B | 5.3 / 6.7 | 5.3 / **7.1** | 5.9 / 6.5 |

No pick in either arm introduced a new failure in the scenario's own suites (a test green on every run of the original code and red on every rerun of the pick).

### Test edits

The harness flags an attempt when it removes, skips or changes any line of an existing test file, or changes the test script. That is a coarse detector: it also flags a changed import line. Of the 11 flagged attempts:

- **Arm A, 8 attempts.** One deleted a test, two rewrote an assertion, one changed a test's input value, and four changed only an import line. Arm A's rule picked 4 of them: the deleted test, one rewritten assertion, and two import-line edits.
- **Arm B, 3 attempts.** Two changed import lines. One changed a test helper so that it reads CRLF line ends, which the scenario's format document requires. Aver picked the last one; it passed every hidden test.

Arm B cannot turn a verdict green by editing a test. `aver verify` judges existing tests with the original code's copies, and an edit that changes an expectation gives `REVIEW`, for a person, instead of `PASS` ([VERDICTS.md](VERDICTS.md)).

### Parallel tasks on one repository

Two scenarios run different tasks on the same code at the same time. Arm A merges its picks with `git merge`; arm B combines its runs with `aver combine`.

| Scenario | Arm A | Arm B |
|---|---|---|
| Two tasks, same functions (easy, Opus) | conflicts in 2 files | merged cleanly; each task's hidden tests pass on the merge (24 of 24), the interaction tests do not (2 of 6) |
| Three tasks, one function (hard, Opus) | conflicts in 3 files | conflict in 1 file; `aver combine` merged nothing and said why |
| Three tasks, one function (hard, Sonnet) | conflicts in 2 files | the same as Opus |

No agent in either arm used a coordination command (leases, notices) unprompted.

### Agent ratings

After each task, every agent answers the same questions about its environment and rates it from 1 to 10 against "I absolutely need this". Arm A rates the plain checkout and its tools, and arm B rates Aver. These are the agents' own answers, not a blind comparison.

- Arm B rated higher on 16 of 18 scenario runs, equal on 1 and lower on 1.
- No arm-B agent rated below 6, and 33 of 46 rated 7 or 8. In arm A, 20 of 46 rated 5 or lower.
- Arm A's most frequent requests: a type check in the test command (17 of 46), a record of which failures existed before the change (14), fewer permission prompts (12), flaky-test detection (10), and visibility into other tasks on the same code (10).
- Arm B's most frequent requests: an overlap report with the other task's actual changes and whether the two merge (16 of 46), stronger proofs (8), and the verdict repeated on the last line (7). Round 9 shipped all three, and the Sonnet run used that build.

### Where Aver did worse

- **Time.** Arm B was slower on all 18 scenario runs: +48% arm wall time and +51% mean time per agent. A verify then ran the affected checks in a Docker sandbox on both the change and the original code.
- **Cost.** Arm B cost about 19% more overall, and less on 9 of the 18 runs. Most of the difference is in the parallel-task scenarios, where agents coordinate and redo work. Without those scenarios, arm B cost slightly less than arm A on the Opus hard set and about 10% more on the Sonnet hard set.
- **One wrong pick (Sonnet, the 312-test suite).** Aver's pick passed 8 of 9 hidden tests with a `PASS` verdict. It missed one documented rule (payees that differ only in case and spacing still match), which no visible test or criterion covered. The other arm-B attempt passed all 9, and Aver ranked it second. Arm A's pick passed all 9. This is the study's only pick that its arm called done and that failed hidden tests. A verify proves only what some test checks.
- **Rating (Sonnet, the red-CI scenario).** Arm B's agents rated it 6.0 against arm A's 6.5.
- **Merges.** On the three-task scenario neither arm produced a merge. Arm B's picks conflicted in one file instead of two or three, and `aver combine` refused to export; nothing resolved the conflict. On the two-task scenario, arm B's clean merge failed 4 of the 6 interaction tests.

## Replay: real agent pull requests through `aver ci`

**Method.** 59 merged and unmerged pull requests written by six different coding agents, on public npm projects that have a `test` script, from a public research dataset of agent-authored pull requests. Each ran through `aver ci` (base against head, in network-less sandboxes) one at a time, with a fresh Aver home and a 30-minute limit. The rerun at the current build has reached 32 of the 59. It waits for a quiet machine before each pull request (1-minute load average below 20). The table compares the same 32.

| | First build | Current build |
|---|---:|---:|
| Receipts | 24 of 32 (75%) | **32 of 32 (100%)** |
| Receipts with per-test results on both sides | 0 of 32 | **27 of 32 (84%)** |
| Median wall time per pull request | 84 s | 90 s |

- **Without per-test results (5).** 3 use a custom test script that Aver cannot add a reporter to, and 2 need the network, which the sandbox did not have. None is a defect in Aver.
- **`FAIL_OWN` precision.** Under the round-6 rules, 1 of 13 `FAIL_OWN` verdicts was a true catch. Most of the rest came from pull requests that changed tests on purpose. Under the current rules (round 6.1), there are 2 `FAIL_OWN` verdicts, and both are right by the rules. One imports a package that no manifest declares. The other adds test suites that fail to load under the project's own harness, and hides them by editing the test configuration. A reader who counts those suites as environmental would score it 1 of 2.
- **Intended test changes.** The 8 pull requests that changed test expectations or the harness on purpose now get `REVIEW`, with each changed test's diff in `summary.md`.

## Aver on its own repository

At the time Aver verified its own changes in seven checks: the Rust workspace, the MCP server, the site, the evaluation harness and the end-to-end typecheck. Since Aver 0.4 it has four, all native. Round 11 made a verify run only the checks a change affects. Measured with `aver ci` and a new home each time, at a load average of 2 to 6:

| Change | Before | Now |
|---|---:|---:|
| One line of documentation, `aver ci` | 12 min 48 s | **1 s**, no check affected |
| The same in a workspace directory, `aver verify` / `aver check` | | 24 ms / 0.1 s |
| A comment in the MCP server's TypeScript, `aver ci` | | 16 s, Node checks only |
| A comment in a Rust crate, `aver ci` | 7.0 to 7.3 min | 6 min 15 s |
| The same, original code's runs already cached | | 5 min 39 s |
| The same, `aver check` natively | could not predict | 3 min 11 s |

In the sandbox a Rust change cost about six minutes per verify, because it built every dependency from scratch for each side. Native verifies (round 14) start from the base's build; [bench/native/results.md](../bench/native/results.md) has their times.

## Workspace directories against `git worktree add`

From `bench/workspace-disk.sh`, on the default `clonefile` strategy. Other builds ran on the machine at a load average of 38 to 58, so every time is an upper bound.

| Measure | 2,053-file TypeScript library | Synthetic, 16,004 files and a 20,000-file `node_modules` |
|---|---:|---:|
| Warm Aver directory, p50 / p95 | 78 / 129 ms | 386 / 428 ms |
| `git worktree add`, p50, before any install | 585 ms | 2,781 ms |
| Disk per Aver directory | 4.4 MB | 7.3 MB |
| Disk per worktree, before any install | 16.1 MB | 79.8 MB |
| Sync of an unchanged directory, p50 / p95 | 40 / 55 ms | 62 / 141 ms |

An Aver directory includes its `node_modules`, as links into one shared store; the worktree numbers exclude `npm install`. The design target of under 1% of a worktree's disk was not met: APFS writes about 1.5 to 2 KB of metadata per cloned file. With the `copy` strategy (filesystems without clones), a warm directory takes about as long as a worktree (369 ms against 416 ms on the library).

## Parallel agent environments

From `make bench-parallel`, which Aver 0.4 deleted with the sandbox: N agents on a 363-file open-source React, Vite and TypeScript app with 327 npm packages. The status quo is `git worktree add` plus `npm ci` per agent. Aver imports the commit once, builds one shared dependency layer (cold) or finds it built (warm), and opens N workspaces. The load average was 40 to 87.

| Agents | Status quo, cold | Status quo, warm | Status quo disk | Aver, cold | Aver, warm | Aver disk |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 9.1 s | 3.8 s | 277 MiB | 38.6 s | 7.3 s | 319 MiB |
| 5 | 48.5 s | 31.2 s | 1.35 GiB | 38.6 s | 7.3 s | 319 MiB |
| 10 | ≈ 97 s | ≈ 63 s | ≈ 2.7 GiB | 38.6 s | 7.3 s | 319 MiB |
| 50 | ≈ 486 s | ≈ 313 s | ≈ 13.5 GiB | 38.6 s | 7.3 s | 319 MiB |

Status-quo values marked ≈ are extrapolated from the measured N = 5, because the disk had too little free space to run them. For a single agent Aver was slower, because building the layer and starting the first sandbox cost more than one `npm ci`. The sandbox also had a cost per check: four sandboxed type checks of the app ran 80 to 87 s each, against 3.1 s natively. The causes are Docker Desktop's VM, a 2-CPU limit per sandbox, and streaming the tree into each container.

## Import, workspace API and cache

From `make bench`, on the same 2,053-file library (109,543 lines of TypeScript):

- **Import.** 168 ms into a 3.9 MiB database. A re-import takes 98 ms and writes nothing. The export round trip takes 404 ms and reproduces the original tree SHA.
- **Workspaces.** 1,000 workspaces open in 33 µs each in process (p99 82 µs) and in 285 µs over signed HTTP (p99 511 µs). Each costs 258 bytes of storage, or 713 bytes with the signed-request audit row.
- **Cache.** A second run of the same checks on the same snapshot starts no run and takes 0.4 ms, against 2 s for the first run.

## Same verdict every time

`bench/verify-10-of-10.sh` verifies seven base and head pairs on scratch copies of `fixtures/sh-calc` (shell checks only) ten times each with `aver ci --fresh`, and passes only when every run gives the expected verdict and exit code. The pairs are a passing change, a regression, and the benchmarks of REDESIGN.md §6 that a shell check can show: a test command that runs no test (B1), a runner that exits 4 before any test (B2), a build that rewrites the built test file (B3), a broken function with its test file deleted (B4), and test names built from the run's directory and process id (B8). B6 and B7 need Node and Cargo; `crates/aver/tests/verdict_integrity.rs` gates them. B12 is not in the script: it tests `aver run` (the cross-check's ranking of two attempts and the brief an issue writes), not a verdict on one base and head pair, and neither result depends on run timing, so repeating it ten times measures nothing the test does not. `verdict_integrity.rs` gates it with scripted agents (`b12a_cross_check_renames_a_shared_new_test_file`, `b12b_an_issue_cannot_close_the_repo_data_fence`).

On 2026-10-17 on an M-series laptop, every scenario gave its expected verdict 10 of 10 times (`bench/results/verify-10-of-10.md`); the run took a minute.

## Limits of this evidence

- The A/B evaluation is small: two agents per task, 23 tasks, one run each. Every scenario is synthetic, and we built them.
- Correctness did not separate the arms on these scenarios.
- The agent ratings are self-reported, and each agent saw only its own arm.
- The replay rerun covers 32 of 59 pull requests, all on npm projects.
- Every timing comes from one laptop, often under load.

## How to reproduce

| What | Command | Needs |
|---|---|---|
| Import, workspace API, cache | `make bench` | Node |
| Workspace directories against `git worktree add` | `bench/workspace-disk.sh` | APFS for the `clonefile` numbers |
| A/B evaluation, easy set | `make eval` | A logged-in `claude`; spends tokens |
| A/B evaluation, hard set | `make eval ARGS="--set hard --timeout 30m --max-budget-usd 8 --label <name>"`; add `--model sonnet` for the Sonnet run | the same |
| The evaluation harness only, with scripted agents | `make test-eval` | nothing |
| Check the scenarios | `node bench/eval/validate.ts [scenario]` | nothing |
| Replay of agent pull requests | `node bench/replay/replay.ts --manifest <file> --max-load 20`, then `node bench/replay/compare.ts` | Docker, network for the fetch |
| Same verdict every time | `bench/verify-10-of-10.sh` (`N=3`, `SCENARIOS="b1 b4"`) | a release build, Git |
| Verify a change to this repository | `cargo run -p aver -- ci --repo . --base <rev> --head <rev>` | Rust and Node |

`make eval` runs one scenario at a time and resumes where it stopped. `node bench/eval/run.ts --report` regenerates the summary from the stored results. Run the evaluation from your own terminal, not from a sandboxed shell.
