# WP3 plan: one decision table, lossless evidence, mechanical oracles

Planning document. Read at `ebb525e`. Paths are under `crates/aver/src/` unless they start with `tests/` (`crates/aver/tests/`), `aver-core/` or a repository-root file. **[inference]** marks a reading the code does not state.

This is the first half of REDESIGN.md's WP3 ("Unified Judge & 6 Hard Invariants"). It covers invariants §4.1 (zero evidence), §4.2 (deleting or skipping tests), §4.3's cell for built artifacts and §4.4 (API changes on frozen tests), and the truncation rule below. The `judge/` module split, the reports of §5.1, the contract bootstrap of §5.2 and the deletion of `check.rs` and `detect.rs` follow in a second plan.

## 0. Why

Dogfooding the API-removal commits found verdict holes one at a time: 5a (a restored test file that no longer compiles), the 64 KiB probe cut (built targets read as missing), and the deleted `workspace/patch.rs` with its inline tests. Each fix was one more reclassifier after `rules::classify`: `lost_report`, `changed`, `unbuilt_test_file`, `deleted_with_code`, `not_rerun`, `base_ran_no_tests`, `with_proof_runs`, `unbuilt_dependency`. Their order is the policy, and no one can see which combinations of states they leave unmapped. The redesign gives each entry five typed dimensions, and one pure function with a wildcard-free `match` maps every combination. The compiler then rejects a new state that has no cause.

The dogfood receipts at the start of this plan:

| Commit | Verdict | Entries with a cause | False `NEW` |
|---|---|---|---|
| 388ca3a (WP2 commit 7), before 5a | `FAIL_OWN` | 374 `CHANGED`, 231 `NEW` | 229 |
| 388ca3a, after 5a | `FAIL_OWN` | 21 `CHANGED`, 2 `NEW` | 0 |
| 32dee73, before 6726a1d | `FAIL_OWN` | 9 `CHANGED`, 3 `NEW`, 1 `PREEXISTING` | 3 (`workspace::patch::tests::*`) |

The two `NEW` entries left on 388ca3a are correct. The commit deleted two daemon tests from `tests/recover.rs`. Their base copies compile on the head and fail at runtime (`daemon.json` is gone), and the owner's rule H2 keeps such a test `NEW`. A removed behavior and a hidden regression look the same there, so 388ca3a stays `FAIL_OWN` by design.

## 1. Decisions

The owner's earlier decisions (docs/redesign/api-removal-verdict.md) stand. Two more were made for this plan on 2026-10-16:

1. **A modified test file can still `PASS`.** An entry may pass when the base's frozen copy of the test passes on the judged head and the head's own version passes. Adding a proving test to an existing test file is how most tasks prove a criterion. A skipped passing test, a rewritten expectation, a deleted test and a changed built artifact are each `CHANGED`.
2. **Who lost the result decides `ENV`.** Aver's own loss is `ENV`: output or a report that Aver cut, an incomplete probe, a sandbox failure, or a target that was not rerun. A head that times out, or whose test binary crashes, over a base that completed stays `NEW`. So a change that hangs its tests is never told the failure is not its own. Neither case can ever `PASS`.

## 2. The table (`aver-core`)

The dimensions are `Copy` enums with no data:

| Dimension | Variants |
|---|---|
| `Origin` | `UntouchedSource`, `ModifiedSource`, `DeletedSourceWithInlineTests`, `UntouchedTestFile`, `ModifiedTestFile`, `DeletedTestFile`, `AddedTestFile`, `BuiltArtifactModified` |
| `Base` | `Pass`, `Fail`, `Flaky`, `CompileError`, `Infra`, `ZeroTests`, `Absent` |
| `Judged` | `Pass`, `Fail`, `Flaky`, `CompileError`, `Exhausted`, `Infra`, `Missing`, `NotApplicable` |
| `Own` | `Pass`, `Fail`, `Flaky`, `Skipped`, `Absent`, `CompileError`, `Infra`, `NotRun` |
| `Transplant` | `FailOnAssertion`, `FailOnCompileOrImport`, `Pass`, `NotRun` |

The table's output is `(BaseStatus, HeadStatus, Option<Cause>)`, so the receipt's fields come from it unchanged. The original request's dimensions gain four variants, each needed to express a rule that already exists. `Base::Absent` is a test the base never had. `Judged::Flaky` is a head flaky over a passing base, which is `NEW`. `Judged::Exhausted` and `Judged::Missing` follow decision 2. `Own::NotRun` covers a file the head did not touch, which has no head-tests run.

The cells that matter:
- `DeletedSourceWithInlineTests` gives `CHANGED`, never `NEW`.
- `DeletedTestFile` or `ModifiedTestFile` with `Judged::CompileError` gives `CHANGED` (5a).
- `DeletedTestFile` or `ModifiedTestFile` with `Judged::Fail` gives `NEW` (H2). The exception is the existing rewritten-expectation rule: `Own::Pass` over a passing base, or a failing base with the head's version passing or deleted, gives `CHANGED`.
- `Base::ZeroTests` gives `ENV` always. Today only a `PREEXISTING` or `FLAKY` entry becomes `ENV`, so a passing head over a base that ran nothing could pass. This is an intended change.
- An unrelated passing test that the head skipped gives `CHANGED` (REDESIGN §4.2). This is an intended change.
- `BuiltArtifactModified` gives `CHANGED` unless `NEW` or `ENV` applies.
- `Infra` on any side gives `ENV`, never `NEW`, never no cause.

Proof (`proves`, weak proof through `FailOnCompileOrImport`) stays a separate small function over the cause, `Own` and `Transplant`. `verdict_with_coverage` is unchanged.

## 3. Lossless evidence

The facts at `ebb525e`:
- A run's stdout is cut to `limits.output_bytes`: the first 8 KiB and the last 56 KiB, with the middle dropped and no marker (`place/proc.rs:258`, `exec/output.rs:181`). Nothing on `CheckRun` records the cut.
- Report files are collected whole from the out directory, up to `MAX_BLOB_BYTES` (64 MiB). The Rust preset writes libtest's text to its own file, so per-test results do not pass through the cut.
- The cargo probe is still read from the cut stdout (`verify.rs` `rebuild`), with a 1 MiB budget. Over that budget `Probe::complete` is false, `rebuild` returns `None`, and the check falls back to the command-level `lost_report` entry, which is `NEW`. That is Aver's loss read as the change's failure.
- A report over 64 MiB, or one that does not parse, gives report `None`, which over a reported base is also `lost_report` and `NEW`.

The changes:
1. The probe writes cargo's JSON stream to a file in the out directory (`$AVER_OUT/probe.jsonl`) and prints nothing. It is read line by line from that file, so its size does not depend on `output_bytes`.
2. A probe that is still incomplete, and a report that Aver dropped for size, are `Judged::Infra`, so `ENV`. A report that the runner left malformed is decision 2's case: the change's crash, `NEW`.
3. Every place that reads verdict evidence from the cut stdout is listed, and each one moves to a file or is proven unaffected. This covers setup findings (already guarded in `exec/findings.rs`), `exit_detail` and any report format read from stdout **[inference: none exists; to confirm]**.

## 4. Commits

Each commit ends with the gate green: `make lint test`, `make test-custody`, `cargo test -p aver --no-fail-fast -- --ignored --test-threads=2`.

1. **Dead code and disk.** Delete `Api::release_forced`, the `Submitted` attempt state, the `attempt` and `reject` ledger entries (an old home still reads them, with a test) and `EditOp::Write.mode`. Drop `deps_layers` and `host_deps` (`schema_v6.sql`). Set `debug = 0` for `dev` and `test`. Put Homebrew's Python first on macOS.
2. **The table, beside the old chain.** `aver-core` only. Its tests:
   - one unit test per cell in §2;
   - an exhaustive enumeration of the whole product, about 8,000 inputs;
   - a 50,000-case `proptest` run of the four properties below;
   - a cross-check against the old chain that lists every intended difference.
3. **`verify.rs` builds the dimensions and calls the table.** `check_entries` computes `Origin` from the judging split (`TestFiles`, `deleted_sources`, `retargeted`, built artifacts), and the rest from the runs, the probe and the head-tests runs. Then it calls the table once per entry. The reclassifier closures (`reclassify`, `retarget`) and the old `rules` functions are deleted in the same commit.
4. **The lossless probe and Aver-loss `ENV`** (§3), with a test that drives a probe over 1 MiB and a report over the size limit.
5. **The Workdir fault test.** Fault points inside `Workdir::restore`, with custody rows. A test aborts at every swap and restore point on an adopted Git checkout, recovers, and asserts the same `HEAD^{tree}`, the same `git status --porcelain --untracked-files=all` and no leftover files.
6. **Dogfood replay.** A release binary from `HEAD` runs `aver ci --base 1dc2f2d --head 32dee73` and `--base 3230c74 --head 388ca3a` under a disk watchdog. 32dee73 must be `REVIEW`, with its `CHANGED` entries listed and no `NEW`. 388ca3a must be `FAIL_OWN` with exactly the two `recover.rs` `NEW` entries of §0.

## 5. The properties (commit 2)

1. **Zero evidence.** `Base::ZeroTests` never gives no cause.
2. **No silent weakening.** These inputs never give no cause:
   - `Origin` of `DeletedTestFile`, `DeletedSourceWithInlineTests` or `BuiltArtifactModified`;
   - `Own::Skipped` over a passing base test;
   - `ModifiedTestFile`, except when its base copy and its own version both pass (decision 1).
3. **Monotonicity.** Putting `Infra` on any one side never turns `NEW` into no cause, never turns `ENV` into `NEW`, and never turns any cause into no cause.
4. **Infra.** `Infra` on any side never gives `NEW` and never gives no cause.

## 6. Risks the next engineer inherits

- Mapping a test to its `Origin` still relies on matching libtest target paths by suffix (`verify.rs` `rust_candidates` and `pick`). A wrong match gives a wrong `Origin`, so the table cannot be more correct than that mapping.
- `Workdir::adopt` and `Workdir::restore` have no production caller until in-place CI is wired (WP2 decision 2). The fault test proves they recover, not that anything calls them.
