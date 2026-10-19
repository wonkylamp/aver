# WP3 plan, part 2: blockers, the contract at runtime, reports, structure

Planning document. Read at `9d9805e`, after part 1 (`wp3-plan.md`) landed: the judge table, the lossless probe, deleted-source inline tests and the Workdir fault test. This part finishes REDESIGN.md's WP3: the rest of §4, §5.1, §5.2, the `judge/` split, the deletion of `check.rs` and `detect.rs`, and the gate test `verdict_integrity.rs`.

## 0. Where things stand

An audit at `9d9805e` found:
- `aver_core::evidence::decide` (the §4.1 blockers: no tests, unapproved exit-only, below the floor, all red, runner error, truncated package, setup failed) is written and tested in `aver-core` but nothing in `crates/aver` calls it. A base and head that both exit 0 having run no test still `PASS` (B1).
- `aver::contract` (the repository test contract, legacy conversion included) has no runtime caller. Checks still come from `env.rs`'s `CheckDef` and `detect`'s fallback.
- `Origin::BuiltArtifactModified` is never constructed, and `Workdir::derived` and `probe_copies` are called only from tests (B3).
- No `<src>` normalization, no parameter-suffix pairing, and the transplant reruns up to three times (§4.5).
- Weak proofs are detected from failure messages only, not from the changed identifiers (§4.6). `pushback.rs` is unwired.
- `verify.rs` is one 4,383-line file; `check.rs` (3,335 lines) is reached only by `aver check --combined` and `python_fallback`.

## 1. Decisions

1. **A blocker is `BLOCKED_NOT_OWN`, not a new `BLOCKED` verdict.** It exits 1 locally and in CI already, so it meets §4.1's exit-code goal without a receipt schema bump. Each blocker is an `ENV` entry named `<check>::(evidence) <reason>`, so the receipt says why.
2. **`--keep-going` stays on the probe build.** `cargo test` cannot keep compiling past a failing target; the probe does (`exec/cargo.rs`). §4.4's bullet is already met by another mechanism.
3. **Changes to examples stay uncovered** unless a check builds them; Aver's own `aver.toml` is not changed here.

## 2. Packages

Each package is one or more commits that end with their targeted tests green; the full gate (`make lint test`, `make test-custody`, the ignored suite) runs in the main checkout after each wave merges.

| # | Package | Files | Wave |
|---|---|---|---|
| A | Blockers: `evidence::decide` wired per check (ran == 0 either side, all red, pytest exit 2–4, Go truncation from declared `Test*` functions, setup failed, stale copies) | `verify.rs` (entries region), `aver-core` evidence/rules, `exec/gojson.rs`, `place/sides.rs` | 1 |
| B | The contract at runtime and the first-run bootstrap: checks load through `contract::parse`; no contract prints `NOT CHECKED` with up to three candidates; `aver check -- <cmd>` refuses zero or all-red evidence and writes `aver.toml` with `min_tests` and `deps.keep`; `detect.rs` deleted, its runner helpers in `adhoc.rs` | `contract.rs`, `env.rs`, `adhoc.rs`, `cli/check.rs`, `cli/verify.rs`, `ci/`, `detect.rs` | 1 |
| C | Run policy and ids: transplant once; no base rerun on an identical command-level failure; slot paths as `<src>` in test ids; parameter-suffix pairing with one fresh base run | `exec/{junit,jest,gojson}.rs`, `verify.rs` (run region), `aver-core` rules | 1 |
| D | `min_tests` and exit-only approval through `decide` (needs A and B) | `verify.rs`, `contract.rs` | 2 |
| E | Built artifacts (`built::<path>` from `Workdir::derived` on base and judged head) and the B7(b) struct-change test | `place/sides.rs`, `verify.rs`, `tests/rust_compat.rs` | 2 |
| F | Weak proof by changed identifiers, pushbacks (at most one, top of the report), §5.1 reports and hygiene | `diagnostics.rs`, `run/report.rs`, `ci/summary.rs`, `cli/` | 2 |
| G | Structure: `verify.rs` split into `judge/{plan,run,finish}.rs`; `check.rs` deleted (`--combined` as a dry verify on a trial merge); `REBUILD_FAILED` and rule 7 deleted; ARCHITECTURE.md §1.9.4 rewritten | everything above | 3 |
| H | `tests/verdict_integrity.rs` (B1–B8, natively where needed) and `bench/verify-10-of-10.sh`; the §8 tests to rebuild | tests, bench | 3 |

## 3. Done

WP3 is done when `cargo test -p aver --test verdict_integrity` passes with B1–B8, the full gate is green, and the dogfood replays still give 32dee73 no `NEW` and 388ca3a exactly the two `recover.rs` `NEW` entries.
