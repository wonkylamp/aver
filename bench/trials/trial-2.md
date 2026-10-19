# Trial 2: same task, at 8b6e3f1

```
  #  agent   verdict          crit  new  preexisting  diff          time     verifies  cost
  1  claude  PASS             1/1   0    2            2f +14 -1     50s      2         $0.31
  2  claude  PASS             1/1   0    2            2f +14 -1     53s      2         $0.31
```

Ratings from `aver feedback`: **6/10** and **7/10**.

Helped (quotes from the agents' answers):
- `aver check`: "neg  WOULD PROVE: fails on the original ✓, passes with your change ✓" in 0.5–0.7 s, so they verified once with confidence.
- "Already failing before you, ignore (2; they fail on the original code too)".
- "Next: nothing. You are done; stop here." left no doubt.
- "I wrote the right test on the first try."

Friction:
- Verify still 21.7 s for the first transplant runs.
- The fenced-data caveat repeats in every report.
- Their own harness's permission prompt on compound shell commands (not Aver).

Both: "this task was trivial… Aver's value grows with the difficulty of the task", and they expect it to be clearly better than a plain checkout on already-failing or flaky suites, vague criteria and agents editing the same code.

Asked for: `aver check` should predict `aver verify` exactly (sandbox-only differences, protected files, flaky tests), and verify should reuse unchanged original-side runs. Commit bc46276 addresses both.
