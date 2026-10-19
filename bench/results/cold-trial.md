# Cold trial: Aver 0.4.1 on four projects it has never seen

2026-10-19, macOS (APFS), `aver 0.4.1` from `~/.local/bin`, Python 3.11.2 with pytest 9.0.3, Node 25.8.1, Go 1.21.5, Cargo 1.99.1. Each project was cloned read-only (`git clone --depth 1`) into a scratch directory and given its own throwaway `AVER_HOME`. None of them is in `bench/replay/manifest.json` or the Rust compat tests.

| Project | Language and runner | Commit | Tests on HEAD |
|---|---|---|---|
| [python-humanize/humanize](https://github.com/python-humanize/humanize) | Python, pytest, src layout, hatch-vcs | 785e5dc | 746 passed, 110 skipped |
| [vercel/ms](https://github.com/vercel/ms) | TypeScript, jest via pnpm | 4ff48ce | 167 passed |
| [Masterminds/semver](https://github.com/Masterminds/semver) | Go, `go test` | dd2b995 | 92 passed |
| [chronotope/humantime](https://github.com/chronotope/humantime) | Rust, inline `#[cfg(test)]` tests, no `Cargo.lock` | 997ff56 | 43 passed |

For each project the steps were: install the dependencies the way a contributor would, run `aver init`, run `aver workspace new -c "<id>: <text>" --count 3`, and then apply three patches:

- **fix**: a genuine small change plus a test tagged `crit_<id>` or `[crit:<id>]`.
- **regression**: the criterion is still met, but an existing behaviour breaks.
- **gaming**: the regression plus a weakened test: a `skip`, or a deleted assertion.

Each patch then got `aver check` followed by `aver verify`. Times are wall clock. "Home" is `du -sh $AVER_HOME` at the end, an apparent size: APFS clones share blocks, so `aver doctor`'s slot figure, which counts real blocks, is given beside it. Raw outputs are in the session scratchpad (`cold/out/*.check|verify`), not committed.

## Results

| Project | Patch | Config | Expected | `aver check` | `aver verify` | check time | verify time |
|---|---|---|---|---|---|---|---|
| humanize | fix (`ordinal(-1)` gave `-1th`) | `aver.toml` as `aver check -- pytest` wrote it, uncommitted | PASS (or REVIEW for the proposed config) | **FAIL_OWN** | **FAIL_OWN** | 24.1 s | 0.3 s |
| humanize | regression only, no criteria | the same `aver.toml`, committed to the base | FAIL_OWN | **PASS** | **DONE: PASS** | 27.9 s | 0.4 s |
| humanize | fix | hand-fixed `aver.toml` in the base (`PYTHONPATH=src`, `prepare` writes `_version.py`, `min_tests` lowered) | PASS | PASS | PASS | 8.8 s | 0.3 s |
| humanize | regression (11, 12 and 13 → "st", "nd", "rd") | hand-fixed | FAIL_OWN | FAIL_OWN (4 NEW) | FAIL_OWN | 10.9 s | 0.3 s |
| humanize | gaming (`@pytest.mark.skip` on `test_ordinal`) | hand-fixed | FAIL_OWN or REVIEW | FAIL_OWN (4 NEW, 14 CHANGED) | FAIL_OWN | 1.1 s | 0.3 s |
| ms | fix (`parse('+5m')` gave `NaN`) | hand-written `aver.toml` in the base (jest `--json`) | PASS | PASS | PASS | 14.6 s | 0.3 s |
| ms | regression (negative numbers stop parsing) | hand-written | FAIL_OWN | FAIL_OWN (18 NEW) | FAIL_OWN | 36.6 s | 0.7 s |
| ms | gaming (`it.skip` on 3 negative tests) | hand-written | FAIL_OWN or REVIEW | FAIL_OWN (18 NEW) | FAIL_OWN | 35.1 s | 1.0 s |
| semver | fix (detailed error for `1.2.3-` and `1.2.3+`) | `aver check -- go test ./...`, uncommitted | REVIEW (proposed config) | REVIEW | REVIEW | 2.5 s | 0.3 s |
| semver | fix | the same file, committed to the base | PASS | PASS | PASS | 7.0 s | 0.3 s |
| semver | regression (every prerelease rejected) | the same file in the workspace | FAIL_OWN | FAIL_OWN (77 NEW) | FAIL_OWN | 8.7 s | 0.4 s |
| semver | gaming (`t.Skip` in `TestNewVersion`) | the same file in the workspace | FAIL_OWN or REVIEW | FAIL_OWN (77 NEW, 1 CHANGED) | FAIL_OWN | 3.1 s | 0.3 s |
| humantime | fix (`nsecs`, `usecs`, `micros`, `msecs` units) | `aver check -- cargo test`, uncommitted | REVIEW (proposed config) | REVIEW | REVIEW | 63.4 s | 0.3 s |
| humantime | fix | the same file, committed to the base | PASS | PASS | PASS | 82.8 s | 0.3 s |
| humantime | regression (`m` no longer means minutes) | committed | FAIL_OWN | FAIL_OWN (7 NEW) | FAIL_OWN | 125.0 s | 0.3 s |
| humantime | gaming (the `"100m"` assertion deleted from `test_units`) | committed | FAIL_OWN or REVIEW | FAIL_OWN (6 NEW, `test_units` CHANGED) | FAIL_OWN | 46.9 s | 0.5 s |

Disk at the end:

| Home | `du` | Slots per `aver doctor` |
|---|---|---|
| py | 261 MB | 0.1 GiB in 11 slots |
| js | 1.6 GB | 1.0 GiB in 7 slots, each with a 165 MB `node_modules` |
| go | 5 MB | Go's build cache lives outside the home |
| rs | 548 MB | 0.5 GiB in 7 slots |

Workspace creation took 19–425 ms per directory each time.

Summary:

- Once `aver.toml` was right, every verdict was right: all 14 rows other than the first two humanize rows. A proposed config gave REVIEW, which is the designed answer. Every skip and every weakened assertion was caught, either as NEW (the base's version of the test was run) or as CHANGED.
- Getting to a right `aver.toml` took hand editing in two of the four projects.
- On humanize, the `aver.toml` that Aver writes itself gave a wrong verdict in both directions: a genuine fix was FAIL_OWN and a regression was PASS.
- `aver verify` right after `aver check` was always cached (0.3–1.0 s), so the verify cost is the check.

## Newcomer run

`aver` with no arguments, outside a repository (0.01 s):

```
Not inside a Git repository.
Next: `aver init` in a repository sets Aver up there; `aver demo` shows Aver catch a gamed test in a throwaway repository.
```

`aver demo` passed all four steps, exit 0, in 1.3 s. The help text says it takes "about 30 seconds". Two snags:

- Step 1 prints `Verify would stop at UNPROVEN: prove sub (Criteria above)`, but the demo never shows any "Criteria above".
- `aver --help` describes `init` as "Create the server key and the admin key and register them", which means nothing to a newcomer. `doctor` still lists "Docker", which 0.4 removed.

## Surprises, with the exact output

### S1. An editable install makes every side test the user's checkout, so verdicts come out wrong in both directions (humanize)

`pip install -e .` (here through `uv pip install -e '.[tests]'`) is the standard development setup for a src-layout Python project. It writes `.venv/lib/python3.11/site-packages/_editable_impl_humanize.pth`, which contains the absolute path of the user's checkout:

```
/…/scratchpad/cold/py/humanize/src
```

`[deps] keep = [".venv"]` clones that `.venv` into every slot. `import humanize` then loads the original checkout in the base slot, in the head slot, and in the transplant. The check that `aver check -- pytest` writes is `python -m pytest …` with no `PYTHONPATH`.

On the genuine fix, the agent's tagged tests fail "on your code" because the agent's code is never imported:

```
Verify would stop at FAIL_OWN. 0 of 1 criteria proven, 4 new failures are yours.
…
  neg  NOT PROVEN (passes_without_change): your tagged test also passes on the original code, so it proves nothing.
…
  >       assert humanize.ordinal(test_input) == expected
  E       AssertionError: assert '-1th' == '-1st'
```

On a pure regression (`digit = value % 10`, so 11 becomes "11st"), with that `aver.toml` committed to the base:

```
DONE: PASS. No criteria (only regressions are checked), no new failures.
Base: 746 passed, 0 failed | Yours: 746 passed, 0 failed
```

From inside the workspace directory, `.venv/bin/python -c 'import humanize; print(humanize.__file__)'` prints the original checkout's `src/humanize/__init__.py`. This is the worst finding of the trial: a signed PASS on a change that breaks four existing tests. Before the config is committed, the cap on a proposed `aver.toml` turns it into REVIEW, which hides the problem.

### S2. The `min_tests` that `aver check --` writes counts skipped tests, so the floor cannot be reached (humanize)

```
Wrote aver.toml: test (min_tests 770). Commit it with your change; `aver verify` now runs it.
```

90% of 856 is 770, but 856 counts the 110 skipped tests. The base passes 746, so every later verify reports an infrastructure blocker:

```
Infrastructure problems, not yours (1; …):
test::(evidence) below the floor on the base
```

Go (82 for 92 tests) and Rust (38 for 43) have no skipped tests, so they were not affected.

### S3. Gitignored generated files are missing from slots, and `aver init` does not say why the base is red (humanize)

hatch-vcs writes `src/humanize/_version.py` at install time. The file is gitignored, so it is not in any slot. With `PYTHONPATH=src`, which `aver init`'s own draft uses, every module fails to collect:

```
E   ModuleNotFoundError: No module named 'humanize._version'
```

`aver init`'s dry run reports only this:

```
pytest                   fail, exit 1, 0 passed, 6 failed (0 s)
! red base: pytest: 6 test(s) fail on HEAD: pytest::tests/test_benchmarks.py > tests.test_benchmarks, pytest::tests/test_filesize.py > tests.test_filesize, … verify judges each change against this failing base, fix it or leave the check out
```

It names six modules and gives no reason. The cause only shows later, in `aver check`'s 15-line tail. The workaround was `prepare = "printf '__version__ = \"0\"\n' > src/humanize/_version.py"`. Projects that use setuptools-scm or hatch-vcs are common.

### S4. `aver check -- npx jest` is refused, though the refusal recommends `npx jest` (ms, pnpm)

```
aver.toml not written: `npx jest` gives no per-test report, so Aver could judge it only by its exit code. To get per-test results, run a test runner Aver adds a reporter to (`cargo test`, `go test`, `pytest`, `node --test`, `npx vitest run`, `npx jest`, `npx mocha`), …
```

The cause, from reading the source without changing it: `adhoc::check_for` calls `runner::reporter(argv, lockfile, ID)` with the npm lockfile only. With no `package-lock.json`, `Lockfile::has("jest")` is false. The `package.json` fallback (`with_manifest`) applies only on the `npm test` path. So in a pnpm, Yarn or Bun repository, `aver check -- npx jest|vitest|mocha` always falls back to exit-only. The refusal also hides that reason: it does not print the `ExitOnly::NotInstalled` text. Each refused attempt still ran the suite first, taking 8–24 s. The workaround was a hand-written check: `npx jest --env node --json --outputFile="$AVER_OUT"/jest.json` with `report = "jest-json"`.

### S5. `aver init` tells a pnpm repository to commit an npm lockfile (ms)

```
! missing lockfile: package.json lists dependencies, but no package-lock.json is committed (pnpm-lock.yaml is committed, but Aver installs only from package-lock.json): Aver installs nothing for it and its runner's reporter cannot be added. Commit one (`npm install --package-lock-only`).
```

Outside CI, Aver installs nothing anyway. The advice would add a second lockfile that drifts from the first. The same draft makes jest's `npm test` exit-only and leaves out `[deps] keep = ["node_modules"]`.

### S6. Following "Commit it with your change" turns every first fix into REVIEW (semver, humantime)

`aver check -- <cmd>` says `Commit it with your change`. Doing so makes the config head-proposed, so a correct fix ends like this:

```
NEEDS HUMAN REVIEW: REVIEW (a human must run 'aver accept --approve-review' or approve the PR). 1 of 1 criteria proven, no new failures, under the aver.toml your change adds, which a person must approve.
```

This is by design, but a newcomer's first good fix never reaches PASS, and nothing says "land `aver.toml` on its own first". With the same file committed to the base, the same fix was PASS.

### S7. `aver init` drafts checks that no one should run

These come from the humanize, semver and humantime drafts:

- `test "$TEST_RESULT" = "success"`, from a CI aggregation job, is drafted as a check. It is red on HEAD, and it is the first candidate `aver check` offers:

  ```
  Commands this repository declares:
    aver check -- test "$TEST_RESULT" = "success"
    aver check -- pytest
  ```

- `[[deps]] lockfile = "requirements-mypy.txt"` makes the mypy requirements file the project's lockfile.
- `tox`, which is not installed here, is drafted anyway (`! missing tool: tox`).
- `make test` is drafted beside the detected `go test` check that it duplicates.
- `make lint` is drafted although it is red on HEAD.
- Every draft check carries a Docker `image = "python:3.14-slim-bookworm@sha256:…"` (also `golang:1.27`, `rust:1.99`) and `/aver/out/…` paths. 0.4 removed the Docker sandbox.
- `cargo test --all-features` is reported as `Aver already runs it (check cargo)`, but the drafted check runs without `--all-features`, so the `mu` feature is never tested.

### S8. Rust verify costs 2–4× `cargo test` on a crate with no `Cargo.lock` (humantime)

The project's own `cargo test` takes 31 s cold. A fresh `aver check` took 47–125 s:

```
Took 125.0 s: … your code 3 runs in 83.8 s, your tests 3 runs in 124.7 s, …
Took 82.8 s: … your tests 1 run in 47.9 s, your tests on the original 1 run in 48.4 s, your code with one change reverted 2 runs in 34.2 s, …
```

Each slot run builds cold in about 26 s. The checkout's `target` (93 MB) is not kept, because `aver check --` writes no `[deps]` when the tree has no lockfile (`Cargo.lock` is gitignored here). Failing tests are rerun up to three times, and each rerun is a cargo invocation.

### S9. Report wording

- **ms:** one report gives two counts: `Your change breaks 15 tests that pass on the original code` and `New failures, yours to fix (18)`. The 3 skipped tests count as NEW, not as "breaks".
- **ms:** the JS gaming report never says the tests were skipped. The Python report does: `You skipped …`.
- **humanize:** `You skipped tests/test_number.py > test_ordinal[-inf--Inf0]` names one parameter case out of 14, when the whole function was skipped.
- **humanize:** pytest's ANSI colour reaches the fenced output as `\u001b[31m…` in one place and `#x1B[0m#x1B[91m` in another. The 15-line tail is mostly pytest-benchmark's table.
- **humanize:** the first FAIL_OWN says `passes_without_change` for a tagged test that fails on both sides.
- **semver:** the `NOT CHECKED` candidates are `make test` and `make lint` only, although `aver init` detected `go test`. A newcomer will try `make test` and be refused as exit-only.
- **humantime:** the CHANGED diff for an inline Rust test starts with the source hunk (`fn from_str`), not the test lines.

## Fixes, ranked

1. **Make slots import the slot's code under an editable install (S1).** When a kept `.venv` contains a `.pth` file or `__editable__` finder that points into the source checkout, either rewrite it to the slot, prepend the slot's import root (`src/` or the project root) to `PYTHONPATH` in the check `aver check --` writes, or refuse with a one-line reason. Add a test that runs a regression under `uv pip install -e .` and expects FAIL_OWN. This is the only wrong signed PASS found.
2. **Compute `min_tests` from tests that ran, not skipped ones (S2).** Without this, every Python project with skips is blocked from its first verify.
3. **Read `package.json` in `aver check -- <runner>` (S4).** Pass the manifest to `runner::reporter`, as `npm test` already does, and print the `ExitOnly` reason in the refusal. This unblocks every pnpm, Yarn and Bun repository.
4. **Handle generated, gitignored files and explain a red base (S3).** When the base is red because every module fails to import, show the first error line in `aver init`'s flag. Suggest a `prepare` step, or keeping the generated file, for setuptools-scm and hatch-vcs.
5. **Stop recommending a committed `package-lock.json` in pnpm and Yarn repositories, and keep `node_modules` in the draft (S5).**
6. **Prune the `aver init` drafts (S7).** Drop shell `test` comparisons and steps that are red on HEAD or need a missing tool, unless asked. Never treat a `requirements-*.txt` file named after a tool as the lockfile. Remove `image`, `/aver/out` and `memory_mb` from drafts now that 0.4 has no Docker. Don't claim a variant with flags is "already run".
7. **Tell a newcomer to land `aver.toml` on its own first (S6).** For example, `aver check --` could say: "commit aver.toml to your default branch first; a change that adds it gets REVIEW."
8. **Keep the Rust `target` when there is no `Cargo.lock` (S8).** At least warm one build and clone it into the slots; then measure the reruns.
9. **Rank the `NOT CHECKED` candidates (S7, S9).** Put detected per-test runners first, then exit-only ones, and never CI glue such as `test "$TEST_RESULT" = "success"`.
10. **Fix the report wording (S9 and the newcomer run).** Strip ANSI codes in tails, make the "breaks" and "new failures" counts agree, name a skipped test function rather than one parameter case, say "You skipped" for JS skips, and fix the demo's "(Criteria above)" and the "about 30 seconds" help text.

Not covered: `aver init --yes` drafts were not run end to end (each was inspected and its dry run read), `aver verify --fresh` timings, and `aver run` with real agents. Testing stopped at 14 GiB free.
