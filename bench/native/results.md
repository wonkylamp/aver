# Native verify benchmark

`make bench-native` on Darwin arm64 (Apple silicon laptop), 2026-10-14. Each case: `aver workspace new`, a one-line edit and `aver verify` (cold pools), then a second one-line edit and `aver verify` (measured), against running the tests directly on the same edit in a warm checkout. Target (plan §3.5): calc under 2 s; aver at most 1.05 times warm `cargo test` plus 1 s.

| Case | First verify (cold) | Verify after a one-line edit | The tests alone | Ratio | Verdict |
|---|---|---|---|---|---|
| calc | 3.85 s | 0.56 s | 0.41 s (`node --test`) | 1.37 | `DONE: PASS` |
| aver (one-line edit in aver-core) | 408.90 s | 102.73 s | 107.95 s (warm `cargo test`) | 0.95 | `DONE: PASS` |

Where each aver verify's time went (its report's last timing line):

- first edit: Took 408.8 s: sync 31 ms, plan 23 ms, original code 3 runs in 408.6 s, your code 1 run in 407.8 s, judge 123 ms. The base pool and the head pool both build the workspace from nothing (the base reruns because some tests fail on the original code natively too).
- second edit: Took 102.7 s: sync 39 ms, plan 28 ms, original code cached from a run 9 min ago, your code 1 run in 102.5 s, judge 95 ms. The only run is the head's incremental `cargo test`.

Both targets are met: calc verifies in 0.56 s, and the aver verify waits no longer than the warm `cargo test` it runs (0.95 of it; the overhead is about 0.2 s). The machine was shared with other agents' Docker sandboxes; an earlier run under heavier contention measured 176 s against 105 s (1.67), so compare runs on a quiet machine. Before round 14, one such change took 75 to 160 minutes to verify in the sandbox (bench/dogfood/LOG.md, round 12).
