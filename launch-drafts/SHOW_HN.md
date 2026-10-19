# Show HN draft

Not posted. Numbers in double braces, such as `{{NUM:ws-create-aver-p50}}`, are placeholders until the evaluation finishes.

## Title options

- Show HN: Aver – your agent says "done", Aver checks
- Show HN: Aver – version control built for AI agents
- Show HN: Aver – agents get their own workspaces, only proven work reaches Git
- Show HN: Aver – a change from a coding agent only counts once it's proven

## First comment

Hi HN. I built Aver because coding agents say "done" when they aren't. They weaken the failing test or break something else, and with several agents I ended up reading every diff.

Aver is version control built for AI agents. Agents work entirely in Aver, and a change only counts once it's proven. Your team keeps Git: Aver hands it finished work.

How it works:

- Each agent gets a copy-on-write workspace with dependencies linked, in 87.5 ms warm.
- Every change is snapshotted. `aver restore --last-green` rewinds in about 32 ms.
- `aver verify` runs the tests on the original code and on the change. Done means each criterion has a test that fails without the change, nothing that worked broke, and no test was weakened.
- `aver combine` merges several agents' work only when everything together still passes.
- The result reaches Git as a branch or PR with a signed receipt.

It works with Claude Code, Codex, Cursor, Gemini CLI and VS Code, and as a GitHub Action for PRs from any agent. In our runs, agents under Aver used about 19% more tokens. It judges 15 of 15 sabotage scenarios right, 10 of 10 runs each.

`aver demo` shows it offline in about 2 seconds: an agent hides the tests it broke and gets FAIL_OWN. Then `aver init` sets up your repository.

What it doesn't do yet:

- It is a prototype: Node/TypeScript, Python, Rust and Go only; macOS, Linux, and Windows through WSL2.
- Agents are not sandboxed. They run on your machine as you.
- A weak test can still prove a criterion. Read the criterion tests.
- Large Rust workspaces take minutes to verify, and about 7 GiB of disk each on Aver's own repository.
- No hosted service.

MIT: https://github.com/wonkylamp/aver. I'd like to hear where it gets in your way.
