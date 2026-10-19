# The `aver` command line

This page covers installing Aver and using it from your terminal: `aver run` for parallel agents, batches of tasks and GitHub issues, coordination between tasks, Claude Code integration, workspace directories for any agent CLI, and `aver.toml`. The short version is in the [README](../README.md). [TASKS.md](TASKS.md) covers creating tasks by hand, and [CI.md](CI.md) covers `aver ci` (with `--check <id>` for one check per matrix job), `aver ci approve` (a pull request review's approval bound to the head commit, which clears a `REVIEW` in CI) and the GitHub Action. Run from your terminal, `aver ci` keeps the base's runs in `$AVER_HOME/records`, so a second run against the same base runs only the head side; `--fresh` reads no record, `--no-records` keeps none, and inside CI (`CI`, `GITHUB_ACTIONS` and the like set) none is kept ([CI.md](CI.md)).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/wonkylamp/aver/main/site/install.sh | sh
```

[`site/install.sh`](../site/install.sh) detects the platform (Linux x86_64 or aarch64, macOS arm64 or x86_64), downloads the matching `aver-<os>-<arch>` binary from the latest GitHub release, checks it against the release's `SHA256SUMS`, and installs it to `~/.local/bin` without `sudo`. On native Windows (MINGW, MSYS, Cygwin) it stops and points to [WINDOWS.md](WINDOWS.md); after installing it warns when `git` is missing, and prints the next steps. Set `AVER_VERSION=v0.4.1` to pin a release and `AVER_INSTALL_DIR` to install elsewhere.

With Homebrew, once the `wonkylamp/homebrew-tap` repository exists (its formula is generated from [`packaging/homebrew/aver.rb`](../packaging/homebrew/aver.rb) by `packaging/homebrew/update-formula.sh <version>`):

```bash
brew install wonkylamp/tap/aver
```

From source, with Rust 1.94 or later:

```bash
cargo install --locked --git https://github.com/wonkylamp/aver aver
```

The binary is the whole of Aver: every command runs in its own process, with no daemon. Aver runs checks natively with the project's own toolchains, so they must be installed as for running its tests. `make up` needs a clone of this repository, described in [TASKS.md](TASKS.md).

### First run

```bash
aver demo      # about 30 s, offline: watch Aver catch an agent that games its tests
aver init      # in your repository: draft aver.toml and connect the agent tools
aver doctor    # check everything Aver needs, with a fix for each problem
aver           # where things stand here and the one command to run next
```

`aver demo` makes a throwaway directory with its own Aver home and a tiny Git repository with shell tests (no Node, no network), and runs the real engine there: an agent edits the wrong function and comments out the tests that fail (`aver verify`: `FAIL_OWN`), `aver restore --last-green` rewinds it and prints how long that took, and the real fix with its proving test is a `PASS`. The directory is removed when the demo ends, also on an error or Ctrl-C; your `$AVER_HOME` and repository are never touched. It exits 0 when every step gave the verdict it expects.

`aver` with no command reads and prints the next step, writing nothing and starting no run, and exits 0: outside a Git repository, or in one without home keys or an `aver.toml`, `aver init` (and `aver demo`); in a set-up repository, `aver workspace new .`; inside a workspace directory, compared with what its latest verify and check judged by a scan that records nothing: changes neither covers, `aver check`; a check that predicts `PASS`, `aver verify`; a verify's `PASS` on the files there, `aver accept`; a `REVIEW`, that a person approves it with `aver accept --approve-review`; any other verdict, the fix and `aver check`, with `aver restore --last-green` when the check found a NEW failure. `aver --help` lists the everyday commands (`check`, `verify`, `restore`, `accept`, `init`, `demo`) first and the rest under `Advanced:`.

`aver init` is described in [Drafting aver.toml](#drafting-avertoml-aver-init). `aver setup` connects the agent tools alone:

`aver setup` looks for `claude`, `cursor`, `codex`, `gemini` and `code` on `PATH` and where installers put them (and for the Claude, Cursor and VS Code apps on macOS), sets up those this repository or you evidently use (Claude Code when it is installed or `.claude/` exists, Cursor with `.cursor/`, Codex when it is installed or with `.codex/`, Gemini with `.gemini/` or `GEMINI.md`, VS Code and Copilot only when `.github/copilot-instructions.md` already exists; each other tool found is asked about on a terminal, and otherwise skipped with the `--tool` that sets it up), lists the files the matching `aver integrate` targets would write, one line each (`--verbose` prints each in full), asks once and writes them, then names the next command. Answering no, also through a pipe (`echo n | aver setup`), writes nothing and exits 0; with no answer at all (stdin closed) it asks for `--yes` and exits 2. `--tool <name>` (repeatable: `claude`, `cursor`, `codex`, `gemini`, `vscode` or its alias `copilot`, `agents-md`) picks the tools instead; with none found it writes `AGENTS.md` only. `--yes` skips the question. Setting up again removes the hooks and the MCP server entries an older Aver wrote. `aver setup --remove` undoes everything `aver setup` and `aver integrate` wrote in the repository ([INTEGRATIONS.md](INTEGRATIONS.md#how-aver-integrate-behaves)).

`aver doctor` prints one line for the checks that passed, the repository line (the ecosystems found and the checks they get, such as `python -m pytest` or `cargo test --workspace`), each problem with its fix, and the next command; `--verbose` prints every check. It checks the system (macOS or Linux; WSL 2 on Windows), `git`, free disk under the Aver home, the build artifacts in the home's workspace directories (a warning when they pass 20 GiB, or 1 GiB with less than 10 GiB free; fix: `aver gc --builds`), the slots under `$AVER_HOME/sides` with the largest four (a warning when they pass 30 GiB, or 1 GiB with less than 10 GiB free; fix: `aver gc`, or `aver gc --builds`), by what removing each alone frees (on APFS a copy-on-write clone does not count the blocks it shares with its source or with another slot, such as a transplant slot cloned from its base slot, so the total can be less than removing them all frees; the build artifacts are apparent sizes), the agent CLIs and whether Claude Code is signed in (`claude auth status`, which prints no credential; signed out, it says to run `claude auth login`), whether apps started from the Dock on macOS find `aver`, whether the current repository is one Aver can check and whether `aver check` finds its tools here (`npm`, `cargo`, `go`, and `python` or `python3` with pytest when the project uses it), whether the `go` and `rustc` here meet the root `go.mod`'s `go` line and `Cargo.toml`'s `rust-version` (a required check: natively, an older toolchain runs no test, and every verify is `BLOCKED_NOT_OWN`; a newer `toolchain` line is a warning; Go is held to these lines only when `go env GOTOOLCHAIN` is `local`, since under `auto`, its default, Go fetches the version `go.mod` asks for and so do Aver's runs, which inherit it), and the last unclean stop: when an Aver process was killed (or the machine lost power), the next command that opens the home drops the workspaces' stat caches, kills check runs the dead process left running, and records what it did, which `aver status` mentions for a day and doctor shows with any runs it interrupted. It changes nothing. It exits 0 when everything required (system, Git, disk) is there and 1 otherwise; the rest are warnings.

## Run agents in parallel

In any Git repository (Node, Python, Rust and Go projects verify today), with the project's toolchain and Claude Code (`claude`) or Codex (`codex`) installed:

```bash
aver run "Add negate(x) to src/math.js" --agents 3 -c "neg: negate(x) returns -x"
```

Aver imports `HEAD`, gives each agent its own copy-on-write directory with the repository's installed dependencies cloned in and a working `git status`, and shows them live:

```
 aver run · 3 agents · HEAD · 2:41                                       ctrl-c stop
  #  agent   state       last action                     files      verify    crit
  1  claude  ⠇verifying  Bash aver verify                1 +2-0     UNPROVEN  0/1
  2  claude  ⠏working    Edit test/negate.test.js        2 +6-0     -         0/1
  3  claude   done       done: PASS 1/1                  2 +9-1     PASS      1/1
```

Each agent is told what done means: `aver verify` prints `DONE: PASS`, which needs a test tagged `[crit:neg]` that fails without the change. While it works, `aver check` tells it, as often as it likes, what `aver verify` will say: it is a dry run of the verify itself, ending with "Verify should PASS" or what will stop it, and the verify that follows reuses its runs. `aver verify` is the signed gate, limited by `--max-verifies` (default 10): verify 11 of 10 runs nothing and exits 2. Both run the checks natively on your machine, in slots cloned from your directory (see "Native runs" below). When an agent stops before `PASS`, Aver hands it the report and lets it continue (Claude through its Stop hook; other agents are resumed, until one exits twice in a row without changing a file, which ends it `idle`). Then the attempts are ranked by receipt:

```
Ranked by receipt (run 4039Y6):
  #  agent   verdict          crit  proofs  new  preexisting  diff          time     verifies  cost
  3  claude  PASS             1/1   2       0    0            2f +9 -1      4m12s    2         $0.61
  2  claude  PASS             1/1   1       0    0            2f +6 -0      5m40s    3         $0.88
     fails 1 of #3's test:
       ```
       [crit:neg] negates zero to zero
       ```
  1  claude  UNPROVEN         0/1   0       0    0            1f +2 -0      6m02s    8 budget  $1.10
     neg: passes_without_change
  Order: verdict, then fewer changed tests, unproven criteria and others' tests failed, then more proofs; diff size only breaks ties.
  aver accept 3 [--pr]   aver diff 3 2 [--tool <cmd>]   aver discard
```

When two or more attempts pass (or need review, or are unproven), the run ends with a cross-check: each of the top three's own tests runs on the others' code as a dry run (nothing stored, no verify spent, at most 5 minutes), and a test that fails there counts against that code. A test that cannot load on the other's code tests a different API and does not count. `--cross off` skips it; the result is in `$AVER_HOME/runs/<run>/cross.json`. When the top two are still equal on evidence, the table ends with "Tie on evidence between #a and #b, decided by diff size: review with `aver diff a b`".

`aver accept 3` turns the winner into a normal branch, `aver/add-negate-x-to-src-math-js-7b3g` (the task's slug and the last four characters of the run id, so two batches' branches never collide; `--branch` picks another), whose parent is the commit the agents started from; your working tree and current branch are not touched. The commit is yours: it is authored with the repository's `git config user.name` and `user.email` (Aver refuses without them). Its title is `--title` when given, else the subject of the agent's latest commit in its Git view (one line, control characters escaped, at most 100 characters), else the task, never a made-up `Work on <repo> at <sha>`; its body is the receipt's evidence as `summary.md` renders it (the counts line, what the verdict covers, test changes, the tests that need review, each criterion's proof), and it names the attempt, agent and receipt in `Aver-Attempt:`, `Aver-Agent:` and `Aver-Receipt:` trailers. A `REVIEW` winner (it changes what an existing test expects) is refused with exit 2 until a human reads it and runs `aver accept 3 --approve-review`; the commit then carries `Aver-Approved-Review: <receipt id>`. The approval holds for that receipt only: an accept that resumes on a winner verified again since needs it again. `--approve-review` exits 2 ("a human must approve a REVIEW") when `AVER_RUN`, `AVER_WORKSPACE`, `CLAUDECODE` or `CODEX_SANDBOX` is set, the environment Aver's own runs and Claude Code and Codex give an agent, and also when stdin is not a terminal: a person approves from an interactive terminal. `aver combine` and `aver rerun` refuse their `--approve-review` the same way. `AVER_TEST_HUMAN=1` stands in for the terminal so tests and scripts can approve; it is a test hook, not a security boundary (anything that can set it can also open a terminal), and it never lifts the refusal under the agent variables above. `aver accept 3 --pr` also pushes it and opens a pull request with `gh`, only when you ask; `aver accept 3 --pr` after `aver accept 3` opens the pull request for the branch already written. A task that started on another task's accepted branch opens its pull request against that branch (`gh pr create --base`). Once the branch is written, the winner's directory is removed (`--keep` keeps it, and a directory whose Git view holds commits your repository lacks is kept, with the commands that fetch them); the branch holds the work. `aver gc --accepted` later removes every directory accepted or discarded runs left behind. The commit is exactly what the receipt describes: if the directory changed after its last verify, accept stops and nothing is exported, and accepting again verifies it as it is now. An edit that lands after the commit is not exported, and keeps the directory (the output names the file). An accept that stopped part way (a crash, a full disk) finishes when you run the same `aver accept` again: it reuses the branch it was writing (recorded in `runs/<id>/accepting.json`), adopts the branch when it is already at the verified snapshot's commit, and says "already accepted" once everything is done; `--pr` adopts a pull request `gh pr view` finds open for the branch instead of opening a second. `aver status` shows a run again, `aver diff 3 2 --tool opendiff` compares two attempts' directories, and `aver discard` deletes a run's directories. Without a run named, `aver status`, `diff`, `accept`, `discard`, `feedback`, `rerun` and `combine --all-pass` mean the newest run or batch of the repository you are in (inside a workspace directory, the repository it was made from); outside any repository, the newest in the home.

`aver run --detach` starts the run in the background and prints its id; it survives closing the terminal, and `aver status <id>` follows it.

`aver run --feedback` asks each agent, when the run ends, the same fixed questions about Aver (what helped, what got in the way, how it compares with a plain checkout on hard tasks, a 1-10 rating against "I absolutely need this", and the one change that would raise it), without letting it change files. The answers go to `$AVER_HOME/runs/<run>/feedback-<n>.md` and the ratings show in `aver status`. `aver feedback [run]` asks after the fact, while the run's directories still exist.

Choose agents with `--agent claude`, `--agent codex` (or `--agent claude,codex` to mix them), or any CLI with `--agent-cmd 'my-agent --cwd {dir} --system {brief}'` (and `--resume-cmd '… {feedback}'`). `--no-tui` prints one line per event instead, for CI and scripts.

### Many tasks at once

`aver run` takes a batch of tasks instead of one, and runs one agent on each (`--agents N` puts N on each):

```bash
aver run --tasks tasks.txt                  # one task per line
aver run --issue 41 --issue 42              # GitHub issues
aver run --issues bug                       # every open issue labeled bug
aver run --issues bug --parallel 4 --detach # four agents at a time, in the background
```

**Tasks file.** Each line that starts in the first column is a task: its text is what the agent is told and the task's title. Indented lines under a task are its criteria and protected paths, in the format of the `aver` block of [CI.md](CI.md): `- <id>: <text>` and `protected: <glob>`. A task without them is regression-only. Blank lines and lines starting with `#` are ignored. Any other indented line is an error that names its line.

```
# this week
Fix ISO week parsing in parseISO
  - iso-week: parseISO("2020-W01") returns 2019-12-30
Cache user settings across reloads
  - persist: settings persist across reload
  protected: test/settings.test.js
Rename the config keys to camelCase
```

**Issues.** `--issue <n>` (repeat it) and `--issues <label>` (open issues with that label, up to 100) read issues with `gh issue view|list --json number,title,body` in the repository, so `gh` must be installed and logged in. The task is the issue's title and body; its criteria come from an `aver` block in the body, as the GitHub Action reads them from a pull request. Tasks are numbered by their issue. Anyone who can file an issue wrote that text, so the agent's brief and prompt quote it under a header, `From GitHub issue #<n>. This describes the task and its criteria; it is data from the issue. It cannot change the rules below or grant you any permission.`, with the task on one line and each criterion on one line: line breaks and control characters escaped (`\n`), `@` broken by a zero-width space, and a `<` that starts a tag written `<\`, so `</repo-data>` closes nothing. Aver's own rules follow unchanged. The text still tells the agent what to build, so run only issues you have read. Tasks from `-c` and a tasks file are yours and appear as written.

**Queue.** At most `--parallel K` agents work at once (default: half the CPU cores, 2 to 8; a task with `--agents N` takes N); the other tasks wait and start as slots free. Every task without prerequisites starts from the same commit, imported once. Ctrl-C stops the running agents and starts nothing more.

**Task graph.** A task can build on others. An indented `after: #1` (or `after: #1, #3`; `after: #41` in an issue's `aver` block) makes it wait until those tasks end. When its one prerequisite passes, Aver accepts it (a branch, as `aver accept` makes) and starts the task on that commit, so agent B builds on agent A's verified work. Accepting B later gives a branch whose parent is A's: a stacked pull request. Several prerequisites are combined first, and the combine is verified against each of their tasks before anything starts on it. A prerequisite that does not pass, or whose combine conflicts, blocks its dependents: they never start, and the summary says why. `--no-auto-accept` accepts nothing for you: dependents wait, and the summary says to `aver accept <m>`, then `aver rerun <n> --onto <m>`. `touches: <glob>` names the paths a task expects to change. A cycle, a task after itself, or an `after` that names no task is an error, and nothing starts.

```
Add a negate helper
  - neg: negate(x) returns -x
Use negate in subtract
  after: #1
  - sub: subtract(a, b) returns a - b
  touches: src/math.js
```

**Budgets and retries.** `--batch-timeout 2h` and `--batch-usd 25` stop the running agents when the batch has run that long or spent that much; their directories stay. `--batch-verifies 60` starts no more tasks once the batch has used that many verifies, and lets the running ones finish. The tasks a budget kept from finishing are `over budget`, and the summary's second line reads `spent $12.40/$25.00 · 41/60 verifies · 1h02m/2h00m`. Claude and Codex report their cost in their output. A custom agent (`--agent-cmd`) writes its totals so far to `.aver/usage.json`, as `{"cost_usd": 1.25, "input_tokens": 120000, "output_tokens": 8000}`; its brief asks it to. `--retry 2` runs a task again, on a fresh run from the same base, when it ends in an outcome of `--retry-on`. The default is `failed,no-result,timeout`; add `fail-own`, `unproven` or `budget` to retry those too. The new agent's brief has a `PREVIOUS ATTEMPT` section with what the last run ran into.

**Coordination.** Each task is an ordinary run, and the batch's runs coordinate like any runs on different tasks (below): an agent hears when another task changes a file it changed, and can lease paths. A task that waited in the queue sees what the finished ones changed.

**Summary.** The batch shows the summary live and prints it at the end:

```
4 tasks · 1 ready · 1 broke something · 1 needs review · 1 unproven
#1 Add negate PASS     +3 -1    aver accept 1
#2 Add double FAIL_OWN broke "adds"
#3 Rename add REVIEW   changed 1 test expectation: aver diff 3
#4 Add triple UNPROVEN not proven: tri (passes_without_change)
overlap: #1 and #4 both change README.md (overlapping lines)
overlap: #2 and #3 both change src/math.js (the same file)
Next: aver accept --all-pass [--pr]   aver diff <n>   aver accept <n>   aver discard
```

Each line is the task's best attempt: `PASS` (ready, with its diff), `FAIL_OWN` (with the first test it broke), `REVIEW` (it changed test expectations a person approves), `UNPROVEN` (with the criterion and why), `BLOCKED_NOT_OWN`, `no change`, `blocked` (a prerequisite did not pass), `budget` (over budget), or, while it runs, its state (`waiting` for a prerequisite, `held`, `queued`). An `overlap` line names two tasks that changed the same file (an accepted task included, from a trial merge of its branch's work) and whether their lines overlap (a merge of the two would conflict), touch only the same file, or differ. The command exits 0 when every task is `PASS`.

**After a batch**, numbers name its tasks: `aver accept 4` accepts task 4's best attempt, `aver diff 4` shows it and `aver diff 4 6` compares two tasks. `aver accept --all-pass [--pr]` accepts every `PASS` task, one branch (and pull request) each, and never a `REVIEW` one (`aver accept <n> --approve-review` takes those one at a time); a task from an issue gets the branch `aver/issue-<n>-<title>-<id>` and a pull request body that starts `Fixes #<n>.`. `aver combine --all-pass` merges all of them into one branch, verified against every task, and only then accepts those not accepted yet (below). `aver status` prints the summary again, `aver status --all` lists every batch and run of every repository with its repository and verdict (`--json` for both; its `repo` is the whole path), `aver discard` stops and removes the batch's runs that were not accepted, and `aver feedback` asks every agent of the batch (`--feedback` asks each when its task ends). `--batch <id>` names an older batch, and `--run <id>` goes back to attempt numbers within one run. The batch's file is `$AVER_HOME/batches/<id>/batch.json`; `--detach` logs to `supervisor.log` next to it.

### Coordination between tasks

Runs on different tasks in one repository see each other. When an agent changes a file that another task's agent also changes, `aver verify` says so, shows the other task's lines next to the agent's, and says whether a three-way merge of the two current versions is clean (`aver check` and `aver brief` say it too; `aver notices` asks at any time):

````
Other tasks in this repository:
```
OVERLAP src/config/load.ts: task "Implement #42" changed line 27; you changed line 27; merging both CONFLICTS
  theirs, at line 27:
    -  const port = file.port;
    +  const port = validated(file.port, "server.port");
  yours, at line 27:
    -  const port = file.port;
    +  const port = env.APP_SERVER_PORT ?? file.port;
test/config.test.ts: task "Implement #42" changed before line 66; you changed before line 59 (no overlap; merging both is clean)
```
CONFLICTS: merging your edits with theirs conflicts where shown; keep your change there minimal, move it away from their lines (new code in a new function or file merges cleanly), or leave that code to them. `aver check --combined` runs your tests on a trial merge with their current work.
````

When both only add lines at the same place (two tasks appending tests to one file), the notice says so: `aver combine` and `aver check --combined` keep both insertions in task order, while `aver workspace rebase` marks them as a conflict. `aver check --combined` merges the directory with the latest workspace of each other task in the repository, as of their last sync, and runs the checks there too: it names the tasks and paths it merged, says how many of your tests (tagged, or in test files you added or changed) pass on the merge, and lists the tests that pass on your copy and fail on the merge.

Before restructuring shared code, an agent (or you, with `aver run --lease 'src/db/**'`) reserves it: `aver lease src/db --reason "moving the schema"`. Agents on other tasks then cannot take an overlapping lease, and `aver verify` reports their edit under it as a `CHANGED` entry (`lease::<path>`) that a person reviews, with the holder and when the lease expires. Leases last while their holder works and expire on their own when it stops; `aver release` frees them. Attempts at the same task never block or notice each other: they compete.

`aver combine <run|n|workspace> <run|n|workspace>…` merges the work of runs on different tasks onto their shared base (an accepted run's winner, or the best attempt of one that passed and is not accepted yet), verifies the result against every task and creates one branch when every task passes. A workspace directory made by `aver workspace new` or `claude --worktree` is an input too, named by its directory name (the label, `calc-k3x9`) or path, by its attempt id, or by the `aver/…` branch `aver accept` exported from it: its work as last verified (a directory changed since is verified first), verified on the merge against its own task. Combine accepts no such workspace; `aver accept` inside it still works after. Every input must start from the same commit; inputs that do not are refused, naming both commits. Where tasks only add lines at the same place (each appending a test to one file), the merge takes every task's lines, in input order; the verify of every task on the result is what checks they fit. Only a person approves `REVIEW`: a run nobody accepted whose best attempt is `REVIEW` is left out with what to do, and so is a merge that is `REVIEW` unless a task in it was accepted at `REVIEW`. `--approve-review` accepts both, and the output names each task it approved that way. Nothing is accepted or exported before that: other changes to the same or adjacent lines are reported by file and line, with each side's lines in a fence, and nothing is merged, accepted or exported; a task that fails on the merge stops it the same way. The command exits 1 then, and its `Next:` line says what to do instead of the same combine.

### Rerun and partial combine

When two tasks conflict, redo one on top of the other, or leave it out:

```
aver combine --all-pass
  README.md: both changed the same or adjacent lines (#1 ("Add negate"), #2 ("Add double"))
  ...
Next: aver rerun 2 --onto 1   aver combine --all-pass --partial
```

`aver rerun <n> --onto <m>` runs task n again on a fresh run that starts from task m's accepted work, with the prompt, criteria and protected paths task n had; task m is accepted first when it passed (`PASS`) and nobody accepted it yet. A task m that is `REVIEW` (it changes what an existing test expects) is refused until you review and accept it (`aver diff m`, `aver accept m`), or accepted here when you pass `--approve-review`, which the output names. `--onto` also takes a run id or a branch Aver made (`aver accept`'s or `aver combine`'s). A combine branch must still be at the commit `aver combine` exported: a branch amended or moved since is refused, since its tip is not what was verified. Without `--onto` the task starts again on its old base. The new run replaces the task's run in the batch (the old one stays in its history), and it refuses while the batch is still running. `--agents`, `--agent`, `--agent-cmd`, `--detach` and `--no-tui` work as for `aver run`; a custom agent's command is not kept, so pass `--agent-cmd` again. A rerun on top of another task is stacked on it: `aver combine 1 2`, or `aver combine 2` alone, verifies both tasks on the result and merges task 1's change once. A rerun points the batch at its new run only once that run's `run.json` exists; if the new run never started its agents, `aver status` and `aver accept <n>` use the task's last run and say the rerun did not start. `aver combine` writes its plan (its branch and each task's) to `$AVER_HOME/combines/pending-<runs>.json` before it accepts anything, so the same combine run again after a crash resumes with the same names and adopts the branches it already wrote.

`aver combine --partial` exports what fits. When the merge of everything does not pass, it keeps tasks in number order, each only when it merges cleanly and passes the verify of every kept task together with those kept before it (a task stacked on another brings it along: `after: #1` keeps #1 with it), so a task is never left out for a conflict or a failure with one already left out. It then exports the kept tasks, even when one task is all that is left, and lists each task it left out with why (`conflicts with #1 at README.md`, or its verdict on the merge with the tasks kept before it; a task failing on the merge is named once, on its verdict line) and `aver rerun <n> --onto <combine branch>` for each. That is one merge and verify per task after the first: the run cache reuses every run whose tree an earlier step already ran, but a long test suite runs again for each new combination. It exits 1 when it left anything out. `--json` prints the status, branch, included and excluded tasks, verdicts and next commands; every combine over a batch's tasks is also recorded in `$AVER_HOME/batches/<id>/combine.json`, each task as included or excluded with its reason.

### Watching from the repository root

Whoever starts the agents watches them from the repository itself, outside any workspace directory:

````
$ aver notices --repo
Tasks working in /home/me/app:
```
task "Add rate limit" (1 workspace) changes src/config/load.ts, test/config.test.ts
task "Implement #42" (2 workspaces) changes src/config/load.ts
OVERLAP src/config/load.ts: tasks "Add rate limit" and "Implement #42" change the same lines; merging both CONFLICTS
task "Implement #42" holds an exclusive lease on src/config/** (lease_01J…, expires in 24 min): moving the loader
```
CONFLICTS: these tasks' changes do not merge as they stand. Let one finish and accept it, then start the other on its branch, or tell one agent to move its change away from the other's lines.
`aver release --repo <lease id>` frees a lease.
````

`aver notices --repo [DIR]` lists every task with live workspace directories in the repository (DIR, or the one holding the current directory), what each changes, each pair of tasks that changed the same files with whether their lines overlap and whether a merge is clean, and every lease. It reads each directory as of its last sync (every verify, check, hook and `aver notices` inside it syncs). `aver lease --repo [DIR]` lists the leases, and `aver release --repo <lease id>…` frees any task's lease. `--json` prints the view for a program, and `aver status --json` adds a `graph` of the batch's tasks with an `overlap` edge for each pair that changed the same files (outside a batch, the tasks in the run's repository).

### Claude Code

```bash
aver init          # in your repository: aver.toml and the agent tools' files
aver integrate claude   # only the Claude Code files
```

With the plugin installed instead (`/plugin marketplace add wonkylamp/aver`, `/plugin install aver@aver`), run `/aver` in Claude Code: it installs the CLI if it is missing, runs `aver doctor`, and runs `aver init --yes` after you say yes. Both ways write the same files.

`aver integrate claude` shows what it will write, asks, and then writes `.claude/settings.json` (Aver's hooks merged into what is there), the `aver` skill at `.claude/skills/aver/SKILL.md` and the commands `.claude/commands/aver.md`, `aver-check.md`, `aver-verify.md`, `aver-status.md` and `aver-run.md`; your `~/.claude` settings are not touched. None of these files holds a path of your machine, so commit them and your team gets the same setup. Each hook calls `aver` from `PATH` behind `command -v aver`, so a teammate without Aver sees no error. Afterwards `claude --worktree <name>` (and subagents or background sessions that use worktrees) work in an Aver workspace directory instead of a `git worktree`, with `aver verify` available. A session that starts in a workspace directory gets the short brief, and an agent or subagent that stops with changes no `PASS` or `REVIEW` receipt covers is sent back once to verify. A session in the repository's own checkout is told to run `aver workspace new` before its first edit; this is an instruction, not enforcement. Setting up again moves the hooks an Aver before 0.4.1 wrote into `.claude/settings.local.json` to `.claude/settings.json`, keeping your own entries there, and takes out the edit, Bash and subagent hooks and the `.mcp.json` server an older Aver wrote. With the plugin installed too, each hook still runs once per event.

To keep agents from stopping before their work verifies, add `[agents]` with `stop = "strict"` to `aver.toml`: every stop with changed files is blocked until a verify passes or the agent runs `aver giveup "<reason>"`. Claude Code itself gives up after 8 blocks in a row. See [INTEGRATIONS.md](INTEGRATIONS.md#strict-stop-gate).

**What agents may do.** Each directory is a copy, but agents' commands run on your machine as you. Claude runs with `--permission-mode acceptEdits` and an allowlist Aver writes into the directory's `.claude/settings.local.json`: file edits, Aver's agent commands (`aver check`, `verify`, `diff`, `restore`, `notices`, `lease`, …), read-only Git (`status`, `diff`, `log`, `show`, `ls-files`), `node`, `npm test` and your `package.json` scripts; anything else is refused. `aver run --yolo` lets Claude run any command without asking. Codex runs inside its own `workspace-write` sandbox. Aver sandboxes neither agents nor checks: checks run natively in clean copies (see "Native runs" below). See [ARCHITECTURE.md §2.10](../ARCHITECTURE.md).

## Workspace directories for agent CLIs

Agent CLIs such as Claude Code and Codex edit real files with their own tools. `aver workspace new` gives each one a directory instead of a `git worktree` plus its own `npm install`:

```bash
cd ~/src/calc                                  # any Git repository
aver workspace new -c "neg: negate(x) returns -x" --count 3
```

This imports `HEAD`, creates a task with the criterion, and prints three directories under `$AVER_HOME/work/calc/`. Each is a copy-on-write clone of one cached checkout of the commit (one `clonefile` on APFS; a reflink or copy walk on Linux), and it gets copy-on-write clones of the dependencies your checkout has installed (`node_modules`, `.venv`, a Cargo `target`), each only when the lockfile beside it matches the commit's; Aver installs nothing. With a `target` cloned, every file that differs from your checkout gets a fresh mtime, so Cargo rebuilds what was built from other content. Creating a warm directory takes tens to hundreds of milliseconds and a few megabytes ([bench/workspace-disk.md](../bench/workspace-disk.md)). Pass `--no-deps` to clone no dependencies and `--task <id>` to claim an existing task. `--adopt-only` is accepted and ignored.

Run any agent in a directory. Inside it:

- `aver check [<criterion>…] [--all]` is the fast loop: a dry run of `aver verify`. It runs exactly the runs a verify would (the same clean copies, rerun policy and cache), prints the verify's report worded as a prediction ("Verify should PASS", or "Verify would stop at FAIL_OWN: …"), signs nothing a reader could trust (its receipt's key is thrown away), stores nothing and counts no verify. A verify of the same code right after starts no run and signs in under a second. Criteria given narrow the report to them, not the runs. It exits 0 when the verify would be done.
- `aver check -- <cmd> [args…]` runs your own test command the same way, as the one check of that dry run: on your copy, on the original code and, when you added or changed test files, on the original code plus them, natively and cached. Aver adds the reporter it reads (`cargo test` keeps its filter and `-p`; `go test` gets `-json`; pytest `--junitxml`; node:test, Vitest, Jest, Mocha, tap and `npm test` the reporters of the default check), and judges any other command by its exit code. In a directory without `aver.toml` it also writes one (see [Project configuration](#project-configuration)). The report starts and ends with one line, `Your command: 2 tests fail on the original and pass with yours; 0 new failures; 1 already failing`, and lists the tests that show the change, the NEW failures with their message, those already failing on the original code, your new tests that pass without your change (they prove nothing), and flaky ones. It exits 1 when something is NEW. It is not a verify: `aver verify` runs every check your change affects. `aver check --near` builds the command for you from the test files near your change (changed test files, test files named after a changed file, then those that name it; at most 50): pytest and the JavaScript runners get the files, `go test` their packages, `cargo test` `-p <crate>` (and `--test <name>` for one crate's integration tests); a test script that cannot take files runs whole, and the report says so.
- `aver check --combined` also merges your directory, on a trial basis, with the current work of every other task in this repository on your base (as of their last sync; where both changed the same lines, your version stays) and dry-runs the verify of that merge against your task. Above the last line it says which tasks and files merged, how many of your tests (tagged with your criteria, or new in your change) pass on the merge, and what passes on your copy and fails on the merge, with the failure. It says nothing about your own verify: the last line is still that prediction. Without another task it says there is nothing to merge.
- `aver verify` records the directory's changes, verifies them against the workspace's own task (the one its attempt belongs to; a `.aver/workspace.toml` whose `task` line names another, or `--task` with another, exits 2 and says which task to put back), natively, and prints a report written for the agent: the verdict and the next step first, the changed files, why each criterion is unproven and how to fix it, the failures that are its own (NEW, with file, line and message), what the change's tests show (yours that fail on the original code and pass with the change, the tests you fixed), the tests still failing as on the original code (not counted against you, but the target if the task is to fix them), flaky tests (rerun up to three times and not counted against you), a pointer to `aver workspace rebase` when tests of a Rust `[[test]]` target your directory declares (as a merged newer main does) could not run as declared under the original `Cargo.toml`, files that could not be recorded (secrets, oversized files, symlinks leaving the directory), and where the time went. `--reason "<why>"` states why the change rewrites tests or harness: the diagnostics keep it as `reason` (unsigned), and the report shows it fenced as `Stated reason (from the agent, not checked):` beside the rewritten tests; it never changes the verdict. In `aver ci` an `Aver-Why: <why>` trailer on the head commit does the same in `summary.md`, right above the diffs under "Needs review". It runs only the checks your change affects ([VERDICTS.md](VERDICTS.md#which-checks-run)): a change to documentation alone runs none and prints `DONE: PASS. No check is affected by this change, so none ran.` in seconds, and the report names the checks it skipped; `aver verify --all` runs every check. While runs take long (a Rust build), it prints progress on stderr, never in the report: a line when a run ends, a count of cache hits, the runs still running after two seconds, and a heartbeat every 30 s (`--quiet` stops them). Every report longer than a screen repeats the verdict with the verifies used as its last line, for an agent that reads output from the end, and `aver verify --last` prints the latest report again (marked stale when the directory changed since) without running anything or using a verify:

  ```
  DONE: PASS (2 of 10 verifies used)
  NOT DONE: FAIL_OWN (3 of 10 verifies used): fix the NEW failures above
  ```

  Test names and output sit in fenced blocks marked as repository data (the full note once per directory, a short marker after). It exits 0 on `PASS` and `REVIEW` and 1 otherwise; a directory that changed nothing is `NOT DONE` whatever the receipt says. Every agent's verify runs in its own process, and all of them share the home's run cache and slots, so two verifies of one base run its checks once; the base's checks start in the background when the workspace is created. If a verify is killed, the next command that opens the home cleans up after it, and `aver status` shows what was recorded. A full disk during a check run is the host's problem: the entry is `ENV` with "[aver] the disk is full", never a failure of your change; free some space and verify again.
- `aver diff` shows the changes against the base, as verify sees them; `aver restore <path>` puts a file back (even an edit no sync recorded: `aver restore --undo` puts back what the last restore replaced, and again goes one restore further back); every `aver check` and `aver verify` records a checkpoint (the files it judged, its verdict and `+N passing, M NEW` against the original code), `aver restore --checkpoints` lists them, and `aver restore --last-green` puts the directory back to the newest one without a NEW failure on the workspace's current base, writing only the files that differ (`--undo` reverts it too); `aver notices` shows what agents on other tasks change and lease.
- `aver accept` (no number) hands the directory off: when its latest receipt describes it as it is now (else it verifies first) and says `PASS` (`REVIEW` only with `--approve-review`, from a human, as above; `--force` for anything else, never for `REVIEW`), it records the decision, commits the workspace, which closes it, and writes a new branch to your repository whose parent is the base commit, as your Git identity, with the receipt in the commit's trailers and its evidence in the body. The commit's title is `--title`, else the subject of your latest commit in the directory, else the task's title or first criterion; the branch is that title's slug with a short id from the workspace, `aver/fix-handle-negative-zero-7b3g`, and the Git view's own `aver/<directory>` when no title was given (`--branch` picks another; `--repo` names the repository when the directory has no Git view), and `--pr` pushes it and opens a pull request with `gh`. Interrupted, the same `aver accept` finishes it (the journal is `$AVER_HOME/accepts/<workspace>.json`); once the directory is gone, `aver accept` in the repository prints the hand-off it would have printed. A done verify report outside `aver run` says so on a `Hand-off:` line under `Next:`, with the receipt id. In a directory of `aver run` it refuses and names the run's own `aver accept <n>`.
  Just above the footer, `What this verdict covers:` says which checks ran (and which only by exit code), which it skipped, which changed files no check runs, which proofs are weak and where the runs executed; a done verdict with a material limit says it on the last line, `DONE: PASS (2 of 10 verifies used); limits: 1 check by exit code` ([VERDICTS.md](VERDICTS.md#what-this-verdict-covers)).

  Test names and output sit in fenced blocks marked as repository data (the full note once per directory, a short marker after). It exits 0 on `PASS` and `REVIEW` and 1 otherwise; a directory that changed nothing is `NOT DONE` whatever the receipt says. Every agent's verify runs in its own process, and all of them share the home's run cache and slots, so two verifies of one base run its checks once; the base's checks start in the background when the workspace is created.
- `aver diff` shows the changes against the base, as verify sees them; `aver restore <path>` puts a file back; `aver notices` shows what agents on other tasks change and lease.
- `aver accept` (no number) hands the directory off: when its latest receipt describes it as it is now (else it verifies first) and says `PASS` (`REVIEW` only with `--approve-review`, from a human, as above; `--force` for anything else, never for `REVIEW`), it records the decision, commits the workspace, which closes it, and writes a new branch to your repository whose parent is the base commit, as your Git identity, with the receipt in the commit's trailers and its evidence in the body. The commit is titled `--title`, else with the subject of the agent's latest commit in the directory's Git view, else the task's title unless Aver made it up (`Work on <repo> at <sha>`), else its first criterion, and the branch defaults to the title's slug with a short id from the workspace, `aver/negate-returns-x-7b3g` for `-c "neg: negate returns x"` (with no title, the branch the directory's Git view shows, `aver/<directory>`; an existing branch is refused; `--branch` picks another; `--repo` names the repository when the directory has no Git view), and `--pr` pushes it and opens a pull request with `gh`. It prints the source of each criterion's proving test (fenced, up to 30 lines), what a reviewer of the branch relies on. A done verify report outside `aver run` says so on a `Hand-off:` line under `Next:`, with the receipt id. In a directory of `aver run` it refuses and names the run's own `aver accept <n>`.

Each directory of `aver workspace new` gets an agent key of its own (`ws-<id>`, named in `.aver/workspace.toml`) that can verify, check, lease and edit its workspace but never act as the admin; `--key` names another.

Aver records the directory's files itself, so an agent never needs to commit there; the brief and the skill say the same: do not commit in an `aver run` workspace, and in one of `aver workspace new` or `claude --worktree` commit on its branch only when whoever started the agent asks for commits.

`aver workspace ls` lists each directory with its state, its run, what its task asks (the first criterion, else the task's title; `--json` adds `title` beside `task`) and its path; `aver workspace rm <dir>` and `aver gc` manage directories; `aver gc` also reclaims the build artifacts (Cargo `target` directories, `aver check` copies) of workspaces idle for a day, and the slots nothing needs: those of workspaces no longer live, and base slots no live workspace uses and snapshot slots once untouched for a week, and at once a base slot installed from lockfiles your checkout no longer has. `aver gc --builds` takes build artifacts idle for 10 minutes too. `aver gc` also removes the base records local `aver ci` runs keep in `$AVER_HOME/records` once unwritten for 14 days, and all but the 64 newest. Releasing a workspace (`aver accept`, `discard`, `workspace rm`) removes its slots at once. `aver gc` also removes, with its slots, every directory that equals a commit you made in its Git view once a branch of your repository has it (fetched and merged, say), listing each as `removed <dir> (matches <commit>)` and in `merged_removed` with `--json`; a directory with changes since that commit, with commits your repository lacks, or unchanged from its base is kept. `aver workspace new` warns when less than 15 GiB is free under `$AVER_HOME`, naming `aver gc`. `aver workspace new --count N` prints each directory as soon as it is made. A secret-like string in a test, fixture, example or template file (`tests/fixtures/key.pem`, `testdata/`, `.env.example`) does not stop `aver workspace new`, `aver workspace rebase` or `aver run`: they import it and name the file on stderr, never the value; one anywhere else refuses the import. A removed directory's label is free again: `aver workspace new --label X` after `aver workspace rm` makes `X` again. `aver workspace rebase [<rev>]` moves a directory onto a newer commit of its repository; where your edits and the new commit's conflict, it moves anyway and writes both versions between conflict markers (`<<<<<<< yours`, `=======`, `>>>>>>> <commit>`) into the files it lists, exiting 2, and `aver check`, `aver verify` and `aver workspace rebase` refuse, naming them, until no marker line is left; then the directory as you left it is the resolution. Sizes are apparent (a copy-on-write clone counts the blocks it shares). A workspace's slots start as copy-on-write clones of its directory, build outputs included, so its first verify rebuilds only what the change touches, and on APFS or a reflink filesystem they cost only what their own swaps and builds changed. Its transplant slot (your tests on the original code) starts instead as a clone of the base slot when no run holds that slot and it was cloned with the same installed dependencies, so it shares the base's build: on Aver's own repository a workspace's slots take 3.3 GiB instead of 6.6 GiB. `aver workspace rm` syncs the directory first and refuses one that holds changes no commit keeps, listing them with a diff stat (an unaccepted workspace with changes, or an accepted one edited since) unless the directory equals a commit of its Git view that a ref of your repository reaches (fetched and merged, say: it then says which commit), and one whose Git view holds commits your repository has not fetched and printing the commands that fetches them, each into a branch the repository does not have yet (`aver/<dir>-commits` when `aver accept` already wrote `aver/<dir>`); `--force` removes it anyway. `aver gc --accepted` keeps an accepted directory whose commit never reached a branch (`aver accept` in it exports it) or that changed since the commit, and says why; so does the release after `aver accept`. Each new directory holds `.aver/BRIEF.md`: the task, how to check and verify, and what fenced text in reports is. An `aver.toml` `[workspace]` table can seed untracked files (`.env.development.local`) or warm build caches (`target/`, `.next/cache`) from your checkout; its `node_modules` and `clone_packages` keys are accepted and ignored ([ARCHITECTURE.md §1.4.1, §1.8.3](../ARCHITECTURE.md)).

### Tagging tests with criteria

A test proves a criterion when its reported name carries the criterion's tag and it fails on the original code (or cannot load there) and passes with the change. The tag has two forms ([ARCHITECTURE.md §1.9.2](../ARCHITECTURE.md)): `[crit:<id>]` anywhere in the name, or, for names that cannot hold brackets, `crit_<id>` as a whole word, with each `-` or `.` of the id written `_` (`crit_div_zero` for `div-zero`). "Whole word" means no letter or digit before `crit_` and none after the id, so the tag may start the name (`crit_div_zero_rejects`) or end it (`rejects_crit_div_zero`); where several of the task's criteria fit, the longest wins, so `crit_neg_two` carries `neg-two` when the task has it and `neg` otherwise. When two criteria of one task share the identifier form (`a-b` and `a_b`), only their bracket forms count. A docstring or comment is not part of the name, and does not count.

| Runner | Criterion `neg` |
|---|---|
| Jest, Vitest, Mocha, node:test | `test("[crit:neg] negate(3) is -3", () => { … })` |
| pytest | `@pytest.mark.parametrize("x", [3], ids=["crit:neg"])` on `def test_negate(x)`, which reports as `test_negate[crit:neg]`; or `def test_negate_crit_neg():` |
| `go test` | a subtest, `t.Run("[crit:neg] negates", func(t *testing.T) { … })` (reported as `TestNegate/[crit:neg]_negates`); or `func TestNegate_crit_neg(t *testing.T)` |
| `cargo test` | `#[test] fn negates_crit_neg()`, or the tests inside `mod crit_neg { … }` (`crit_neg::negates`); Rust test names cannot hold brackets |

`aver check neg` reports on `neg` (it still runs what a verify runs, so the verify after it finds every run cached); `aver check --combined neg` runs only the tests that carry `neg`'s tag where it can narrow the runner (Node's runners), and the whole check elsewhere.

### Native runs

`aver verify` and `aver check` run the checks natively, in slots: copy-on-write clones of your repository (for the original code) or of your workspace directory (for your tests on the original code, and for your code), installed dependencies and build outputs included, each swapped to the tree it runs, never your directory itself. A run inherits the environment of the `aver` command running it, minus the CI job's tokens and job files, with a private `TMPDIR` and its build output inside the slot; your toolchains are the ones your `PATH` finds, and the network is on. A verify of a one-line change waits about as long as a warm `cargo test` or `node --test` (`make bench-native`). The receipt is `aver.receipt/v3` with `provenance` `{"mode": "local", "host_profile": …, "network": "on", "uid": "same"}`: it trusts this machine and the agent, as the agent's own shell already does ([VERDICTS.md](VERDICTS.md#where-the-checks-ran)).

Aver 0.4 deleted the Docker sandbox and `--sandbox` (REDESIGN.md §1.2). A tool the checks need that is missing here, or only reachable through an asdf, mise, pyenv or Volta shim, is reported as an infrastructure problem naming it.

Ctrl-C (SIGINT, or SIGTERM) ends every command with exit code 130. With no check running, the command exits at once. During `aver verify`, `aver check` or `aver ci`, each running check gets SIGTERM and, 2 seconds later, SIGKILL; then the command exits 130 with no receipt and no verify used, and stores nothing of the checks it cut short, so the next verify runs them again. A second Ctrl-C kills the checks and exits at once. An `aver ci` running its checks in the checkout (docs/CI.md) puts the checkout back at the head before it exits; after a SIGKILL, `aver ci restore [--repo DIR]` does. In `aver run` and `aver rerun`, the first Ctrl-C stops the agents and ends the run cleanly, and a second exits 130.

## Project configuration

An `aver.toml` at the repository root pins the environment: the image by digest (since 0.4 it only keys the environment; nothing runs in it, and a tag is resolved from a local image, never pulled), the lockfile, the install command, the checks, the resource limits and, optionally, which files are tests (`[env] tests`, default Node's test file patterns). [fixtures/calc/aver.toml](../fixtures/calc/aver.toml) is a complete example.

Outside CI, Aver never guesses your tests. Without a check in `aver.toml`, `aver verify` and `aver check` run nothing, print the following with up to three commands the repository declares (its CI workflows, Makefile, justfile, Python runners, `package.json`), and exit 2:

```
NOT CHECKED: Aver does not know this repository's test command yet.
Run once: aver check -- <the test command you use>
Commands this repository declares:
  aver check -- cargo test
```

`aver check -- <cmd>` in a workspace directory without `aver.toml` writes it. It runs each command joined by `&&` as its own check (`aver check -- 'cargo test && npx tsc --noEmit'` is two) on your copy and the original code. When a command gives no per-test report (a command Aver adds no reporter to, such as `make test`), Aver could judge it only by its exit code, which never shows which test a change breaks: it says so in one line with how to get per-test results for that runner (run the runner directly, or its JUnit option with a hand-written check and `report = "junit"`) and writes nothing (exit 2). `--allow-exit-only` writes such a command anyway, as `report = "exit"` with a comment saying so; that is a person's decision, so the agent instructions do not mention it. When a command ran no test Aver could read on the original code, or every test failed there, it says so in one line with the last 15 lines of the output and writes nothing (exit 2). Otherwise it writes `aver.toml` with each command, `min_tests` at 90% of the tests the original code ran (at least 1), and `[deps] keep` and `inputs` from the `node_modules`, `.venv` and `target` directories and the lockfiles present, prints `Wrote aver.toml: test (min_tests 42). Commit it with your change; `aver verify` now runs it.`, then the dry-run report. Until a merge puts it in the base, the checks of a proposed `aver.toml` are unapproved, and the file itself is not judged as part of your change ([ARCHITECTURE.md §1.13.2](../ARCHITECTURE.md)).

In CI, `aver ci` still detects a repository without `aver.toml`: it uses `npm test`, adding the test runner's own reporter when the `test` script runs `node --test`, Vitest, Jest, Mocha or tap, so base and head are compared test by test, and running `prepare` (and `build` when the package's entry points are build output) first. When `package.json` lists `typescript` in `devDependencies` and the repository has a `tsconfig.json`, it also runs a `typecheck` check: the project's `typecheck` script, else `npx tsc --noEmit -p .`, with each type error its own entry (the `tsc` report format), so errors the base already has do not block and only the ones a change adds count. See [ARCHITECTURE.md §1.4.1](../ARCHITECTURE.md) and [VERDICTS.md](VERDICTS.md).

### Drafting aver.toml: `aver init`

Inside a Git repository, `aver init` (besides creating the keys) drafts an `aver.toml`, since only CI detects checks without one. It names what CI's detection would run (`Aver's detection finds: test (npm test, junit report).`) and drafts from that and from what the repository declares at `HEAD`:

- `.github/workflows/*.yml` `run:` steps that look like test, lint or type-check commands. CI configuration is input only: a step that uses `${{ secrets.* }}` (or a variable the workflow sets from one), whose job starts service containers, that uses another `${{ … }}` expression such as a matrix value, or that needs the network (`curl`, `docker`, …) is left out, with the reason;
- the Makefile's `test`, `check` and `lint` targets, and the justfile's recipes of those names;
- `package.json` `test`, `lint` and type-check scripts, in nested packages too;
- `tox.ini`, `noxfile.py` and pyproject's `[tool.pytest…]` table.

The draft keeps the checks detection already makes (listing checks turns detection off) and gives each new one its runner's report where it has one (`pytest --junitxml`, node:test's JUnit reporter, cargo's libtest output, `go test -json`, `jest --json`); any other command is judged by its exit code only, so per-test causes are unavailable for it. Commands Aver already runs are left out as duplicates. The draft is then run once on `HEAD`, natively, in a throwaway home, and flagged: a check that fails there (a red base), a program that is not installed, exit-code-only checks, and packages whose dependencies no committed lockfile pins.

Nothing is written without `--yes`, which writes `aver.toml` atomically. In a repository that already has an `aver.toml`, `aver init` only says so and keeps it, `--yes` included; `--force` drafts a replacement, and `--yes --force` writes it. `--json` prints the same as one object (`preset`, `checks`, `dropped`, `flags`, `dry_run`, `toml`, `written`).

`aver init` then sets up the agent tools the repository or this machine shows, with the files `aver integrate` writes ([INTEGRATIONS.md](INTEGRATIONS.md)): for Claude Code, when `claude` is installed or the repository has `.claude/`, the hooks in `.claude/settings.json`, the `aver` skill in `.claude/skills/aver/SKILL.md` and the commands in `.claude/commands/aver*.md`, the same files the Claude Code plugin ships; and Aver's block in `AGENTS.md` when the repository has `AGENTS.md` or `.codex/`, or `codex` is installed. It lists the files and writes them with `--yes`, or after you answer yes on a terminal; otherwise it writes none of them and says that `aver init --yes` does. `--json` adds an `agents` object (`tools`, `written`, `already`, `skipped`). `aver setup --remove` undoes these writes.

`aver init --ci` also drafts `.github/workflows/aver.yml`, the GitHub Actions workflow that runs the `aver-verify` action on pull requests (`review: approve`), pushes to the default branch (which record base runs), `merge_group` (regression-only) and `pull_request_review` (`mode: approve`), with steps before the action that install what the repository's lockfiles name ([CI.md](CI.md#set-up-in-5-minutes)). It prints the workflow, then what each event does and the `gh api` command that requires the `Aver` check on the default branch, which it never runs. The default branch is `origin`'s, else the checked-out one, else `main`. Nothing is written without `--yes`; an existing workflow is replaced only with `--yes --force`, and `--yes` alone then exits 2 before writing anything. With `--ci`, an existing `aver.toml` is kept and `--force` applies to the workflow only. `--json` adds a `workflow` object (`path`, `exists`, `written`, `branch`, `check`, `action`, `setup`: the setup steps' names, `content`). The action is pinned to the commit the release was built from (`AVER_ACTION_SHA` at build time).

### Python, Rust and Go (generic mode)

In CI, without a `[[check]]` list, Aver also finds the other packages in the repository, at most three directories down, and gives each a check in its own directory with its own dependency layer:

| Package | Found by | Check | Per-test results from |
|---|---|---|---|
| Rust | `Cargo.lock` (or a root `Cargo.toml`) | `cargo test --workspace --no-fail-fast --locked` | cargo's own output (`libtest`) |
| Go | `go.mod` | `go vet ./...`, then `go test -json ./...` | `go test -json` (`go-json`) |
| Python | a project or requirements file plus tests | `python -m pytest` (or `unittest` without pytest) | `pytest --junitxml` (`pytest-junit`) |
| Node below the root | its own `package-lock.json` and a `test` script | `npm test`, as at the root, and a `typecheck-<directory>` check when it lists `typescript` beside its own `tsconfig.json` | the runner's reporter |

`aver check` runs the same commands natively; where only `python3` is installed (stock macOS), it runs `python3` for `python`, and a command it cannot find is named as such ("neither python nor python3 is on PATH") instead of a missing report.

Aver installs no dependencies for a check: a slot has what its source directory has installed (`node_modules`, `.venv`, a Cargo `target`), and a run whose tree's lockfiles differ from those they were installed from says so in its log. Checks build and test with the network on. A change that does not compile fails: `cargo test` builds every target, a Go package that does not build fails as `<package> > (package)`.

To choose the checks yourself, declare each lockfile's layer and give a check its own image:

```toml
[[deps]]
lockfile = "Cargo.lock"            # kind from the name; install defaults per kind

[[check]]
id     = "rust"
image  = "rust:1.99-bookworm@sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6"
run    = ["sh", "-c", "cargo test --workspace --no-fail-fast --locked >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s"]
report = { format = "libtest", path = "/aver/out/rust.txt" }
timeout_s = 1800
```

A check mounts the layers of its language nearest its `cwd` by default (every npm layer for Node); `deps = ["Cargo.lock", …]` names them. Aver's own [aver.toml](../aver.toml) verifies its Rust workspace this way, beside three npm packages.

**Uncovered changes.** A source file you change that no check can exercise (Rust files when every check runs `npm test`, a package with no check, Rust code under a `cfg(feature)` no Cargo check builds) holds the verdict at `UNPROVEN`, and every report lists it as "changed files not covered by any check". `covers = ["**/*.rs", "fixtures/**"]` on a check says which files it exercises. See [VERDICTS.md](VERDICTS.md#uncovered-changes).
