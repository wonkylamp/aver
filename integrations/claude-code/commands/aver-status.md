---
description: Show an Aver run's agents and attempts, ranked by receipt.
argument-hint: "[run id] [--all]"
disable-model-invocation: true
allowed-tools: Bash(aver status:*)
---

Run `aver status $ARGUMENTS`. With no arguments it shows the newest run; `--all` lists every run.

Summarize it: which agents are still working and, for each finished attempt, the verdict, proven criteria, new failures, diff size, verifies used and cost. Name the best attempt and the commands for the next step, `aver diff <n> <m>` and `aver accept <n>`, but run neither unless the user asks.
