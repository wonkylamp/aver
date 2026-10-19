# The verdict for a change that removes a public API with its tests

Status: 5a implemented in the commit that adds this file ("Verify: judge per test when restored base tests do not build"); 5b declined by the owner, so REVIEW comes from 5a alone and there is no `removes:` line or task schema bump. Decisions 3 and 4 taken: a restored base test file that does not compile against the head is one `CHANGED` entry per file, `<check>::(did not compile) <path>`, with its first compiler error (not the single `(judged build)` entry of §5a), a restored test that compiles and fails stays `NEW`, and the summary, the agent report and `aver check` say how many test files did not compile instead of "they still ran from the base version". docs/VERDICTS.md and ARCHITECTURE.md §1.9.3 hold the rule as built. Written 2026-10-16 from WP2 commit 7 (`388ca3a`, "daemon gone"), verified against `3230c74`.

## 1. The case

Commit 7 deletes `aver::http` and `aver::daemon` with the tests and helpers that used them. Under `crates/aver/tests/` it deletes 10 files (`auth.rs`, `auth_routes.rs`, `daemon.rs`, `home_lock.rs`, `http_agent_ux.rs`, `http_disk.rs`, `http_support/mod.rs`, `http_tasks.rs`, `http_workspace.rs`, `verify_adhoc.rs`), modifies 35 (among them the shared modules `run_support/mod.rs`, `common/mod.rs`, `world/mod.rs` and `scripted/mod.rs`), and adds `scripted_off.rs`.

## 2. Reproduction

`aver ci --repo . --base 3230c74 --head 388ca3a` with the installed `aver` 0.3.0 in a throwaway home: `FAIL_OWN`, exit 1, receipt `rcpt_ydouojw4qgrnimjfwy5w63b4x65esemjtqw2zr7tve7yzbdezo6q`.

The only entry with cause `NEW` is the command-level `rust` entry: base `fail`, head `fail`, no transplant or head-tests status. Every other entry with a cause (`site::*`, `bench::*`) is `PREEXISTING`: Node 22 on this machine cannot load `.ts` test files. Those entries are unrelated to this case.

What each `rust` run did (`aver-out/runs/rust.<side>.<n>.log`):

| Side | Tree | Result |
|---|---|---|
| base | `3230c74` | Builds. Fails 3 tests on every run: `integrate::a_stale_integration_is_reported_by_doctor_the_hooks_and_the_cli`, and two `place` tests that get `PermissionDenied` binding a listener under 0.3.0's network-off profile. |
| head (judged) | `388ca3a` with the base copies of every deleted and modified test file restored | Does not build: `unresolved import aver::http` in the restored `run_support/mod.rs`, `http_support/mod.rs`, `http_agent_ux.rs` and `api.rs`. Cargo stops at the first target that does not compile (`--no-fail-fast` does not keep it going), so no test binary runs and there is no libtest report. |
| head-tests | `388ca3a` with its own test files | Builds. Fails the same 3 tests plus the new `place::in_process::no_command_starts_a_daemon_and_serve_is_gone`, which hits the same `PermissionDenied`. |
| transplant | `3230c74` plus the head's added and modified test files | Does not build (`aver::exec::scripted` is missing on the base). |

The summary also lists "Deleted 80 existing tests (they still ran from the base version…)". That is not what happened: they never ran, because the judged head did not build.

## 3. Why the engine says FAIL_OWN

1. The judged head restores every base test file (verify.rs module doc, ARCHITECTURE.md §2.5.1). Its base copies call an API the head removed, so they do not compile. In Rust one broken target (or a broken shared module such as `run_support/mod.rs`) stops the whole build.
2. The base reported every test and the head reported none, so the check falls back to one command-level entry, and `rules::lost_report` makes it `NEW` ("the change is why, whatever already failed on the base").
3. Rule 6 (`rules::changed`) could make a command-level entry `CHANGED` ("any changed test file counts, because the judged head and the head-tests tree differ only in those files"), but only when the head-tests run passes as a whole. The base's own red tests also fail there, so the run fails, and `NEW` stands.

So the H2 rule for deleted tests (`NEW` stays `NEW` when the head's own version is `absent`) is not what fires here. The command-level fallback fires first.

Two consequences beyond this commit:

- **On a green base, the same commit gives `REVIEW`, but for a coarse reason.** The head-tests run would pass, rule 6 would turn the one `rust` entry into `CHANGED`, and the whole Rust suite would sit behind a single reviewed entry.
- **That coarse rule hides regressions behind `REVIEW`** (read from the code; not run). Take an agent that breaks `foo` in a Rust crate, renames any `pub` item some existing test file uses, and updates that test file. The base copy no longer compiles on the head, so the judged head loses its report, and the head-tests run passes if the agent also weakened `foo`'s test. The receipt is then `REVIEW` with one `rust` entry, where per-test judging would give `NEW` for `foo`'s test and `FAIL_OWN`. A person still has to sign off, but what they see does not name the regression.

## 4. The two candidate verdicts

**REVIEW, with a `CHANGED` entry for each deleted test.** This means switching off H2 when the base copy fails only because what it uses is gone (a Rust compile error, or `Cannot find module` or `is not a function` in JavaScript: the signs `diagnostics::missing_name` already reads). It contradicts `ci_trust::deleting_the_test_file_of_a_broken_test_exits_1_crit_ci_trust_delete_test`, which deletes `src/add.js` together with `test/math.test.json` and asserts `FAIL_OWN`. The gaming case and a legitimate removal have the same shape: an implementation removed with its tests. Nothing in the trees tells them apart; only the intent does. `REVIEW` still exits 3 in `aver ci` and is never auto-accepted. But `aver verify` reports it as done, so an agent that cannot fix `add` learns that deleting it ends the task.

**An `UNPROVEN` reason.** `UNPROVEN` means a criterion has no proof, and every `rules::UnprovenReason` names a criterion. A deleted test that fails on the head is a failing entry, not a missing proof. For a regression-only receipt such as this one (no criteria), there is no criterion to attach the reason to. This option misreports what happened.

Neither verdict is right as a rule of the trees alone. Removal is a decision about intent, and Aver learns intent only from the task. ARCHITECTURE.md §1.9.3 already says "deleting a test is a regression unless a task authorizes it", but nothing implements the authorization.

## 5. Recommendation

**5a. Judge per test when only the restored test files break the judged head's build (an engine fix, no owner call on semantics).** When the judged head lost its report and the head-tests run has one:

- Judge each base test in a test file the head left alone against its head-tests result. That file is the same on both sides, and so is the harness.
- Judge each base test in a file the head modified or deleted by rule 6 against its head-tests result. Its own version passing is `CHANGED`; `absent` is `NEW` (H2) for a test the base passed, and `CHANGED` for one already failing.
- Add one entry, `<check>::(judged build)`, with cause `CHANGED` and a detail that names the base test files that did not compile on the head.

This fix has a limit. A test in an untouched file that declares a shared module the head modified (`mod run_support;`) ran with the head's module in the head-tests run, so its result is the head's helper judging it, not the base's. Map such tests to the shared module's file, which rule 6 already does for module paths in the test's name. Treat a target-level `mod` declaration the same way, so the test counts as "in a modified file". With that, the regression hidden behind `REVIEW` in §3 becomes `NEW` again. Commit 7 would get named entries: the 80 tests in deleted files `NEW` (`FAIL_OWN`), the modified files' tests `CHANGED`, the base's 3 red tests `PREEXISTING`, and the new `place` test `NEW` on this machine (its `PermissionDenied` is 0.3.0's network-off profile; 0.4 runs with the network on).

**5b. Let the task authorize removals (owner call: a new task field and schema version).** Add a criteria-block line `removes: <glob>` beside `protected: <glob>`, stored as the task's `removed_paths` (a new task schema version and `schemas/aver.task.v3.json`; ARCHITECTURE.md §1 first). A base test whose defining file the head deleted under a `removes` glob, with own version `absent`, becomes `CHANGED` with the note "the task removes this file; its old version fails on the new code". The verdict is then at best `REVIEW`. Without the line, H2 and the crit test stay as they are. In `aver run` and in a workspace directory the agent cannot write its own task. In `aver ci` the line comes from the pull request body, which is as trusted as the criteria that already come from there.

With 5a and 5b, commit 7 verified under `removes: crates/aver/tests/{auth,auth_routes,daemon,home_lock,http_agent_ux,http_disk,http_tasks,http_workspace,verify_adhoc}.rs, crates/aver/tests/http_support/**` would be `REVIEW` on a machine where the base is green: a person signs off the removal.

## 6. Owner decisions

1. Take 5a into WP3's `verify.rs` work, or as a fix before it? (Recommended: before. The `REVIEW` that hides regressions in §3 is a trust hole today, not only a false `FAIL_OWN`.)
2. Accept the task schema bump for `removes`, or carry the authorization outside the hashed task (an `aver verify --removes <glob>` flag recorded in the receipt)? (Recommended: the task, so the verdict follows from signed inputs.)
3. Is an authorized removal `REVIEW`, or `PASS`? (Recommended: `REVIEW`. A person approved the task, not the deleted assertions.)
4. The summary's "Deleted N existing tests (they still ran from the base version…)" is false when the judged head did not build. Fix the wording with 5a?
