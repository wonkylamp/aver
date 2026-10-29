# Aver verify action

This GitHub Action verifies a pull request or a push with Aver. It imports the base and head commits, runs the repository's checks on both natively on the runner, and produces a signed receipt. The receipt names a verdict and attributes every failure to a cause. The action posts the summary as a pull request comment and as the job summary, and uploads the receipt as an artifact.

## Quickstart

Add `.github/workflows/aver.yml`:

```yaml
on: pull_request
permissions:
  contents: read
  pull-requests: write
jobs:
  aver:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - uses: wonkylamp/aver/action@v0.4.2
```

`aver init --ci` writes a complete workflow for your repository instead: the verify, base record, merge queue and approve jobs, with the steps that install its dependencies ([docs/CI.md](../docs/CI.md#set-up-in-5-minutes)). The checkout's `ref:` is the pull request's head, so the checks run in the job's checkout rather than GitHub's test merge commit. Pin the action to a full commit SHA in production, as the checkout line does, and keep the tag in a comment. To use the action from a private copy of this repository, allow other repositories of the same owner to use its actions (Settings, Actions, General, Access: "Accessible from repositories owned by the user", or `gh api -X PUT repos/<owner>/<repo>/actions/permissions/access -f access_level=user`); the action then builds aver from that copy's source, as [Security](#security) describes. [examples/aver-verify.yml](examples/aver-verify.yml) is a complete workflow that also verifies pushes to `main`.

## Inputs

| Input | Default | Description |
|---|---|---|
| `mode` | `verify` | `verify` runs `aver ci`. `approve` runs `aver ci approve` on a `pull_request_review` event and no check: when a person other than the author approved the pull request's current head commit, it posts the `Aver` check run as `success` on that commit (needs `checks: write`). See [Approving a REVIEW](#approving-a-review). |
| `review` | `fail` | What exit 3 (`REVIEW` or `UNPROVEN` under `fail-on: unexplained`) does: `fail` fails the step, titled `Aver: needs maintainer review`; `neutral` keeps the job green and posts a neutral check run; `approve` keeps the job green and posts a check run named `Aver` for every verdict (`success`, `failure`, or `action_required` for exit 3), which branch protection requires and `mode: approve` clears. `neutral` and `approve` need `checks: write`. |
| `check` | empty | Run only this `[[check]]` of the base's `aver.toml` (`aver ci --check`), as one job of a matrix. The receipt covers that check alone, the artifact is `aver-receipt-<check>`, and with `review: approve` the check run is `Aver (<check>)`. An unknown id fails the step and names the known ones. |
| `fail-on` | `unexplained` | When the step fails. `unexplained` fails on `FAIL_OWN`, and on any `BLOCKED_NOT_OWN` the base does not explain: only tests that failed or were flaky on the base explain a block, and a check that ended in infra (`ENV`) on either side always fails the step (docs/VERDICTS.md); `REVIEW` and `UNPROVEN` exit 3, which `review` decides. `fail-own` fails only on `FAIL_OWN`. `not-pass` fails on any verdict except `PASS`, `REVIEW` included. `never` never fails on a verdict. On `merge_group` events the action ignores it and runs `aver ci --regression-only`. |
| `comment` | `true` | Create or update one pull request comment with the receipt summary. |
| `viewer-url` | empty | URL of an Aver receipt viewer. The summary links to it. |
| `aver-version` | empty | Release tag of aver to install. Empty means the action's own ref. If that ref has no release (a branch or a SHA), the action builds aver from source with cargo. |
| `working-directory` | `.` | Path of the Git repository to verify. |
| `github-token` | `${{ github.token }}` | Token for the comment step only. |
| `base` | empty | Base commit. Required for events other than `pull_request`, `merge_group` and `push`. |
| `head` | empty | Head commit. Required for events other than `pull_request`, `merge_group` and `push`. |

For `pull_request`, the action compares `pull_request.head.sha` with `pull_request.base.sha`. For `merge_group`, it compares `merge_group.head_sha` with `merge_group.base_sha` in regression-only mode: the pull request's criteria and any review were the pull request check's business, so only `FAIL_OWN` and an unexplained `BLOCKED_NOT_OWN` fail the queue, and `REVIEW` and `UNPROVEN` pass. For `push`, it compares `after` with `before`. A push that creates a branch has no earlier commit, so the action compares the branch with its merge base with the repository's default branch, fetched from `origin`; when the branch has no commits beyond it, or no history in common, the action skips verification, writes a notice and succeeds. If the checkout is shallow, the action fetches the missing commits from `origin` with the checkout's credentials, so keep the default `persist-credentials` of `actions/checkout`. It also fetches the history down to the merge base of the two commits: when the base is not an ancestor of the head (the base branch moved on after the pull request was last updated), `aver ci` compares the head with that merge base, the commit the branch forked from, and the receipt records both commits.

## Outputs

| Output | Description |
|---|---|
| `verdict` | `PASS`, `FAIL_OWN`, `BLOCKED_NOT_OWN`, `UNPROVEN` or `REVIEW`. Empty if aver failed or the run was skipped. |
| `receipt-id` | The receipt id, `rcpt_…`. Empty if aver failed or the run was skipped. |
| `exit-code` | `0` if the verdict is acceptable under `fail-on`, `1` if it trips `fail-on`, `2` on an error, `3` for `REVIEW` or `UNPROVEN` under `fail-on: unexplained` (`0` with `review: neutral` or `approve`). Empty if the run was skipped, and in `mode: approve`. |
| `approved` | `mode: approve` only: `true` when an approval bound to the pull request's current head commit, else `false`. |

The action's last step exits with `exit-code`. The comment and the artifact upload run before it, so a failing verdict still gets both.

## Acceptance criteria

Without criteria, the action runs in regression-only mode. The verdict is `PASS` unless a check broke on head (`NEW`), the infrastructure failed (`ENV`), or the pull request changed test expectations or harness (`CHANGED`, verdict `REVIEW`).

To declare criteria, add a fenced code block whose info string is `aver` to the pull request description:

````
```aver
- div-basic: divide returns the quotient
- div-zero: divide throws on division by zero
protected: test/**
protected: package.json
```
````

- `- <id>: <text>` declares a criterion. An id matches `[A-Za-z0-9][A-Za-z0-9._-]{0,63}` and is unique. A test proves a criterion when its name contains `[crit:<id>]`, for example `test('[crit:div-zero] divide throws on division by zero', …)`.
- `protected: <glob>` names a path the pull request may not add, change or delete. `*` and `?` match within a path segment, and `**` matches whole segments. A directory covers everything under it. Changing a protected path is a `NEW` failure, so the verdict is `FAIL_OWN`.
- Blank lines are ignored. Any other line, a duplicate id, an empty block, or more than one `aver` block is an error (exit code 2). The error names the line.

The verdicts are as follows:

| Verdict | Meaning |
|---|---|
| `PASS` | No check broke and every criterion has a passing proof. |
| `FAIL_OWN` | The change broke something: a check failed on head that passed on base, or a protected path changed. |
| `BLOCKED_NOT_OWN` | Infrastructure or flakiness stopped verification. The change is not at fault. |
| `UNPROVEN` | A criterion has no passing proof. |
| `REVIEW` | Nothing broke and every criterion is proven, but the change rewrote test expectations or harness (`CHANGED` entries). A maintainer reviews them. `UNPROVEN` outranks it. |

Each failure has a cause: `NEW` (broke on head), `PREEXISTING` (also fails on base), `FLAKY` (passes and fails on the same code), `ENV` (infrastructure, or a dependency the pull request added whose install script cannot build with the network off) or `CHANGED` (the base's copy of a test the pull request modified or deleted fails, and the pull request's own version passes or is gone; or a frozen harness edit).

### `REVIEW` and `UNPROVEN`: needs maintainer review

With the default `fail-on: unexplained`, `aver ci` exits 3 on `REVIEW` (the pull request rewrote test expectations or harness) and `UNPROVEN` (an unproven criterion, or a changed source file no check covers): nothing broke, and nothing proved the change safe. By default (`review: fail`) the step fails, titled `Aver: needs maintainer review (REVIEW)` or `(UNPROVEN)`, and the job summary says why, so a required check holds the merge until a maintainer looks. With `review: neutral` the job stays green and a later step posts a check run `aver-review` with conclusion `neutral` and that title; the job then needs `permissions: checks: write` (without it the step warns and carries on). The token is used only in that step, never in the one that runs the pull request's code.

`fail-on: not-pass` fails the step on `REVIEW` and `UNPROVEN` with exit 1; `fail-on: fail-own` passes them.

### Approving a REVIEW

To let a maintainer clear `REVIEW` and `UNPROVEN` by approving the pull request, run the verify job with `review: approve`, require the check named `Aver` in branch protection, and add a job on `pull_request_review` with `mode: approve`. [examples/aver-approve.yml](examples/aver-approve.yml) is the complete workflow.

The approve job runs `aver ci approve`, which reads the pull request and its reviews with `gh api` and runs none of the pull request's code. An approval binds only when it is the reviewer's latest decision, its commit is the pull request's current head, the reviewer is not the author, the account is a person's (type `User`, login not ending in `[bot]`), and the reviewer can push to the repository (`OWNER`, `MEMBER` or `COLLABORATOR`). Then the job posts `Aver` as `success` on that commit and uploads `approval.json` (reviewer, review id, head commit and tree) as the `aver-approval` artifact. Otherwise it stays green with a notice saying why. A push after the approval is a new head commit: the verify job runs on it and posts a new `Aver` check run, and the approval of the older commit does not bind. With a `check` matrix, give the approve job the same matrix, so it clears each `Aver (<check>)`.

Fork pull requests get a read-only token on both events, so neither job can post a check run; use `review: fail` for repositories that take pull requests from forks.

## How the pull request is judged

The harness comes from the base, so a pull request cannot make itself pass by weakening it:

- Regressions are judged with the base's copy of every existing test file. A pull request may edit, delete or skip existing tests, but the base's versions still run against its code, so a test the change broke still fails as `NEW`. When the pull request modified or deleted the file that defines such a test and its own version passes (or is gone), the entry is `CHANGED` instead: a rewritten expectation that a maintainer reviews, so the verdict is `REVIEW` rather than `FAIL_OWN`. The pull request's own versions of the tests run separately and are what prove criteria, so a `[crit:<id>]` case added to an existing test file works.
- Dependencies come from the pull request: adding or upgrading a package in `package.json` and the lockfile installs for the head's runs (with the base's install command, scripts disabled unless the base's `aver.toml` allows them). The base keeps its own dependencies.
- Frozen harness comes from the base: `aver.toml`, `.npmrc`, test runner configuration files, and the `scripts`, `config`, `type` and test runner fields of `package.json`. Changing one is a `CHANGED` entry (`judging::<path>` or `judging::package.json#<field>`) for a maintainer to review, and while one exists no criterion is proven: change the harness in a pull request of its own.

The comment starts with **What this PR did to the tests**: test files added, modified and deleted, existing tests the pull request removed or skipped and how many of them now fail from the base's copy, and any frozen harness it changed. It also says how the checks ran (on the runner, as a separate user or the runner's own) and when the base has no `aver.toml` and the default `npm test` check ran.

## View the receipt

The action uploads an artifact named `aver-receipt` with four files:

- `receipt.json` is the signed receipt (`aver.receipt/v2`).
- `diagnostics.json` explains each failure and unproven criterion (location, output excerpt, reason, closest test names). It is not signed.
- `summary.md` is the summary that the comment and the job summary show.
- `public-key.txt` is the verifier's public key, 43 characters of base64url.

To check the signature, open the Aver receipt viewer (`site/receipt.html` in this repository, or wherever you deploy it). Drop `receipt.json` on it and paste the key from `public-key.txt`. The viewer also reads `?url=<receipt URL>` and `?key=<public key>` from its address. GitHub artifact URLs need authentication, so `?url=` works only for a receipt you host elsewhere.

The action creates a new Aver home, and so a new verifier key, for every run. A valid signature shows that the receipt has not changed since that run signed it. It does not identify the machine that ran it.

## Base records

On a pull request, the base side of every check usually reran a commit the default branch's CI already ran. With base records, a push to the default branch (and a `merge_group` run) records the head's runs with `aver ci --record` and saves them with `actions/cache/save`. A later pull request on that commit restores them with `actions/cache/restore` and passes `aver ci --base-record`, so its base side starts no run. The workflow needs both events, as [examples/aver-verify.yml](examples/aver-verify.yml) has. A `merge_group` run passes `base` and `head` as inputs.

The cache key comes from `aver ci keys`: a hash of the runner (OS, architecture, `ImageOS`, `ImageVersion`, the running workflow file, the Aver version and its executor id), the commit's Git tree, and the head's files that the contract's `deps.inputs` name. The pull request restores only an exact key match, with no prefix fallback. aver checks the record's runner, tree and dependency inputs again, rehashes every object in it, and ignores a record that does not match with one line on stderr; the base then runs as before. A `NEW` failure that rests only on recorded base runs gets one fresh base run first. When the fresh base fails the same way, the entry is `PREEXISTING`.

**Trust model.** GitHub scopes caches by ref:

- A cache written on the default branch is readable by every pull request into it. That is what a base record is for, and only workflows that run on the default branch can write there.
- A cache written by a pull request's run is scoped to that pull request's merge ref. Other pull requests and the default branch cannot read it, so one pull request cannot plant runs in another's base.
- A fork's pull request cannot write the default branch's cache.
- A `merge_group` run's cache is scoped to the merge queue's temporary ref, so pull requests cannot restore it. Pull requests restore the record of the push that lands on the default branch afterwards.

A record therefore holds runs that the repository's own default-branch CI made. It saves time and never decides a failure alone: a new failure is checked on the base again before it is excused.

## Security

**Use `pull_request`, never `pull_request_target`.** A `pull_request_target` workflow runs in the base repository's context, with a write token and the repository's secrets. This action executes the pull request's code, because it runs its tests. Running untrusted code next to a write token and secrets is the "pwn request" pattern. The action refuses to run on `pull_request_target`.

**Fork pull requests get no comment.** GitHub gives `pull_request` workflows from forks a read-only token. The action detects a fork and skips the comment with a notice. The job summary and the artifact still carry the receipt. If the token cannot comment for another reason (HTTP 403), the action also skips the comment with a notice and the job does not fail. The action updates only a comment written by a bot account that starts with the hidden marker `<!-- aver-receipt -->`. Anyone can post a comment with that marker, and updating theirs would let them rewrite the verdict later.

**What runs where.** Everything runs on the repository's own runner. aver imports commits with Git plumbing and runs the checks in the job's own checkout, which it swaps to each side's tree and puts back at the head afterwards, untracked files included; an `if: always()` step runs `aver ci restore` in case the job was cancelled mid-run. Checks run natively on the runner with a cleaned environment (no inherited variables, so no tokens), the base's harness, the base side before the head, and the network off where the runner allows; as a separate user when passwordless sudo works, so the code under test cannot read the throwaway verifier key or other processes' environments. Before native checks run, the action removes the token `actions/checkout` persisted in `.git/config`, in every submodule's configuration under `.git/modules`, and in the credentials file under `$RUNNER_TEMP` that checkout v6 and later have those configurations include (it is only needed for fetching the commits); it refuses a job whose Git configuration still names a credential header it cannot remove, such as the global configuration's, and a job that holds `id-token: write`. This is the trust a repository's own CI already gives a pull request's code. No sandbox isolates the checks; the receipt's `provenance` says `ci`, and `uid` `separate` or `same`. The token is used only by the comment step. The action never passes it to aver or to the checks.

**Downloads are verified.** The aver binary comes from this repository's GitHub release and the action checks it against the release's `.sha256` file. When the ref has no release asset (a branch, a commit SHA, or a private repository, whose releases answer 404 to the runner's unauthenticated download), the action builds aver with `cargo build --locked` from the source that the runner already downloaded for the action itself: for `uses: owner/repo/action@ref` that is the whole repository at that ref, so the binary matches the pinned commit and no second clone or token is needed. The build takes several minutes the first time; `actions/cache` keeps the cargo registry and `target/` for later runs. Nothing downloaded is piped into a shell.

## Limitations

- Aver verifies Node and TypeScript (npm), Python, Rust and Go projects. The default checks are `npm test` (with per-test results when the test script runs `node --test`, Vitest, Jest, Mocha or tap), `pytest`, `cargo test` and `go test` for each package Aver finds, and `aver.toml` can change them. A Rust change builds on both sides, so allow for that in the job's timeout.
- The checks run with the runner's own toolchains, so the runner needs the project's Node, Python, Rust or Go, as for running its tests. The action installs release binaries on Linux runners and builds aver from source elsewhere.
- The action compares head with the merge base of the event's `base.sha` and the head, so test changes that landed on the base after the branch forked are not counted against the pull request. They are not tested with it either: update the branch to verify the change against the current base.
- Each run uploads an artifact named `aver-receipt`, so two uses of the action in one workflow run conflict. Use it once per workflow run.
