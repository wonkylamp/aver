# How Aver decides a verdict

Aver runs every check on the base commit and on the head, compares them entry by entry, gives each entry a cause, and derives one verdict from the causes. This page explains the rules with worked examples. `ARCHITECTURE.md` §1.9.3 is the normative text; `crates/aver-core/src/rules.rs` implements it as pure functions.

## The sides

| Side | What runs |
|---|---|
| base | The base commit, with its own harness and dependencies. |
| head | The *judged head*: the head's code with the base's frozen harness (`aver.toml`, `.npmrc`, test runner configs, the `scripts` and runner fields of `package.json`), the base's copy of every test file that exists at base, and the base's versions of the tests inside Rust source files (`#[cfg(test)]` and `#[test]` items, and doc-tests in doc comments). For the repository's own per-test checks it holds none of the test files the head added, so an added file cannot patch the code the base's tests judge (a module that monkeypatches it, a new `conftest.py`, a Go `init`); a task check, or a check judged by its exit code, runs with them. |
| head-tests | The head with its own versions of the test files (harness still from the base). It runs when the head added, modified or deleted a test file. |
| transplant | The base plus the head's new and modified test files, and the head's versions of the test items inside Rust source files on the base's code. It proves criteria: a test that passes here does not need the change. |

A failing base or head side is rerun until it has three results or disagrees with itself; a base that fails as a whole, with no test results, stops after a compile error or two identical failures (same exit code, same output). The transplant runs once: a proving test is expected to fail there. Where the runner selects tests by name (Go, pytest, Cargo, `node --test`, and the runner a default `npm test` script runs: Node's test runner, Vitest, Jest or Mocha), a side whose first run failed 1 to 50 tests and passed others reruns only the failing tests, and every other test keeps its first result; a rerun that does not report every test it was asked for runs the full command instead ([ARCHITECTURE.md](../ARCHITECTURE.md), Focused reruns). An infrastructure run (out of memory, timeout, a run killed from outside) is retried once. When the head disagrees with itself on an entry the base passed, the base is rerun to three results, so a flaky head is judged against a base that passed three times or showed its own flakiness. Test ids do not depend on where a run happened: the run's directory reads `<src>` and its temporary directory `<tmp>`, and a test whose parameter changes on every run (a timestamp) is matched to its base id after one more base run shows the base's id changing too. No run sees which run it is: no run has an `AVER_RUN_ATTEMPT`, so a head cannot fail its first run only and pass as `FLAKY`.

**Test files.** Without `tests` patterns in `aver.toml`, a test file is one of Node's discovery patterns (`*.test.*`, `*.spec.*`, `*-test.*`, `*_test.*`, `test-*.*`, `test.*`), anything under a `test`, `tests` or `__tests__` directory, and what tests read or run outside those: Django's `tests.py`, `conftest.py` anywhere, `__snapshots__` and `*.snap`, `golden*`, `fixtures`, `__fixtures__`, `__mocks__` and `testdata`, and Jest and Vitest setup files. Before, a head could rewrite a golden file or a `tests.py` along with the code and verify `PASS` with nothing in `test_changes`. A Rust source file whose test items the head changed is listed among the modified test files too.

## Causes

Each entry is a test (or a whole check without per-test results). Its cause is one lookup in a table (`aver_core::judge`) over five things: where the test lives and what the head did to that file, the base's result, the judged head's result, the head's own version of the test, and the transplant. Every cell is frozen in `crates/aver-core/tests/testdata/judge_table.txt`; the rules below describe it. Since the table replaced the old chain of rules, five cases read differently, on purpose:

- A side that ran and reported no test result at all is `ENV`, and so is a base or judged head whose runs ended in infrastructure, even after an earlier run reported.
- A test the head adds that flakes is `NEW`: nothing on the base explains its failing runs. It used to be `FLAKY`, which passed.
- A test whose base copy passes on the head but which the head skipped, or deleted from a file it modified, is `CHANGED`, so a person sees the removal. It used to be no cause at all.
- A test in a test file the head deleted never counts as passing.
- The head's own version of a test that hangs counts as failing, and an own version whose result Aver lost is `ENV`.

First match wins:

| # | Condition | Cause |
|---|---|---|
| 0 | Every head run hit a resource cap, and the base finished (passed or failed) | `NEW` |
| 1 | A side ended in an infrastructure failure | `ENV` |
| 2 | The head's runs disagree, and the base passed every run or had no such test | `NEW` |
| 2a | A side's runs disagree | `FLAKY` |
| 3 | Head fails, base passed or had no such test | `NEW` |
| 4 | Head fails, base fails | `PREEXISTING` |
| 5 | Head passes | none |

A whole check, judged by its exit code because the head's runs ended without a readable report while every base run reported each test (a build the change breaks, a module that throws when it loads, a test named `(exit)` that makes the report malformed), is `NEW` whatever already failed on the base: the base's runner reported every test and the head's did not. Likewise rule 0 used to need a green base, so a head that hung over a base with one failing test was `ENV`.

Rule 2: a head that fails some runs of a test the base passes three times is the change's failure. Read as `FLAKY`, which does not block, a regression that failed half the time would pass seven verifies in eight. A test flaky on the base too stays `FLAKY`. This makes an innocent change over a test that is flaky but passed its three base runs `FAIL_OWN` now and then; that cost is the honest side of the trade, since the alternative lets a race the change introduced pass.

**Setup findings.** The Go preset runs `go vet` as a setup command, and `aver.toml` may declare others (`mypy`). Each line they print for a failing setup command (`<file>:<line>: error: …`, `<file>:<line>:<col>: …`) is its own entry, `<check>::(setup) <file>: <message>`, keyed without the line number. A finding the base already has is `PREEXISTING`; one the change adds is `NEW`. So a base with one `mypy` error hides no new one. When a run's output was cut to its limit, findings are not read and only the `(exit)` entry counts.

**Recorded base runs (`aver ci --base-record`).** When the base side's runs all came from a base record (a run of the same commit by an earlier CI job, docs/INTERFACE.md §1.1) and the head fails tests they passed, Aver runs the base once more, fresh, before judging. If the fresh base fails one of those tests too, or its command fails, the base side runs fresh and the entries are judged from it: a test that fails on both is `PREEXISTING`. If the fresh base passes, the entries stay `NEW`. A failure the record's base passed is never excused without a base run on this runner. A test the record's base itself failed is judged from the record, like a cached run: the record is the same commit on the same runner key, and only the default branch's CI writes the records a pull request can restore (action/README.md).

Then a `NEW` or `PREEXISTING` entry is looked at once more:

| # | Condition | Cause |
|---|---|---|
| 7a | The entry is `NEW`, the base never ran its test, and its Rust test target is declared in a `[[test]]` table of the head's `Cargo.toml` that the base's frozen copy lacks or defines otherwise | `CHANGED` |
| 6 | The head modified or deleted the existing test file that defines the test, and the head's own version of it passes (or, for a `PREEXISTING` test, is gone: deleted or skipped) | `CHANGED` |

**A skipped or deleted test (review fix H2).** A `NEW` test whose head version is gone (the head deleted its file or the test, or marked it `it.skip`) stays `NEW`: the base's copy passed on the base and fails on the change, and there is no new expectation for anyone to review, so it is the change's regression, `FAIL_OWN`, with the note "the PR skipped or deleted this test … restore the test and fix the code". Before, a gone own version counted like a passing one, so "break `add`, then `it.skip` its test" was `CHANGED`, `REVIEW`, and exit 0 in CI. A `PREEXISTING` test the head removes (it already failed on the base) is still `CHANGED`.

**A restored test file that does not compile.** The judged head puts back the base's copy of every existing test file the head modified or deleted. When the change removes or changes what such a file uses (a public function deleted together with its tests, or a signature changed on purpose with the file's calls updated to match), that copy no longer compiles against the head's code, and `cargo test` builds no test target at all: before, the whole check lost its report and was one command-level `NEW`, so the change was `FAIL_OWN` with nothing to fix but "put back what you meant to remove". Now, for a Rust check whose head runs lost the report while the head's own test files build, Aver builds the judged head with `cargo build --tests --keep-going` and reads which test targets did not build and why. If every error is in a test file the head modified or deleted, each such file is one entry, `<check>::(did not compile) <path>`, `CHANGED`, with the file's first compiler error as its detail (fenced in the reports, as repository text), and `cargo test --no-fail-fast` runs again over only the targets that built. Their tests are judged one by one as usual, so the verdict is `REVIEW` unless one of them is `NEW`: a regression in a test file the head left alone is still `FAIL_OWN`, and a restored test that compiles and then fails at runtime is still `NEW` (rule 6 and H2 above). A test whose target did not build has no entry of its own; its file's entry stands for it, and so does the entry of a modified shared module (`mod common;`) the target declares. A target that built but that Aver's rerun did not report is Aver's fault, never the change's: its tests are one `ENV` entry, `<check>::(not run again) <target>`, not `NEW` each. So is a build whose result Aver lost (its run ended in infrastructure, or cargo's stream, which it writes to a file of its own, stops before `build-finished`): the check's one entry is `ENV`, not `NEW`, since nothing then says whether the restored files or the change stopped the build. `summary.md` and the agent report say how many test files did not compile and that the remaining tests were judged one by one, instead of "they still ran from the base version".

**A deleted source file that held tests.** A Rust source file the change deletes takes its `#[cfg(test)]` items and doc-tests with it, and nothing of the code they tested is left to run them against. Before, each such test was missing on the head and `NEW`, so deleting a module was `FAIL_OWN`. Now the base tests of that file that the head no longer reports are one entry, `<check>::(deleted with its code) <path>`, `CHANGED`, naming the file and how many of its tests the base ran: the verdict is `REVIEW` at best, and never `PASS`. A source file the change keeps but whose test module it deletes is judged as before: the base's tests go back in, and one that compiles and fails is `NEW`, because the code under test is still there.

This applies only to a check whose command is `sh -c` (or `bash -c`) running one plain `cargo test` (environment assignments before it, unquoted arguments, targets chosen with `--lib`, `--bins`, `--tests`, `--test`, `--bin` or `--doc`, or none, with or without `--examples`), as the Rust preset and Aver's own `aver.toml` do. In 0.4.1 and before, a command with `--examples`, Aver's own among them, was not read, so a signature change whose test file the head updated was `FAIL_OWN`. Any other command (a `cargo nextest`, a Makefile target, `--all-targets`, quoted or computed arguments) keeps one command-level `NEW` when the head loses the report. An error outside the restored files (the head's own code, a test file the head left alone) also keeps it.

Rule 6 holds whatever the test's base result. The base's copy fails on the head and the head's own version passes, so the head rewrote what the test expects; the base's result only says whether the old expectation held before. A test that already failed on the base and passes only in the head's rewritten version was not fixed: it is `CHANGED`, not `PREEXISTING`, and the verdict is `REVIEW` with the diff. Without it, rewriting an already-failing test into a passing one would verify `PASS` (the c2 example below). A test whose base copy still passes on the head is not `CHANGED`, even when the head edited it: the base's expectation judged the change and held, so the edit hid nothing (adding a case to an existing file is the common example).

Rule 7a is for a workspace that merged a newer main. Main gave a new test target `required-features = ["faults"]`; head runs build test targets from the base's frozen `Cargo.toml`, so the target built without its feature and its tests failed. Before, they were `NEW` and the verdict `FAIL_OWN`, blaming the agent for main's manifest. Now they are `CHANGED`, the `[[test]]` edit is a `judging::…/Cargo.toml#test` entry beside them, the verdict is at best `REVIEW`, and the agent report says to run `aver workspace rebase`, which moves the workspace onto the merged commit so main's changes stop counting as the agent's. A test the base ran is still judged by the base, so an agent that edits `[[test]]` to keep a failing test from building still gets `FAIL_OWN`, and one that only edits it gets `REVIEW`.

Frozen harness edits (`judging::<path>` entries) are `CHANGED` too: every check still ran with the base's copy, so the edit could not change a result, but a person should see it.

So are compiled test files the change's build rewrote (`built::<path>` entries). Both sides compile the same frozen test sources, so a compiled test file (`dist/test/v4.test.js`) whose content differs between the base's runs and the change's is the change's build altering what the tests are, for example a build script that edits the compiled assertions. Aver compares only content hashes. The entry is `NEW` instead when the check fails on the change.

So are edits under another task's exclusive lease (`lease::<path>` entries, for a workspace head): the lease says another task reserved the path, and whether both changes may land there is a person's call, for any agent tool. Not `NEW`, since nothing a check measured broke, and not `BLOCKED_NOT_OWN`, since the edit is the agent's own. The agent report says not done until the edit is undone, and `--all-pass` accepts and combines skip the task.

So are edits to sensitive paths (`sensitive::<path>` entries): by default `.github/workflows/**` and `.github/actions/**`, which run with the repository's secrets and decide what CI checks. Nothing failed, but a person reviews the edit before it lands, so a change that touches one is at best `REVIEW`. The list is `[judge] sensitive` in the base's `aver.toml`: an explicit list replaces the default and `sensitive = []` turns it off. The change's own `aver.toml` cannot change it. A sensitive path no check covers is not also listed as uncovered: its entry already asks a person to look, so the verdict stays `REVIEW` instead of `UNPROVEN`.

**Zero evidence.** A check that cannot show what the change does gets an `ENV` entry `<check>::(evidence) <reason>`, so the verdict is `BLOCKED_NOT_OWN`: a side that ran no test, a base on which no test passed, pytest stopping on its own error (exit 2, 3 or 4) even with a partial report, a Go package a panic cut off before its declared tests reported, a setup command that failed, a kept directory holding a stale copy of a changed file, a base that ran fewer tests than the contract's `min_tests` (`below the floor on the base`), or a check judged by its exit code alone that no person approved (`exit code only`). The detail says what was seen, such as "The base ran 3 tests (0 passed, 3 failed) and your version ran 3". Before, each of these could pass: a suite that ran nothing compared equal on both sides.

An exit code is no evidence of any test, so it counts only when a person approved the command: it is in the base's `aver.toml`, or, in zero-config CI, it is the `test` script of the repository's own `package.json`. An exit-only check a head proposes in its own new `aver.toml` is blocked, and so is one Aver guessed (`python -m unittest discover`, `npm test` without a script); the cure is a check with a test report, or merging the `aver.toml`. Approval does not matter for a check with a per-test report: a command that runs no test is already blocked as `no tests`.

The floor holds the base, not the change. A change under which fewer of the base's tests run is not blocked below the floor: each test it lost is a `NEW` entry (`missing from head report`), so the verdict is `FAIL_OWN`, the change's own. A type check is never held to a floor: its report lists errors, so a clean run lists none.

## Verdict

First match wins:

1. Any `NEW` → `FAIL_OWN`. The change broke something.
2. Any `ENV`, or an inconclusive entry → `BLOCKED_NOT_OWN`. Something outside the change stops Aver from deciding.
3. No unproven criterion, no uncovered change, and any `CHANGED` → `REVIEW`. Nothing broke, but the change rewrote test expectations or harness that a person has to approve.
4. No unproven criterion and no uncovered change → `PASS`.
5. An unproven criterion whose only proofs are `FLAKY` (and no harness edit) → `BLOCKED_NOT_OWN`.
6. Otherwise → `UNPROVEN`.

`UNPROVEN` outranks `REVIEW`: a person is asked to review only once the agent has nothing left to prove. `REVIEW` is therefore a `PASS` with changed expectations attached.

An entry is **inconclusive** when it cannot say whether the change broke anything: a whole check, judged by exit code, failing on both sides (one red test hides everything else), or a test that was flaky on the base and failed on every head run.

A criterion is **proven** by a test tagged `[crit:<id>]` that passes on the head, fails or cannot load on the transplant, and passes in the head-tests run. Nothing is proven while a harness file changed.

## Caps

Two facts about the contract hold a verdict below `PASS` whatever the runs found. They only lower a verdict, never raise one, and the diagnostics name the one that applied in `cap`.

- **No runtime tests** (`no_runtime_tests`). No check ran the repository's tests on the original code: the only checks are type checks (their report lists errors, not tests). Nothing then ran the change, so `PASS` and `REVIEW` become `UNPROVEN`, and the report says to add a check that runs tests. An approved exit-only check counts as running them: Aver cannot count its tests, so the verdict names it as a limit (`What Aver could not check`) instead of capping it. When every check was skipped as not affected (§ Which checks run), nothing was asked of the runs and this cap does not apply.
- **A proposed contract** (`proposed_contract`). The base has no `aver.toml` and the change adds the one the checks ran, as `aver check -- <cmd>` writes it in a new repository. The change then vouches for itself through its own contract, so `PASS` becomes `REVIEW`: a person approves the contract (merging `aver.toml` on its own first makes the next verify a plain `PASS`).

`UNPROVEN` outranks `REVIEW`, so no runtime tests is weighed first.

## Uncovered changes

A check can only judge files it could exercise. A check at the root without `covers` covers every file. Each other check covers the source files of its image's language under its working directory (a `node` image covers JavaScript and TypeScript, `python` Python, `rust` Rust, `golang` Go; an image Aver does not know covers every file there), or exactly the paths its `covers` patterns in `aver.toml` name. A source file the head changed that no check covers is **uncovered**. That is any file but documentation and media (Markdown, text, images, fonts, licences, `docs/`), which are never uncovered: data and configuration (`.json`, `.yaml`, `.toml`, a `Makefile`, dotfiles) are covered by any check whose directory holds them, and source of a language Aver has no preset for (Java, C, Ruby, shell) only by a check whose image Aver does not know or whose `covers` names it. Before, only `.js`, `.ts`, `.py`, `.rs` and `.go` files could be uncovered, so a Java change beside a Node package verified "PASS: no check is affected". The report ends the list with the `covers = ["<dir>/**"]` line that would cover the files listed.

The receipt lists uncovered files in `uncovered`, the diagnostics list up to 1000, `summary.md` has a "Not covered by any check" section, and the agent report warns "changed files not covered by any check: …".

A Rust change inside code under `#[cfg(feature = "x")]` is uncovered too when no Cargo check covering the file builds feature `x`: not by default (the package's `default` set, through its `[features]` table), not through its command line (`--features`, `-F`, `--all-features`), and not through another member's dependency on the package, all read from the base's manifests. A `cfg` with `not` builds by default and is no gate; `cfg_attr` is not a gate; `any(…)` needs one of its features, `all(…)` each. A gated `mod x;` gates the module's file. clap's `make test-full` builds features that `cargo test --workspace` does not, so a regression in `Parser::add_env` (behind `env`) was a `PASS` in generic mode; it is `UNPROVEN`, and `summary.md` says "`clap_builder/src/parser/parser.rs` changes code behind feature `env` of `clap_builder`, which no check enables; add a check running `cargo test --workspace --features clap_builder/env` to aver.toml" (diagnostics `unbuilt`). Running `--all-features` as well was measured and left out: on clap's base it fails 275 of 969 `builder` tests and hangs in the REPL doc-tests, so the extra run would block the verdict rather than judge the change.

**Why UNPROVEN and not a PASS with a warning.** A `PASS` tells an agent to stop. A change that breaks a Rust build would verify as `PASS` if the only check ran `npm test` in a JavaScript package, and the agent report would say "You are done". Uncovered files are a change nothing measured, so the receipt cannot claim the change is safe. They are not a failure either, nor a blocker outside the change: the evidence simply does not cover what changed, which is what `UNPROVEN` means. So uncovered files turn `PASS` and `REVIEW` into `UNPROVEN`; `FAIL_OWN` and `BLOCKED_NOT_OWN` stand, because they already say something about the change. The cure is configuration, not another verify: a check that covers the files (or `covers` patterns on one that does). The base's `aver.toml` decides coverage, so a head cannot widen it for itself.

**Example.** The head changes `site/src/core.ts` and `crates/core/src/lib.rs`; the only check runs in a `node` image with `cwd = "site"`. The TypeScript file is covered and its tests pass. The Rust file is uncovered, so the verdict is `UNPROVEN` with `uncovered: ["crates/core/src/lib.rs"]`. With a `rust` check at the root (or generic mode, which adds one for a `Cargo.lock`), the same change can `PASS`.

Coverage is by path, not by execution: a covered file can still be one no test reaches. It catches the gap that matters most, a whole package or language no check runs. Hunk necessity, next, catches the code inside a covered file that the proving tests never need.

## Untested code in a passing change

A proving test shows that the change does what a criterion asks. It does not show that every line of the change is needed for it. A real fix with an untested function beside it, or with an extra `if (a === 1337) return 0;` in the function it fixes, would pass: the proving tests never reach the extra code.

So, when a verdict would be `PASS`, Aver reverts each changed piece of source code (a **hunk**) alone and runs only the proving tests on what is left: the tests that proved a criterion, or with no criteria the tests that show the change and the tests it fixed. A hunk whose reversion leaves every proving test passing is not needed by any of them. The verdict becomes `UNPROVEN` and the diagnostics list the hunk with reason `untested_hunk` under `necessity`. Every report says, for each such hunk:

> Reverting your change at `src/math.js:15-15` did not cause any proving test to fail; add an assertion that requires it or remove the unneeded code.

`aver ci` exits 3 on it under the default `--fail-on unexplained`. The cure is a test that fails without that code, or removing the code. `aver check` runs the same phase, so it predicts this before a verify is spent.

- **What a hunk is.** A zero-context line diff of each changed source file a check covers. Test files, the judging set and sensitive paths are not tried. A new file is one hunk. Comments and whitespace are left out. An `if` added beside an edited line, and each conditional block of a rewritten body, is its own hunk, so an extra branch in the very function the fix touches is still found. Lines are the head's.
- **Only lower.** The phase runs only on a would-be `PASS` and only turns it into `UNPROVEN`. Anything uncertain counts as needed: a run that lost its result, a proving test missing from the report, or any failure. A reversion that does not build or load is tried again with the hunks that share its new names (a function and its call), and the group is untested only when the proving tests pass without all of it.
- **When it does not run.** With no proving test, a proving test whose check cannot run tests by name (an exit-only check), or more than 8 hunks. The verdict stands, and the `What this verdict covers` section says `Necessity: not checked for N changed code parts, because …`.
- **Cost.** One run of the proving tests per hunk, selected by name, cached by tree and command. On fixtures/calc's b13 heads it adds 0.45 to 0.8 s. On real changes it adds 0.5 s (tqdm), 0.8 s (chi), 1.4 s (Aver's site) and 23 s (anyhow, where three reversions do not compile and are retried together).

## Why REVIEW exists

A test that the change updated looks exactly like a test the change weakened. Both show the same evidence: the old version fails on the new code and the new version passes. Only someone who knows what the change was for can tell them apart. So Aver does not guess. It asks a person to review the intent of the changed expectations, not every line of the change, and shows exactly those: each `CHANGED` entry and the diff of its test file, in the "Needs review: test changes" section of `summary.md`.

This follows the review practice described in Anthropic's engineering post [Redesigning source control](https://www.anthropic.com/engineering/redesigning-source-control): reviewers check what a change means to do rather than reading every line. Aver is not affiliated with that post; it applies the same idea to test expectations.

The freeze still holds. Regressions are judged with the base's tests and harness, so a weakened test cannot turn a regression into `PASS`: at best it turns `FAIL_OWN` into `REVIEW`, and the weakening is what the reviewer sees. That holds for the files Aver knows to be tests or harness (the patterns above, and the tests inside Rust source files); test code anywhere else (a helper module tests import under another name) still runs as the head wrote it. A project with such files names them in `tests` in `aver.toml`.

The agent report shows the same: each `CHANGED` test or harness file with its before and after lines, under "Changed by you, for a human to review", and a test listed there is never also listed as evidence that the change works. Its next step says what the person reviewing will look at, not "nothing".

**Only a person approves a REVIEW (B5).** The agent report's first and last lines say `NEEDS HUMAN REVIEW: REVIEW (a human must run 'aver accept --approve-review' or approve the PR)`, never `DONE: REVIEW`, and carry no `Hand-off: aver accept` line. `aver verify` still exits 0 on it: the agent's work is finished and it waits for a person, so the Stop hook and `aver run` let the agent stop there. `aver accept` (solo, a run's attempt, a batch task) refuses a `REVIEW` with exit 2 unless `--approve-review` is passed, and refuses `--approve-review` itself with exit 2 ("a human must approve a REVIEW") when the caller is an agent: when `AVER_RUN` or `AVER_WORKSPACE` (set by `aver run` for its agents), `CLAUDECODE` (Claude Code) or `CODEX_SANDBOX` (Codex) is set and not empty, and when stdin is not a terminal (a person approves from an interactive terminal; `AVER_TEST_HUMAN=1` stands in for one in tests and scripts, a test hook rather than a security boundary, and never under the agent variables). `aver combine --approve-review` and `aver rerun --approve-review` refuse an agent the same way. `aver accept --all-pass` accepts only `PASS`.

An approval is bound to one receipt. `--approve-review` approves the `REVIEW` receipt the accept verifies for the directory as it is: an edit after that receipt makes the accept verify again, and the new receipt needs the approval again. The accept journal records the approved receipt with its snapshot, and the commit carries it as a trailer, `Aver-Approved-Review: <receipt>`. An accept run again after a crash finishes with the journaled approval only while the directory is still that snapshot; otherwise it refuses until a person approves the new receipt.

A change can say why it rewrites tests: `aver verify --reason "<why>"`, or an `Aver-Why: <why>` trailer on the head commit in `aver ci`. The reason is kept in the unsigned diagnostics (`reason`) and shown fenced, as data, above the diffs a reviewer reads, under `Stated reason (from the agent, not checked):`. Aver does not check it, and it never changes a verdict.

**In CI, the approval is the pull request review, bound to the head commit.** With the action's `review: approve`, a `REVIEW` or `UNPROVEN` posts the required check run `Aver` as `action_required`. A job on `pull_request_review` with `mode: approve` runs `aver ci approve`: an approval binds when it is the reviewer's latest decision, it was given on the pull request's current head commit, and the reviewer is a person other than the author. It then writes `approval.json` with the head commit and its tree and posts `Aver` as `success` on that commit. A push after the approval is a new head commit, verified again, and the old approval does not carry over ([CI.md](CI.md#approving-a-review-in-ci)).

## Worked examples

**NEW.** The head changes `add` to `return a - b`. `add returns the sum` passes on base and fails three times on head. Cause `NEW`, verdict `FAIL_OWN`. If the head also edited that test and its own version fails too, the test stays `NEW`, and `summary.md` says "The PR's own version of this test fails too."

**PREEXISTING.** `parses the config` fails on base and on head. It is not the change's failure and does not block a per-test receipt.

**FLAKY.** A test fails once and passes once on the head, and the base, rerun to three results, fails it once too. It is not counted against the change. It blocks only a criterion whose every proof is flaky. If the base passes it three times, the head's failure is the change's: `NEW`.

**ENV.** A head run is killed from outside (the out-of-memory killer) and its retry fails the same way, so the side ended in infrastructure and the cause is `ENV`: verdict `BLOCKED_NOT_OWN`. A tool the check needs that is missing on the machine gives `ENV` the same way, naming the tool. So does a report over Aver's 64 MiB limit: Aver drops it, the run has no result, and the detail says so. A report the runner wrote malformed, a runner that crashed, and a head that timed out over a base that finished stay `NEW`: those results were the change's to lose, not Aver's.

**CHANGED.** The head makes `sub(a, b)` return `b - a` on purpose and changes the test from `sub(5, 3) == 2` to `sub(3, 5) == 2`. The base's copy fails on the head; the head's version passes. Cause `CHANGED`, verdict `REVIEW`, and the summary shows the one-line test diff. A head that instead breaks `add` and marks its test `it.skip` (or deletes it) does not: its own version is gone, so the test stays `NEW` and the verdict is `FAIL_OWN`.

**CHANGED on a red base (c2).** On the calc fixture `divide` throws "not implemented", so both divide tests fail on the base. The head makes `divide` return `a / b` and rewrites the zero-division test from `assert.throws(() => divide(1, 0), /division by zero/)` to `assert.equal(divide(1, 0), Infinity)`. With the base's copies, `div-basic` passes on the head (it is "Fixed") and the zero-division test still fails, since `a / b` does not throw. The head's own version of that test passes. The test was rewritten, not fixed: cause `CHANGED` (not `PREEXISTING`), verdict `REVIEW`, and the summary says "only the PR's rewritten version passes, so it was rewritten, not fixed" above the one-line diff. Left alone, the test stays `PREEXISTING` and the verdict is `PASS`.

**The inconclusive rule.** The project's `npm test` fails on both sides, judged by its exit code (one test needs the network). The check is one command-level entry, `PREEXISTING`, and inconclusive, so the verdict is `BLOCKED_NOT_OWN`, not `PASS`: nothing measured the change.

**A base that ran no test (review fix H1).** A Go older than `go.mod`'s `go` line (under `GOTOOLCHAIN=local` every run ends with `go: go.mod requires go >= 1.22`), a Rust older than `Cargo.toml`'s `rust-version`, a `package.json` with no `test` script, or any runner that fails before its first test: the base's runs failed and either their output names a toolchain error Aver knows, or the check declares a report and no base run wrote one with a test in it. Such a base explains nothing, so the entry that would be `PREEXISTING` (or `FLAKY`) is `ENV` (a `ZeroTests` base in the cause table), never explained by the base: `aver ci` exits 1 instead of passing a real regression over a base that never ran. An `(exit)` entry over base reports with no test in them is `ENV` the same way. The diagnostics note, `summary.md` and the agent report name the cause from the run log, with its fix ("the toolchain could not run the tests on the original code … (fix: install the Go version go.mod asks for, or lower its go line)"), the run log's line fenced; before, they blamed the missing per-test report and told the agent "not because of you … stop". `aver doctor`, and `aver ci` on stderr before it runs, compare the root `go.mod` (`go` and `toolchain` lines) and `Cargo.toml` (`rust-version`) with the `go` and `rustc` here and say when they are too old.

**The transplant proof.** The criterion `div-zero` is proven by `[crit:div-zero] divide throws on zero`, a new test. On the transplant (the base with the new test file) it fails because `divide` is not implemented; on the head and in the head-tests run it passes. Had it passed on the transplant, the criterion would be unproven with reason `passes_without_change`.

**A weak proof.** The criterion `neg` asks that `negate(x)` return `-x`. The change adds `negate` returning `x` (wrong), and its only test, `[crit:neg] negate exists`, asserts `typeof m.negate === 'function'`. On the transplant `negate` is undefined, so the test fails there and passes on the head, which used to prove the criterion. But that failure only says that a name is missing on the original code (`is not a function`, `is not defined`, `'undefined' !== 'function'`, Python's `NameError` or `AttributeError`, Go's `undefined: x`), and a module the test imports being missing (`Cannot find module`, a test file that cannot load at all) says no more: any test that merely names the new code fails there the same way. **Weak proofs alone never prove**: a criterion whose every passing proof is weak is unproven (reason `weak_proof`), so the verdict is `UNPROVEN`, not `PASS`, and every report says why and what a real test needs: one that loads on the original code and fails there on an assertion about what the criterion asks, such as `negate(2) === -2` reaching a result the original code gets wrong. The diagnostics list it under `weak_proofs` (`missing_module` or `missing_name`), and `summary.md` and `aver accept` show the proving test's source, so a person can see what it checks.

**A proof by build failure.** In Go or Rust a test of a function the change adds does not compile without it: on the transplant, `go test` reports the package as not built and `cargo test` runs no test at all. That is a weak proof too (`build_error`), whether or not every error is a name the original code does not define (`new_api`): a test that only mentions the new function fails to build the same way, so the criterion is unproven and a person reviews it, or a test that builds on the original code and fails there on an assertion proves it. When the original code itself does not build, nothing can prove: the proving entries are `ENV`, the verdict is `BLOCKED_NOT_OWN`, and every report says "The original code doesn't build for check <id>" with the first error.

## Beyond the verdict: what the change shows

A verdict says whether the change broke something and whether each criterion is proven. Agents in the A/B evaluation (bench/eval) asked for three more things, which the reports now give without changing a verdict or a signed field. They are unsigned diagnostics (`aver.diagnostics/v0`, ARCHITECTURE.md §1.9.4), shown by `aver verify`, `aver check`, the Stop hook and `summary.md`:

| Report line | What it means | Source |
|---|---|---|
| Your tests show the change | Tests in test files the head added or changed fail (or do not load) on the original code and pass with the change. With no criteria, this is the evidence that a fix works. A Rust test fn added to an existing file counts, in a crate below the root too, and so does a new test whose transplant did not build (cargo then runs no test at all), as "does not load or run". | transplant runs; verify runs them whenever the head changed a test file, also without criteria |
| Fixed | Tests that failed on every base run pass on the head. | receipt entries: base `fail`, head `pass`, no cause |
| Still failing | `PREEXISTING` tests. Not counted against the change, but if the task is to fix failing tests, these are the target. | receipt entries |
| Nothing shows the change works yet | No criteria, neither of the first two, and the change touches source code (a change to documentation or data alone, or one no check is affected by, gets no such line). | |
| NOT PROVEN (weak proof): … only because a module / a name they use is missing there | A weak proof, above: the criterion is unproven. | `weak_proofs`, `unproven[].reason` |
| NOT PROVEN (weak proof): … do not build on the original code | A proof by build failure, above. | `weak_proofs` (`build_error`) |
| Changes no proving test needs | Untested code in a passing change, above: the verdict is `UNPROVEN`. | `necessity` (`untested_hunk`) |
| Proving tests | The source of each criterion's first proving test, in `summary.md` and `aver accept`. | `proof_sources` |
| The original code doesn't build for check … | Nothing can prove over that base, above. | `failures[].note` (`ENV`) |

A directory that changed nothing is never reported as done: `aver verify` prints `NOT DONE: PASS, but you changed nothing` and exits 1, and `aver check` ends with a warning instead of "Verify should PASS". The receipt itself is still `PASS`, because nothing that passed on the base fails.

**Tasks about failing tests.** "Make CI green" is a task whose target is the `PREEXISTING` failures. Aver does not turn them into an automatic criterion, and `aver run` has no `--fix-failing` flag. A criterion "the tests failing on the base must pass" needs either a new proof rule, which changes the verdict rules and the receipt (a v3 receipt for the hosted consumer), or a task check that runs the suite and `proves` it; such a check fails on both sides until the task is done, and a command-level entry failing on both sides is inconclusive, so the verdict would read `BLOCKED_NOT_OWN` ("not because of you; stop") exactly while the agent should keep working. Instead the brief tells the agent that for such a task the `PREEXISTING` failures are the target, the reports list them as "Still failing" and the ones it fixed as "Fixed", and an unchanged directory is not done. A task author who wants a gate names the tests in a criterion, and the agent proves it with a tagged test that fails on the original code.

**The default type check.** A base without `aver.toml` whose `package.json` lists `typescript` in `devDependencies` beside a root `tsconfig.json` gets a second default check, `typecheck` (its `typecheck` script, else `npx tsc --noEmit -p .`). Its output is read with the `tsc` report format: each type error is its own entry, keyed by file, error code and message, not by line, so an error that only moved keeps its entry. A type error the change adds is `NEW`, one the change fixes is "Fixed", and errors the base already has are `PREEXISTING` test by test, so they do not block. An `aver.toml` gets the same with `report = { format = "tsc", path = "/aver/out/tsc.txt" }` on a check that writes `tsc`'s output there.

## Which checks run

A verify runs only the checks the change can affect. A check whose files the change does not touch would report on the head exactly what it reports on the base, so verification runs it on neither side and lists it in the receipt's `skipped` with the reason `not_affected`. A change that only edits documentation runs no check at all and verifies `PASS` in seconds; the report says "No check is affected by this change".

A changed file (added, modified or deleted) affects a check when it is any of these:

| The changed file is | Affects |
|---|---|
| `aver.toml` | every check |
| a lockfile of a dependency layer | every check that mounts that layer |
| harness (`package.json`, runner and package manager configuration, `Cargo.toml`, `build.rs`, `go.mod`, `pyproject.toml`) in a directory on the way to a check's `cwd` | that check (root harness: every check) |
| harness below a check's `cwd`, of that check's ecosystem (a crate's `Cargo.toml` for a `rust` check at the root) | that check |
| in a check's scope: a path its `covers` patterns name, or, without `covers`, any file under its `cwd` | that check |

Task checks always run. Every check runs when the change is empty, because that report describes the original code, and with `aver verify --all` (also `aver ci --all`, `aver check --all-checks`, and `all` in the HTTP `verify`).

**Soundness.** The rule is conservative where Aver cannot know better, and trusts `covers` where the configuration says what a check reads:

- A check without `covers` at the repository root, as in a single-package repository or the default environment, is affected by every change. So nothing changes for such repositories.
- Any file under a check's `cwd` counts, not only source files: tests read fixtures and data.
- A harness file affects every check it judges. A root `package.json` affects every check.
- A check whose tests read files outside its `cwd` must name them in `covers`. This repository's `aver.toml` does: the `rust` check covers `fixtures/`, `schemas/` and the Claude Code plugin, which its tests and binary read, and the `site` check covers `schemas/`, whose test vectors its tests read. A `covers` that leaves out such a file can skip a check that would have failed; the base's `aver.toml` decides, so a head cannot narrow it for itself.
- A skipped check never holds a verdict. With no entry, it cannot be `NEW`, inconclusive or a proof. A criterion whose only tests are in a skipped check is `not_claimed`, so the verdict is `UNPROVEN`, never `PASS`.
- Selection does not change the uncovered rule: a changed source file that no check covers is still listed, and a covered one always makes its check run.

**Example.** In this repository, a one-line change to `docs/RELEASING.md` skips all four checks. A change to `crates/aver/src/judge/run.rs` runs only `rust`. A change to `site/src/core.ts` runs `site` and `site-typecheck`, and not the Rust build.

## Where the checks ran

A receipt (`aver.receipt/v3`) says where its runs executed, in `provenance`, and how each check was read, in `levels`. The verdict rules above are the same whatever the provenance; what a reader may conclude from the receipt is not.

| `provenance.mode` | Where | What the receipt trusts |
|---|---|---|
| `local` | Natively on the verifier's machine (`aver verify`, the default): slots cloned from the agent's directory and swapped to each tree, the caller's environment minus CI tokens, the base's harness. | The machine and the agent. Test code ran as the verifier's user and could read what that user can; nothing stopped an agent from tampering with the host. Right for an attended agent that already has your shell. |
| `ci` | Natively on a CI runner (`aver ci`, the default), the base before the head. | The runner, as the repository's own CI already does. With `uid: "separate"` the code under test ran as another user and could not read the verifier key. |
| `sandbox` | In the Docker sandbox (`--sandbox`): no network, no host files, no secrets. Only older receipts have it; Aver no longer runs a sandbox. | The sandbox. |

- `host_profile` (native modes only) hashes the operating system, the CPU and every toolchain binary with its version, so a receipt says which toolchain ran it; runs from another toolchain are never reused. `executor` is then `native-v0.` and the profile's first 16 hex digits.
- `network` is `off` when the runs could not reach the network (where the system can turn it off: `sandbox-exec` on macOS, a network namespace on Linux) and `on` otherwise. Natively, off means no external network: loopback stays up, so a test that serves and connects on 127.0.0.1 runs (under `aver ci --run-as` on Linux it is down). A test that fails on both sides because it reaches for the network is `PREEXISTING` with a note saying so, and the reports name it. A check that passes only with the network is not reproducible.
- `uid` is `separate` when the checks ran as a user other than the verifier's (`aver ci --run-as`, or the Docker sandbox in older receipts), `same` otherwise.
- `levels` gives each check that ran (not `skipped`) as `native` (a parser Aver has for the runner: libtest, go test -json, Jest, Mocha, pytest, tsc), `junit` (JUnit XML) or `exit` (the exit code alone, one entry for the whole check, so one failing test on the base blocks the verdict: give it a JUnit reporter).

A tool missing on the machine, or reachable only through a version manager's shim, ends that check's runs as infrastructure (`ENV`), naming the tool.

## What this verdict covers

Every report states the limits of its verdict, from what the receipt, its diagnostics and the verify's timing already record (`aver_core::scope::Scope`, ARCHITECTURE.md §1.9.5). `aver verify` prints it just above the footer, `aver ci` as a section of `summary.md`, and `aver accept` puts the `Ran`, `Not covered` and `Where` lines in the commit message. Its JSON form (`Scope::to_json`, pinned by `schemas/vectors/scope.json`) holds the data, the rendered `lines` and `names`, `limits` and `compact`, so every client prints the same words.

```
What this verdict covers:
  Ran 3 of 5 checks: 2 per test, 1 by exit code only.
  Skipped 2 checks your change cannot affect.
  Not covered: 2 changed files no check runs.
  Weak proof: criterion neg fails without your change only because a module is missing.
  Where: natively on this machine, as your user, network on; 4 of 9 runs reused from the cache.
```

followed by the repository names, fenced (at most three per line, then `+N more`): `ran:` (a check read by its exit code alone marked `(exit code only)`), `skipped:` and `not covered:`. `Ran` and `Where` always appear; the other lines only with content. When nothing limits the verdict (every check read per test, nothing skipped or uncovered, no weak proof) the section is two lines and names nothing: `What this verdict covers: Ran 2 of 2 checks, all per test.` and its `Where:` line. `Where` reads `natively, by \`aver ci\`, as a separate user, network on` (`aver ci` on any machine, not only in a CI job), as every run since the slot runner does (an older native receipt may read `network off (best effort)`); a receipt the scripted test executor wrote, or an older one from the Docker sandbox, reads `in a non-native executor (the scripted test executor; before Aver 0.4, the Docker sandbox), as a separate user, network off`, and one before v3 has no provenance and reads the same without the user. When a per-test Cargo check ran its transplant or its head-tests run over only the test targets the change makes (ARCHITECTURE.md §2.5.1, step 5a), `Where` ends with what each did: `; the transplant ran 2 of 14 test targets, the rest read from the original code's run; your tests' run ran 1 of 14 test targets, the rest read from your code's run`. It is not a limit: the receipt is what the full runs give. A weak proof's line names its reason: `only because a module is missing`, `only because a name its tests use is missing there`, or `only because its tests do not build there`.

`summary.md`'s section adds two lines, what the run proved and what a person should look at because no check could, and leaves out the `Not covered` line, which the second one counts:

```
- Proven: 1,204 base tests frozen (each judged with the base's own copy), 2 new tests failed on the base and passed on the head.
- What Aver could not check: 1 changed file no check covers; 1 check judged by its exit code alone; 1 sensitive path modified (`.github/workflows/ci.yml`); a person should look at these.
```

Base tests frozen are the test entries the base ran (a check's exit code is not a test); new tests are the head's added or changed tests that fail on the original code and pass with the change (the diagnostics' `evidence.shows_change`). With nothing to flag, the second line reads `What Aver could not check: nothing it can name; it knows only what the checks ran.`

A done verdict's last line gains a suffix only for material limits: a check read by its exit code alone, a weak proof (a new API's, whose head-tests run checked the assertions, is not one), network on, or runs reused from the cache that were cached before the running `aver` was installed (the run cache records no verifier version, so their age stands in for it): `DONE: PASS (2 of 10 verifies used); limits: 1 check by exit code, network on`. Uncovered files are not a suffix: they already make the verdict `UNPROVEN`.

## Exit codes and ranking

| Mode | `PASS` | `FAIL_OWN` | `BLOCKED_NOT_OWN` | `REVIEW`, `UNPROVEN` |
|---|---|---|---|---|
| `unexplained` (default) | 0 | 1 | 1, or 0 when the base explains it | 3 (needs a maintainer) |
| `regressions` (`--regression-only`, merge queues) | 0 | 1 | 1, or 0 when the base explains it | 0 |
| `fail-own` | 0 | 1 | 0 | 0 |
| `not-pass` | 0 | 1 | 1 | 1 |
| `never` | 0 | 0 | 0 | 0 |

Any error exits 2.

Exit 3 (review fix H2) means nothing broke but nothing proved the change safe either: `REVIEW` (a changed test expectation or harness a person has to approve) or `UNPROVEN` (an unproven criterion, or a changed source file no check covers). Before, the default exited 0 on both, so a test the change rewrote, or a change to a file no check runs, was green CI. The GitHub Action fails the required check on exit 3, titled "Aver: needs maintainer review (REVIEW)" or "(UNPROVEN)", unless its input `review: neutral` keeps the job green and posts a neutral check run of that title, or `review: approve` keeps it green and posts the check run `Aver` as `action_required`, which a maintainer clears by approving the pull request on that head commit (below).

`aver ci --fail-on unexplained` (the default) exits 1 on `FAIL_OWN`, and on a `BLOCKED_NOT_OWN` that the base does not explain. The base explains a block only when every entry that blocks (each `ENV` or `FLAKY` entry and each inconclusive one) fails or is flaky on the base and none of them is `ENV` (`ci::blocked_by_the_base`, which the exit code, `summary.md`'s last line and the Action all read). A red base (a test that already fails there, a test flaky there) still passes the check, so a repository with known failing tests is not punished for them. A check read by its exit code alone (an entry with no test name) that fails on the base does not: no test result says what the base ran, so it cannot tell a broken main from a runner that cannot run the suite. Before, ky's `npm test` (exit code only: `xo`, `ava`, `tsc`) failed on both sides for want of Playwright's browsers, and its regression exited 0. An `ENV` entry is never explained by the base, so any block that rests on infrastructure fails the check:

- The head's runs ended in infrastructure (after the retry) and the base completed. Whatever the base's tests did, they did not stop the head's runs, and nothing showed the change is safe. It stays `ENV`, not `NEW`: an infrastructure failure says nothing about what the code does, and Aver's own infrastructure has caused them (below), so calling it the change's failure would send an agent hunting for a regression that may not exist. The exit code is what must not read "not yours".
- The base's own runs ended in infrastructure (a tool missing on the machine, a dependency that cannot install). No test ran on the base, so it explains nothing.

- A pytest test module that does not import on the base (B15). pytest's `--continue-on-collection-errors` reports each such module as one failing test case named after the module, with the message `collection failure` (`pytest::tests/test_format.py > tests.test_format`). None of the module's tests ran on that side, so its failure there is no evidence: the entry is `ENV` (a `ZeroTests` base in the cause table), never `PREEXISTING`, and the diagnostics note says the module failed to import on the base. Before, black's regression, gaming-hide and gaming-rewrite patches were `PASS`: every test module failed to import on both sides (a generated `_black_version.py` the checkout lacked), each collection error was `PREEXISTING`, and the 19 tests that did import passed. This holds for a module related to the change: one the head changed, one below a changed `conftest.py`, one the base tree lacks, or one whose base source imports the repository's own Python code (a top-level name of a `.py` file below the check's directory and `src/`, or a relative import) when the change touched any of that code: the package it imports may import the changed one. A module that imports none of the repository's code (a documentation test needing a tool the job did not install) failed for a reason the change cannot touch, so it stays `PREEXISTING` and the verdict rests on the modules that ran, under `min_tests`. A module that imports on the base and not on the head is still the change's: its entry and its tests are `NEW`.
- A command the checks need is not installed. A shell's `command not found` (bash, zsh, dash, a script's own `line N: x: command not found`) or `env: x: No such file or directory` in a base run that reported no test is a toolchain refusal like a Go older than `go.mod`, so the entry is `ENV` whether the check is per-test or judged by its exit code, and a setup command that fails the same way names the command in its `(evidence) setup failed` entry. The report gives the fix: the dependencies are not installed, so install them (`npm install`, `pnpm install`, `pip install`, ...) before Aver runs, or put the command on `PATH`. A runner that reported tests is not a missing command, whatever its output says, and a tool missing only on the head stays the change's own failure (`FAIL_OWN`, exit 1).

The verdict is still `BLOCKED_NOT_OWN`: the rule changes the exit code, not the receipt, and `--fail-on fail-own` still exits 0 on it.

**Example (CI on 856ab5a).** Two parallel `aver ci` processes on one runner verified calc commits with the same lockfile, so they shared the dependency layer volume. The second found it incomplete while the first was installing it and deleted it under the first's runs, which ended as sandbox errors (`volume is in use`) after their retry. The regression (`add` returns `a - b`) read `BLOCKED_NOT_OWN` with exit 0, since the calc base's divide tests fail. Layer builds then took a lock across processes, and the same block exits 1. `--fail-on fail-own` exits 1 only on `FAIL_OWN`. A lost report or a head timeout over a red base gives `FAIL_OWN`, and the default fails any other `BLOCKED_NOT_OWN` the base does not explain. `--fail-on not-pass` exits 1 on everything but `PASS`, `REVIEW` included, and `fail-own` exits 0 on `REVIEW` and `UNPROVEN`.

`aver run` ranks parallel attempts `PASS` > `UNPROVEN` > `REVIEW` > `BLOCKED_NOT_OWN` > `FAIL_OWN` > no receipt > no change, then fewer `CHANGED` entries, fewer unproven criteria, fewer `FLAKY` entries, not given up, fewer changed lines, shorter wall time. An attempt that left the existing tests alone beats one that changed what they expect, however small its diff: without tagged tests, a test-gaming attempt is often the smallest. An agent whose latest verdict is `REVIEW` is finished and waits for a person; `aver accept <n> --approve-review` takes a `REVIEW` winner (not `--force`), and only a person may pass it.
