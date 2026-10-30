# Verify pull requests in CI

`aver ci` verifies a pull request's head against its base and writes a signed receipt. The `aver-verify` GitHub Action runs it on every pull request and posts the result as a comment. [INTERFACE.md](INTERFACE.md) is the full contract for `aver ci`, and [action/README.md](../action/README.md) lists the action's inputs.

## Set up in 5 minutes

In the repository, with Aver installed:

```bash
aver init --ci          # shows aver.toml (when there is none) and .github/workflows/aver.yml
aver init --ci --yes    # writes them
git add aver.toml .github/workflows/aver.yml && git commit -m "Verify pull requests with Aver"
```

The workflow runs the `aver-verify` action on four events. On `pull_request` it verifies the head against the base and posts a check run named `Aver` (`review: approve`). On `pull_request_review`, a maintainer's approval of the head commit clears `REVIEW` or `UNPROVEN` (`mode: approve`). On a push to the default branch it records its runs, so pull requests on that commit skip their base. On `merge_group` it fails only on a regression. Each job has the least `permissions:` it needs, checks out the pull request's head with full history and no persisted credentials, and every `uses:` is pinned to a commit. Before the action, steps install the toolchains and dependencies the repository's lockfiles name (Node with `actions/setup-node` and `npm ci`, `pnpm` or `yarn`; Python with uv, `uv sync --frozen` or `uv pip install -r requirements*.txt`, its virtualenv on `PATH`; Rust with `rustup` and `cargo fetch --locked`; Go with `go mod download`), so the checks find them.

Then require the check named `Aver` on the default branch (with the action's `check` input in a matrix, each `Aver (<check>)`), and turn on the merge queue in the same rule if you want one. `aver init --ci` prints the `gh api` command that sets the rule; it never runs it. [CLI.md](CLI.md#drafting-avertoml-aver-init) has the flags.

## GitHub Action

The `aver-verify` action verifies every pull request on your own GitHub runners and posts the receipt as a comment. It is new, and it verifies Node and TypeScript, Python, Rust and Go projects. Add `.github/workflows/aver.yml`:

```yaml
on: pull_request
permissions:
  contents: read
  pull-requests: write
jobs:
  aver:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - uses: wonkylamp/aver/action@v0.4.2
```

Without criteria, the run is regression-only: the verdict is `PASS` unless a check fails only with the change (`NEW`), for infrastructure reasons (`ENV`), or in a way that hides the change (a check with no per-test results that fails on both sides is `BLOCKED_NOT_OWN`). To declare criteria, put an `aver` block in the pull request description:

````
```aver
- div-basic: divide returns the quotient
- div-zero: divide throws on division by zero
protected: test/**
```
````

A test proves a criterion when its name contains `[crit:<id>]`, it fails without the change and it passes in the pull request's own version of the tests; it may be a new file or a case added to an existing one. Regressions are judged with the base's copies of existing tests, so deleting or skipping a test cannot hide a failure, and the summary reports what the pull request did to the tests. Dependencies come from the pull request. Changing a `protected` path is a `NEW` failure, and so is changing frozen harness: `aver.toml`, `.npmrc`, test runner configuration, or the `scripts` and test runner fields of `package.json`. The step fails on `FAIL_OWN`, on a `BLOCKED_NOT_OWN` the base does not explain, and on `REVIEW` and `UNPROVEN` ("Aver: needs maintainer review"; `review: neutral` posts a neutral check run and keeps the job green instead, and `review: approve` posts a required `Aver` check run that a maintainer's approval clears, below), by default (`fail-on: not-pass` fails on anything but `PASS`; `fail-on: fail-own` only on `FAIL_OWN`). A skipped or deleted test the change breaks is `FAIL_OWN`. [action/README.md](../action/README.md) lists the inputs, explains the security model (never `pull_request_target`; fork pull requests get no comment) and the limitations.

The action is a thin wrapper around `aver ci`, which runs the checks natively wherever the project's toolchain is installed:

```bash
aver ci --repo . --base origin/main --head HEAD --pr-body description.md
```

It imports both commits into a throwaway home, verifies head against base and writes `aver-out/receipt.json`, `diagnostics.json` (why each failure and unproven criterion happened; not signed), `summary.md` and `public-key.txt`. The receipt (`aver.receipt/v3`) records both commits, the configuration, the criteria and an optional `--subject`, and its `reproduce` line is an `aver ci` command anyone can rerun. It exits 0 when the verdict is acceptable, 1 when it trips `--fail-on`, 3 under the default `--fail-on unexplained` for `REVIEW` and `UNPROVEN` (nothing broke, nothing proved the change safe: a maintainer decides), and 2 on an error ([VERDICTS.md](VERDICTS.md#exit-codes-and-ranking) has the table per mode). Before it runs it warns on stderr when the base's `go.mod` or `Cargo.toml` asks for a newer Go or Rust than the runner has. The verifier key is new for every run, so the signature shows the receipt is unchanged since the run, not who ran it. To sign with a key you publish, keep a home with `--home <dir>` (with `--run-as <user>`, so the checks cannot read its key) and print its key with `aver key --home <dir>`; concurrent runs on one home wait for each other. `diagnostics.json` also records the time each run took. See [ARCHITECTURE.md §2.9](../ARCHITECTURE.md).

When `--repo` is a checkout at the head commit with no changes to tracked files (a CI job's checkout), the checks run in it, one at a time: the dependencies the job installed and its build caches are used where they are, and nothing is cloned. Aver moves the checkout's untracked files aside, except installed dependencies: `node_modules`, `.venv` and an untracked `venv` at any depth stay in place whatever installed them (npm, pnpm, yarn, bun, uv, pip), as do the dependency directories of `aver.toml`'s layers, and `--slots` and local workspaces clone them the same way. So does a Python version file the install generated from Git where the base's `pyproject.toml` names it (setuptools-scm's `version_file` or `write_to`, hatch-vcs's `version-file`, such as black's `src/_black_version.py`): an editable install imports it from the checkout, so without it every test module failed to import on both sides. Only the base's `pyproject.toml` counts, so a head cannot keep a file of its own for the base's runs. `--slots` and local workspaces do not keep it. Before WP6 only an npm lockfile kept `node_modules`, so a pnpm or lockfile-less checkout lost its dependencies on the swap and every check failed with a missing command, which read as not the change's fault. Aver then swaps the checkout to each side's tree and puts it back at the head, untracked files included, when it ends, also on an error or on Ctrl-C or SIGTERM. A run that was killed outright leaves `.git/aver-swap.json`; `aver ci restore [--repo DIR]` puts the checkout back (the action runs it in an `if: always()` step), and the next `aver ci` there does it first. `--slots` runs the checks in slots cloned from the checkout instead; any other `--repo` uses slots and says why on stderr. On `pull_request` events `actions/checkout` checks out GitHub's test merge commit (`refs/pull/<n>/merge`) unless told otherwise, and `aver ci` verifies the head, so with the default checkout the checks run in slots ("the checkout is at <merge>, not at --head <head>"). Every workflow here, and the one `aver init --ci` writes, checks out `ref: ${{ github.event.pull_request.head.sha }}` (with `|| github.sha` where the job also runs on pushes), so the checks run in place with the job's installed dependencies. Push and `merge_group` checkouts are at the head.

Under `--run-as <user>` every check runs as that user with a `HOME` of its own, in the checkout handed to it, so the user has to reach the checkout and the toolchains the job installed. On GitHub's Ubuntu images both sit in the runner's home, which other users cannot enter (mode 0750): without access every check fails to start on both sides (`sh: 1: cargo: Permission denied`, `BLOCKED_NOT_OWN`). The action's `run-user.sh` creates `aver-run` when the runner has passwordless sudo. Where a directory on the way is closed to other users like that, it lets `aver-run` search (POSIX ACLs, `setfacl` from the `acl` package; without it the action warns) each directory from there down to the checkout, `~/.rustup` (or `RUSTUP_HOME`), `~/.cargo` (or `CARGO_HOME`), uv's Python installs (`~/.local/share/uv/python`) and the tool cache (`RUNNER_TOOL_CACHE`, where a self-hosted runner keeps `actions/setup-node`'s Node), and denies it every other entry of those directories: the runner's credentials under `~/actions-runner`, `$RUNNER_TEMP` (the steps' scripts, `GITHUB_ENV` and the other job files) and the rest of `~/.local` and `~/.cache` stay out of its reach. Nothing under `$RUNNER_TEMP` is opened, a toolchain there included. The denies are set first, and when one fails nothing is opened and what an earlier job on the runner opened is closed again. A home other users can already enter (0755, as on Debian) gets no ACLs. The action then runs `aver ci` with `RUSTUP_HOME` and `CARGO_HOME` set to the runner's when the job left them unset, so rustup finds the job's toolchain and cargo the crates `cargo fetch` downloaded (read-only to the check: the job's `cargo fetch --locked` has to fetch every crate the checks build, so for a pull request that changes `Cargo.lock` the base side finds only the crates both lockfiles share unless the job also fetches the base's); with `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME` and `XDG_STATE_HOME` unset where they point into the runner's home (the images set `XDG_CONFIG_HOME=~/.config`, where pnpm and Yarn stop at `EACCES`), so those follow the check's own home; and with only the `PATH` entries `aver-run` can search, since one it cannot (`~/.dotnet/tools` on the images) turns a missing command's `not found` into `Permission denied`, which Aver does not read as a missing tool (`PATH` stays whole when the filter would hide the `git` or `sudo` Aver itself runs). Node, npm, pnpm and Yarn through corepack, pytest in a uv `.venv` on a uv-managed Python, Go and cargo were measured to run this way on `ubuntu-latest`. Not opened, by design: Go's module and build caches (`~/go`, `~/.cache/go-build`; `go test` downloads the modules again into the check's home), corepack's cache (pnpm and Yarn are downloaded again), pip's `--user` installs (`~/.local/lib`), what other setup actions install in the home or under `$RUNNER_TEMP` (such as `pnpm/action-setup`'s `~/setup-pnpm`), and an `.npmrc` that `actions/setup-node` writes under `$RUNNER_TEMP` for `registry-url` (npm runs without it, so installs belong before `aver ci`). On a self-hosted runner the ACLs stay after the job; they name only `aver-run`, and the next job clears them from what it opens. Running `aver ci --run-as` outside the action, give the user the same access and set `RUSTUP_HOME` and `CARGO_HOME` yourself.

`--regression-only` is for merge queues: it reads no criteria and sets `--fail-on regressions`, which fails only on `FAIL_OWN` and an unexplained `BLOCKED_NOT_OWN` and exits 0 on `REVIEW` and `UNPROVEN`, which the pull request's own check already asked a person about. The action uses it on `merge_group` events.

With base records, a pull request's job does not run the base side again when the default branch's CI already ran that commit on the same runner image, workflow and dependency inputs: the push runs `aver ci --record <dir>` and saves the directory with `actions/cache`, and the pull request restores it under the key `aver ci keys` prints and passes `aver ci --base-record <dir>`. The action does this for `push` to the default branch, `merge_group` and `pull_request`. [INTERFACE.md §1.1](INTERFACE.md) has the record and key format and [action/README.md](../action/README.md) the trust model.

Run locally, `aver ci` keeps base records in your home by default: the base's runs are written to `$AVER_HOME/records/<key>` after each run and read back by the next run against the same base tree, runner and dependency inputs, so a second `aver ci` on the same base runs only the head side ("aver: reused N base runs from the home's record" on stderr). It does this only outside CI: when any of `CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `BUILDKITE`, `CIRCLECI`, `JENKINS_URL`, `TF_BUILD` or `TEAMCITY_VERSION` is set (to anything but empty, `0` or `false`), and whenever `--record` or `--base-record` is given, the home is not touched. `--fresh` reads no record but writes one; `--no-records` neither reads nor writes. A head failure that rests on a record alone, whether the record passed that test or failed it too, gets one fresh base run before it is judged. `aver gc` removes records unwritten for 14 days and all but the 64 newest.

To run the action itself end to end on your machine, with no network, run `make test-e2e-local` (`action/test/e2e-local.sh`). It runs the steps of `action/action.yml` in order with the real action scripts and a release build of `aver`, against a bare Git repository as `origin` and a mock GitHub API on localhost, for a passing pull request, a regression, a `REVIEW` under each `review` input, a push that records the base and a pull request that reuses it, a merge queue and `mode: approve`, and checks each job's exit, outputs, summary and the API requests it made.

## Approving a REVIEW in CI

`aver ci` exits 3 on `REVIEW` and `UNPROVEN` under the default `--fail-on unexplained`: nothing broke, and nothing proved the change safe. A person decides. Make the decision a pull request approval:

1. Run the verify job with `review: approve`. It posts a check run named `Aver` on the head commit for every verdict: `success`, `failure`, or `action_required` for exit 3. The job itself stays green on exit 3.
2. Require the `Aver` check in branch protection (not the job).
3. Run a second job on `pull_request_review` with `mode: approve`. It runs `aver ci approve`, which reads the pull request's reviews through `gh api` and runs no check. When a person other than the author approved the current head commit, it writes `approval.json` (reviewer, review id, head commit and tree) and posts `Aver` as `success` on that commit.

A push after the approval moves the head: the verify job runs again on the new commit, and the old approval does not bind to it. An approval by the author, by a bot account, or by someone without write access to the repository (`author_association` other than `OWNER`, `MEMBER` or `COLLABORATOR`) never binds. [action/examples/aver-approve.yml](../action/examples/aver-approve.yml) is the complete workflow. A pull request from a fork gets a read-only token, so neither job can post a check run there.

`aver ci approve --github-event <payload> [--repo <dir>] [--out-dir <dir>] [--receipt <id>]` exits 0 when an approval binds, 3 with one line `NOT APPROVED: <why>` when none does, and 2 on an error (no `gh`, an API error, or a head commit the checkout lacks).

## One check per job

`aver ci --check <id>` runs only that `[[check]]` of the base's `aver.toml`, so a workflow matrix can run each check in its own job. The receipt covers that check alone: it has no entry of another, its `reproduce` line repeats `--check <id>`, and `summary.md` says so. An id the base does not define exits 2 and names the ones it does. The action's `check` input passes it and names the artifact `aver-receipt-<id>`.
