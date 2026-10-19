# X thread draft

Not posted. Each `## n` is one post. Numbers in double braces, such as `{{NUM:replay-total}}`, are placeholders.

## 1

Your agent says "done". Aver checks.

Aver is version control built for AI agents. Agents work entirely in Aver, and a change only counts once it's proven. Your team keeps Git: Aver hands it finished work.

github.com/wonkylamp/aver

## 2

The problem: agents say "done" when they aren't. They weaken the failing test, or fix one thing and break another. With several agents at once, you end up reading every diff.

## 3

Every agent gets its own workspace and its own history. A workspace is a copy-on-write clone with dependencies already linked. A warm one takes 87.5 ms. Rewinding to the last green state takes about 32 ms.

## 4

"Done" only counts once it's proven: each criterion has a test that fails without the change, nothing that worked broke, no test was weakened. The agent runs aver verify and reads the answer itself.

## 5

Agents are told when they touch the same files. aver combine merges only when everything merged together still passes.

## 6

Your team keeps Git. Finished work arrives as a branch or PR with a signed receipt. You review, merge and deploy as now.

## 7

15 of 15 sabotage scenarios judged right, 10 of 10 runs each. Replayed through Aver: {{NUM:replay-total}} real agent PRs, {{NUM:replay-fail-own}} broke a passing test. Token cost: about 19% more.

Prototype: Node/TS, Python, Rust, Go. Agents are not sandboxed. MIT.

## 8

Try it. aver demo takes about 2 seconds, offline: an agent games its tests, Aver catches it, a restore rewinds it, the real fix passes.

curl -fsSL https://aver.build/install.sh | sh
aver demo
aver init

Feedback welcome, especially where it gets in your way.
