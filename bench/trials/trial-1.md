# Trial 1: `aver run`, 2 Claude Code agents, first `aver run` (51da741)

Task: "Add a negate(x) function to src/math.js that returns -x", criterion `neg: negate(x) returns -x for positive and negative numbers`, on a copy of fixtures/calc (which has two already-failing tests).

```
Ranked by receipt (run EB6TW2):
  #  agent   verdict          crit  new  preexisting  diff          time     verifies  cost
  2  claude  PASS             1/1   0    2            2f +11 -2     1m32s    2         $0.33
  1  claude  PASS             1/1   0    2            2f +11 -2     1m35s    2         $0.30
```

## Feedback, summarized faithfully (both agents, asked by resuming their sessions)

Both rated **6/10**.

Helped:
- The brief's definition of done ("add a test whose name contains [crit:<id>]… it must fail on the original code"): no guessing what counts as proof.
- The PREEXISTING section ("Not yours… divide returns the quotient"): stopped them chasing red tests they did not cause.
- `git status`/`git diff` worked as usual (one agent found `git ls-files` denied by the allowlist).

Friction:
- Verify took 61–66 s for tests that run in 158 ms natively, with a budget of 5 verifies: "feels like an exam rather than a tool".
- "2 files differ from the base … this sync recorded 0 changes" was confusing.
- `<repo-data … trust="untrusted">` wrappers, snapshot hashes and receipt ids were noise.
- No local way to check a test fails on the original without spending a verify.

Asked for (both): a fast, unlimited local check of the tagged tests against the original and the change, with the full verify as the final gate. Commit 81d8b1f built it (`aver check`).
