# Real-agent trials

By-hand trials of `aver run` with real Claude Code agents (`scripts/try-run.sh`), each followed by the fixed feedback questions (`aver feedback`). Agents are Aver's main users, so their ratings against "I absolutely need this" are tracked across versions.

| Trial | Date | Aver | Task | Agents | Result | Ratings |
|---|---|---|---|---|---|---|
| [1](trial-1.md) | 2026-10-13 | first `aver run` (51da741) | fixtures/calc: add `negate(x)` | 2 × Claude Code | both PASS in ~1m30s, $0.30 and $0.33 | 6, 6 |
| [2](trial-2.md) | 2026-10-13 | 8b6e3f1 | same | 2 × Claude Code | both PASS in ~50s, $0.31 each | 6, 7 |

Both trials' agents said the task was too easy to show Aver's value; the hard-task A/B evaluation is `make eval` (bench/eval/).
