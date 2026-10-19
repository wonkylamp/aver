# Verify N of N

`bench/verify-10-of-10.sh` on Darwin arm64, 2026-10-19: each scenario verified 10 times with `aver ci --fresh` (b5 and b11: a workspace directory, verified and accepted), each in a throwaway home.

| Scenario | Expected (verdict, exit) | Runs as expected | Seen |
|---|---|---|---|
| pass | PASS 0 | 10 of 10 | PASS 0 |
| regression | FAIL_OWN 1 | 10 of 10 | FAIL_OWN 1 |
| b1 | BLOCKED_NOT_OWN 1 | 10 of 10 | BLOCKED_NOT_OWN 1 |
| b2 | BLOCKED_NOT_OWN 1 | 10 of 10 | BLOCKED_NOT_OWN 1 |
| b3 | REVIEW 3 | 10 of 10 | REVIEW 3 |
| b4 | FAIL_OWN 1 | 10 of 10 | FAIL_OWN 1 |
| b5 | REVIEW 0 2 2 0 | 10 of 10 | REVIEW 0 2 2 0 |
| b8 | PASS 0 | 10 of 10 | PASS 0 |
| b9 | GREEN 0 | 10 of 10 | GREEN 0 |
| b10 | EXPORTED 0 | 10 of 10 | EXPORTED 0 |
| b11 | PASS 0 0 0 | 10 of 10 | PASS 0 0 0 |
| sensitive | REVIEW 3 | 10 of 10 | REVIEW 3 |
| b13 | UNPROVEN:src/math.js:6-6 3 UNPROVEN:src/math.js:15-15 3 PASS 0 PASS 0 | 10 of 10 | UNPROVEN:src/math.js:6-6 3 UNPROVEN:src/math.js:15-15 3 PASS 0 PASS 0 |
| b14 | BLOCKED_NOT_OWN 1 | 10 of 10 | BLOCKED_NOT_OWN 1 |
| b15 | BLOCKED_NOT_OWN 1 | 10 of 10 | BLOCKED_NOT_OWN 1 |

B9 times over 10 runs (debug or release as built; targets: warm creation under 100 ms, restore under 50 ms):

- warm `aver workspace new` (one directory, the repository already imported at HEAD), wall clock: median 87.5 ms, runs 74 69 116 78 88 59 94 116 87 116 
- `aver restore --last-green`, wall clock: median 33.5 ms, runs 31 28 33 34 35 26 41 37 51 28 
