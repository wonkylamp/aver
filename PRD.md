# Aver: product requirements

> Aver was previously codenamed Strata.

## 1. Vision

In the near future nearly all code is written by many agents working in parallel. The tools they inherited were built for humans: a clone per contributor, branches as a negotiation protocol, pull requests as a review queue, CI as something you poll, logs as something you scroll. Each of those costs an agent tokens, wall-clock time and correctness, and none of them answers the only question that matters when a change lands: **is there machine-verifiable evidence that this change does what was asked and breaks nothing?**

Aver is version control and collaboration infrastructure designed for agents first. Its unit of truth is an immutable, content-addressed **snapshot** of a whole codebase plus the environment that builds and tests it. Agents work in **workspaces**, cheap copy-on-write transactions against a snapshot, edited through an API. Work is driven by **tasks** with testable acceptance criteria. Every attempt at a task produces a signed **receipt**: differential evidence, base versus head, run hermetically, with every failure attributed to a cause. Merges happen on evidence and policy, not on a human reading a diff. Decisions and their lessons go into a **ledger** that future agents can query.

Git survives only at the edges, as an import and export format, so Aver can sit beside existing repositories and hosting. Agents never touch Git.

## 2. Users

| User | What they need from Aver |
|---|---|
| **Worker agent** (primary) | Claim a task, open a workspace in milliseconds, read and edit code through tools, run affected checks, get a receipt that says exactly what it broke and why, fix, resubmit. No clone, branch, PR, CI poll or log reading. |
| **Planner / orchestrator agent** | Write tasks with testable criteria, fan out many competing attempts, pick winners on evidence, escalate conflicts, record decisions. |
| **Human principal** | Set intent and authority (who may create tasks, what may merge unattended), review escalations, audit the ledger. Rarely reads code. |
| **Platform operator** | Run Aver locally or as a service, configure budgets and identities, trust that code under test runs in clean copies with a cleaned environment and cannot change how it is judged. |

## 3. Core concepts

| Primitive | One-line definition |
|---|---|
| Snapshot | Immutable, content-addressed state of the whole codebase plus its pinned environment. The only thing that is ever "the code". |
| Workspace | Copy-on-write transaction against a snapshot. Opens in milliseconds, thousands in parallel, commits atomically into a new snapshot or is discarded. |
| Task | Intent plus testable acceptance criteria (each with an id), signed by an identity with authority to set it. |
| Attempt | One workspace's result for one task. Many attempts per task can compete. |
| Receipt | Signed, differential evidence for an attempt. |
| Unit | A region of the codebase behind a contract (interface, data shapes, budgets). The unit of ownership and coordination. |
| Lease | A time-limited claim on a unit by an agent, preventing collisions before they happen. |
| Agent | A signed identity with scoped capabilities and a track record. |
| Ledger entry | A record of a decision: what changed, why, which attempt won, what failed and what it taught. |

## 4. The target agent loop

```
task.claim → graph.query → lease.acquire → workspace.open(snapshot)
  → workspace.edit / workspace.op → exec.run(affected checks)
  → verify(workspace, task) → fix and re-verify
  → attempt.submit → merge on evidence + policy, or escalate
  → ledger.record → lease.release
```

There is no clone, branch, pull request, CI poll or log reading anywhere in that loop. Every step returns structured data sized for an agent's context window.

## 5. Milestones

| # | Milestone | Outcome | This build? |
|---|---|---|---|
| **M1** | Storage, Git bridge, workspaces, hermetic execution | Import a real Git repo as a snapshot and export any snapshot back. Open, edit, commit, discard copy-on-write workspaces via API. Pin each snapshot's environment from a project config file and hash it. Run checks in isolated sandboxes with cached results. | **Yes** |
| **M2** | Receipts | Differential base-vs-head runs, cause attribution (NEW / PREEXISTING / FLAKY / ENV), criteria-to-check mapping via explicit tags, signed receipts in the object store. `verify()` over MCP and HTTP. Minimal task claim and attempt submit so the loop closes end to end. | **Yes** |
| M3 | Attempts, merge and ledger | Competing attempts, workspace rebase onto a moved base, merge-on-evidence with a hard-coded v0 rule, full ledger with queries ("what was tried for this unit"), affected-test selection from execution traces. | No |
| M4 | Code graph | Index of symbols, callers, contracts and tests over snapshot bytes; `graph.query` with a token budget; affected-check selection from the graph. | No |
| M5 | Coordination | Units declared in project config, leases, contract-change notifications to dependents, escalation of conflicting tasks. | No |
| M6 | Identity and policy | Per-task capability tokens, policy-as-code deciding what merges on evidence and what escalates, agent track records, minimal human console. | No |
| M7 | Production | Distributed object store, remote execution fleet on microVMs, multi-language checks, multi-tenant deployment. | No |

## 6. Requirements for this build (M1 and M2)

This section records what M1 and M2 required. Aver 0.4 deleted the MCP server and the Docker sandbox (REDESIGN.md §1.2): agents use the `aver` CLI, and checks run natively (ARCHITECTURE.md §2.4.6). F5, F9, S2 and the definition of done describe the build as it shipped then.

### Functional

- **F1 Import.** Import any commit of a real Git repository into the object store as a snapshot. Reject submodules and files matching high-confidence secret patterns, with a report.
- **F2 Export.** Export any snapshot back to Git as a commit whose tree is byte-identical to what Git would produce for the same files. An imported-then-exported commit has the original tree SHA.
- **F3 Workspaces.** Open, read, list, edit (write, delete, move, exact replace, unified patch), diff, snapshot, commit and discard workspaces via API. Edits are atomic batches with optimistic concurrency.
- **F4 Environment pinning.** Derive each snapshot's environment from `aver.toml` (container image pinned by digest, lockfile, install command, check commands, limits) and hash it into the snapshot identity.
- **F5 Hermetic execution.** Run checks in sandboxes with no network, CPU, memory, PID and time limits, read-only root filesystem, non-root user, destroyed after each run. Results cached by (snapshot, environment, check). Concurrent identical requests run once.
- **F6 Differential verification.** Run checks on base and head in identical environments, rerun failures up to three times, attribute causes, map checks and individual tests to criteria, produce a verdict.
- **F7 Receipts.** Signed by the verifier key, stored content-addressed, retrievable by id, reproducible with one command.
- **F8 Tasks and attempts (minimal).** Create signed tasks, claim one (creating an attempt bound to a workspace), submit an attempt, advance a named ref when the receipt is PASS.
- **F9 Interfaces.** MCP server (primary) and HTTP API over the same operations.
- **F10 Open schemas.** JSON Schemas for every hashed or signed object, versioned, published in `schemas/` from day one.

### Security

- **S1** Every request is signed by a registered agent key. Unsigned, unknown-key, stale or replayed requests are rejected.
- **S2** Sandboxes have no network, resource limits, read-only root, non-root user, and are destroyed after each run.
- **S3** Repository content is returned to agents as clearly delimited data, never as instructions.
- **S4** No secrets in snapshots or sandbox environments.

### Performance targets

| Path | Target |
|---|---|
| Import a 50k+ line TypeScript repo | < 10 s on a laptop, excluding `git clone` |
| Open a workspace | p50 < 1 ms in-process, < 5 ms over signed HTTP |
| 1,000 open workspaces on one snapshot | < 1 KiB storage overhead each before any edits |
| Cached check result | < 10 ms, zero sandbox starts |
| Snapshot of a workspace with k changed files | O(k × depth) new objects |

### Definition of done

1. One command starts everything locally.
2. Benchmarks: import a real open-source TypeScript repo of at least 50k lines; open 1,000 workspaces on one snapshot; report open time per workspace and storage overhead.
3. A second run of the same checks on the same snapshot is served entirely from cache.
4. End-to-end test through the MCP server: an agent claims a task, opens a workspace, makes a change that fixes one criterion but breaks an existing test, gets a receipt with `FAIL_OWN` and the correct `NEW` cause, fixes it, re-verifies to `PASS`, and commits a new snapshot that exports cleanly back to Git.
5. A test where a check already failing on base is tagged `PREEXISTING`, and one where a deliberately flaky test is tagged `FLAKY`.
6. Tests proving an unsigned request is rejected and sandboxed code cannot reach the network.
7. README explaining how to run it and how an agent connects via MCP.

## 7. Non-goals for this build

Code graph, leases, policy engine, human console, multi-language support, production deployment. Each has a designed slot in the data model (see ARCHITECTURE.md §1.12) so it can be added without rewriting stored objects.

Also out of scope: replacing Git hosting, human code review UX, Windows hosts, monorepo workspaces with multiple package roots, and Git LFS.

## 8. Locked decisions

1. Git-compatible at the edges only. Aver never builds its own storage on a Git working directory and never reads state from one. Workspace directories may carry a write-only Git view for agent tooling (`git status`, `git diff`), which Aver writes and never reads back.
2. Store bytes. Structure goes in an index later. No syntax-tree storage in v0.
3. Receipt, task and ledger formats (and every other hashed object) are versioned open schemas from day one.

## 9. Risks and open questions

- **Self-grading.** If the agent writes the test that proves its criterion, it can write a test that passes trivially. v0 mitigation: tasks may define their own checks, and the receipt marks which proofs came from the task author versus the attempt. M6 adds pinned test paths that an attempt may not modify.
- **Coarse checks mask regressions.** One `npm test` that already fails on base hides any new failure inside it. v0 mitigation: per-test results from JUnit reports, so attribution is per test, not per command.
- **Timeouts are ENV by rule.** An agent-introduced infinite loop times out and is classified ENV, not NEW. Followed as specified; M3 can reclassify head timeouts as NEW when base passed well inside the limit.
- **Dependency install needs the network.** Installs run in a separate, scripts-disabled step; checks run with the network off where the system allows. Restricting install egress to the registry only is an M7 item.
- **Checks are not sandboxed.** Since Aver 0.4 they run natively, as the user, in a cleaned environment (ARCHITECTURE.md §2.4.6).

## 10. North star and roadmap after M2

M1 and M2 are built. This section supersedes the M3-M7 rows in §5 for sequencing.

### North star

Several agents independently improve the same system, and the platform establishes whether their **combined** changes satisfy the intended requirements. Code and files still exist, but the unit of collaboration is a **change with a purpose**: an objective plus its implementation and its evidence, not a branch of edited files. Git's foundations (immutable snapshots, content addressing, distributed history) stay; the collaboration model around them is replaced. Agents use a structured API; people get a clear view of objectives, active work, dependencies, evidence, decisions and releases.

The seven capabilities the platform grows toward, with where Aver stands:

| # | Capability | What it means | Status |
|---|---|---|---|
| 1 | Live system map | Features, code, interfaces, tests, dependencies and decisions linked together. Every statement points to its evidence and shows when it was last checked. Never pretends docs are accurate. | Designed in part (code graph, units). Evidence-linked statements are new. |
| 2 | Intention before patch | A **Change**: objective, constraints, success criteria, authority. It contains the implementation, experiments, failed approaches, decisions and open questions, so a successor agent inherits the real state of the work. | Task + attempts + receipts exist. Constraints, authority scopes and inherited history are missing. |
| 3 | Coordinate before collision | Flag changes that affect the same meaning ("active subscription") even in different files, using declared intentions, dependency analysis and combined execution. Flags likely conflicts; never claims to detect all behavioural conflicts. | Units, leases, contract notices designed only. |
| 4 | Disposable working worlds | Each change gets a reproducible environment with required services, representative data and a running application; experiments are cheap to discard and reproducible when kept. | Workspaces and clean native check runs exist. Services, data and running apps are missing. |
| 5 | Acceptance on inspectable evidence | Before/after reproduction, regression checks, checks against the combined candidate, and explicit gaps. Evidence is tied to exact code and environment; when either changes, the platform knows which conclusions need re-checking. Requirements needing human judgment are exposed, not hidden behind a green suite. | Receipts cover before/after, attribution and unproven criteria. Combined-candidate checks, staleness and human-judgment criteria are missing. |
| 6 | Decisions, not just edits | "Why does this restriction exist?" returns the requirement, alternatives, incident and implementation. "What would change if we removed it?" returns affected behaviours with uncertainty marked. Memory is editable and challengeable so agent guesses do not become folklore. | Signed append-only ledger exists. Queries and supersede/challenge links are missing. |
| 7 | Recovery as an operation | "Withdraw this change and show what depends on it", distinguishing reversible code from migrations, external actions and effects that need a recovery plan. | Not started. |

### Sequencing

The order favours what runs on the user's own machines and runners and can be adopted without moving off GitHub, before anything hosted.

| Round | Outcome | North-star capability |
|---|---|---|
| R2 | Agent UX (search, tree, ad-hoc sandboxed commands, version visibility) and trust (protected paths, task checks in the base environment, receipt verification in the client). **Built.** | 4, 5 |
| R3 | **`aver ci` and the `aver-verify` GitHub Action**: receipts on pull requests, run on the user's own runners, zero hosting cost. **Built.** | 5 |
| R4 | Static site: landing page and in-browser receipt viewer. **Built.** | 5 |
| R5 | **Parallel agents on one repo**: on-disk workspaces replacing `git worktree` plus one install per agent, `aver run` with ranking and accept, coordination between tasks (**built**), dogfooding | 3, 4, 5 |
| R6 | **Change** as the first-class unit: constraints, authority scopes, success criteria, criteria marked "needs human judgment", and an inheritable history of attempts, failed approaches and open questions | 2, 5 |
| R7 | **Combined-candidate verification**: verify several passing changes merged together before landing; evidence staleness when code or environment moves | 3, 5 |
| R8 | Decision ledger queries with supersede and challenge links | 6 |
| R9 | Evidence-linked system map built on the code graph and units; intention-level conflict hints | 1, 3 |
| Later | Working worlds with services and data; withdraw-with-dependents and recovery plans; a hosted fleet on microVMs | 4, 7 |
