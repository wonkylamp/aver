---
description: Run several agents on one task with Aver, each in its own workspace directory, and rank their attempts by receipt.
argument-hint: '"<task>" [--agents N] [-c "id: text"]...'
disable-model-invocation: true
allowed-tools: Bash(aver run:*), Bash(aver status:*)
---

Start an Aver run in this repository for: $ARGUMENTS

1. If the arguments hold only a task description, use `--agents 3` and turn each acceptance condition the user stated into a criterion `-c "<id>: <text>"` with a short lowercase id. Show the command before you run it.
2. Run it in the background so it outlives this session: `aver run "<task>" --agents <n> -c "<id>: <text>" --detach`. It prints the run id.
3. Follow it with `aver status <run>` until every agent is done, then show the ranking: each attempt's verdict, proven criteria, new failures and diff size.
4. Suggest `aver diff <n> <m>` to compare the best attempts and `aver accept <n>` for the winner. Do not run `aver accept` or `aver discard` unless the user asks.

The agents spend tokens. `aver run` needs `claude` or `codex` installed.
