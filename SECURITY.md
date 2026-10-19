# Security policy

## Reporting a vulnerability

Report vulnerabilities privately. Do not open a public issue, discussion or pull request for one.

- **GitHub private vulnerability reporting.** On [github.com/wonkylamp/aver](https://github.com/wonkylamp/aver), open the Security tab and choose Report a vulnerability. Only maintainers can read the report.
- **Email.** security@aver.build, once that address is live. Until then, use GitHub.

Include the Aver version (`aver --version`) or commit, the OS, the toolchain versions the checks ran with (`aver doctor` lists them), and the smallest repository or command sequence that reproduces the problem. A receipt or `diagnostics.json` helps when the problem is a wrong verdict.

We aim to acknowledge a report within 3 working days and to agree on a fix and a disclosure date within 14. We credit reporters in the release notes unless they ask us not to.

## Supported versions

Aver is before 1.0. Only the latest release and `main` receive security fixes.

## Scope

In scope:

- **Leaks into native check runs.** Host environment variables, the user's Cargo, Git, npm or pip configuration, or a version-manager shim reaching a check run, or a run left behind into a later one.
- **Forged or wrong verdicts.** A change that gets a `PASS` receipt it should not, by editing the harness, the environment, `aver.toml`, test reports or anything else the head controls. A receipt that verifies with a signature Aver did not make.
- **Authorization between agents.** An agent acting on another agent's workspace or attempt, or escalation between agent capabilities.
- **Key exposure.** The CLI revealing a private key, or accepting a key file others can read.
- **Prompt injection across the data boundary.** Repository content (files, test names, failure output) reaching an agent outside a fenced block in the CLI's agent reports, or outside a `content`, `diff`, `detail` or `output` field in JSON output.
- **`aver ci` and the GitHub Action.** Running untrusted pull request code with write tokens or secrets, or a workflow path that defeats the action's refusal of `pull_request_target`.
- **Supply chain.** `site/install.sh`, the release workflow and the checksums it publishes, and the receipt viewer on aver.build.

Out of scope:

- **Agent CLIs started by `aver run`.** They run on the host as you, in a copy of the repository, and Aver does not sandbox them. Claude is bounded by `acceptEdits` and a per-directory allowlist (none with `--yolo`), Codex by its own sandbox, and a custom `--agent-cmd` by nothing. This is documented behavior, not a vulnerability, unless an agent escapes the bounds Aver says it sets.
- Attacks that need control of the host or `$AVER_HOME` already.
- **Code under test reaching the host.** Checks run natively as you, in clean copies with a cleaned environment and the network off where the system allows. They are not sandboxed, so a test that reads your files or, where the network stays on, the network does what the same test would do in your own shell.
- Denial of service by a holder of the admin key.
- Findings from automated scanners without a demonstrated impact.

## Threat model in brief

Aver treats code under test and repository content as hostile, and registered agents as authenticated but fallible and possibly adversarial. The user's own `aver` processes are trusted and hold the key that signs receipts. Aver opens no network listener.

Every check runs natively, in a slot cloned for its side and swapped to the tree under test, never in an agent's directory. It gets the caller's environment minus the CI tokens and job files `place::proc::STRIPPED` names, with a private `TMPDIR` per run, and the network on. Whatever a run leaves running is killed when it ends. In CI, `aver ci --run-as` runs the checks as a separate user.

The head of a change runs with the base's copy of the harness, so a change cannot make its own tests easier to pass.

The full model is in [ARCHITECTURE.md §6](ARCHITECTURE.md#6-security-model): trust boundaries (§6.1) and keys and identity (§6.3). The slot runner is §2.4.6. The tests that hold it to its environment are in `crates/aver/tests/place.rs`, and any change to the environment handling in `crates/aver/src/place/proc.rs` needs a test there.
