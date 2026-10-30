# Releasing Aver

A release starts when a pull request that changes the version in `Cargo.toml` merges to `main`. Once `ci-ok` passes on the merged commit, `ci.yml` tags it `v<version>` and starts `.github/workflows/release.yml`, which does the rest. Nobody pushes a tag by hand. Nothing is published until every binary has been built, smoke-tested and installed on a clean runner.

## Steps

1. Open the release pull request from an up-to-date `main`:

   ```bash
   scripts/release.sh 0.4.2
   ```

   On a new branch `release/v0.4.2`, it sets the version in `Cargo.toml` and `Cargo.lock`, the action refs the docs show (`wonkylamp/aver/action@v0.4.2`, `AVER_VERSION=v0.4.2`), the README's version badge and the `aver init --ci` goldens. Then it runs `make ci-local`, commits, pushes the branch and opens the pull request with `gh`. It needs a clean tree. `--local` stops after the commit. `RELEASE_CHECK=<cmd>` replaces `make ci-local`, for a rehearsal.

2. Merge the pull request once its checks pass. CI runs again on the push to `main`. When its `ci-ok` passes, the `tag` job tags the merged commit `v0.4.2` and dispatches the Release workflow on that tag.

3. Watch the Release run. When it finishes, the release is public, `site/install.sh` and `action/install-aver.sh` download it, the Homebrew tap names it, `v0.4` points at it and a pull request bumps the Claude Code plugin's version. Merge that pull request.

A merge that does not change the version tags nothing. Use the version a prerelease should have (`0.5.0-rc.1`) and it releases as a prerelease: no tap update, no series tag and no plugin bump.

## Which tool does what

| Piece | Done by | Why |
|---|---|---|
| Tagging a merged version bump | [release-plz](https://release-plz.dev) `release` (`release-plz/action` v0.5.139, release-plz 0.3.169), configured by `release-plz.toml`: `git_only`, no crates.io publish, no release pull requests, no GitHub release, tag `v{{ version }}` for the `aver` package only | It tags a version whose tag does not exist yet, through the GitHub API, and nothing else. `salsify/action-detect-and-tag-new-version` was the other candidate. Its repository is archived (last release v2.0.3, 2022), it runs on `node16`, and it pushes every local tag. |
| Refusing a version that went down | `packaging/release/version-check.sh`, before release-plz | release-plz only asks whether the tag exists. |
| Starting the release | `gh workflow run release.yml --ref v<version>` in ci.yml's `tag` job | A tag pushed with `GITHUB_TOKEN` starts no workflow, but a `workflow_dispatch` sent with it does. So no app token or deploy key is needed. |
| Building, smoke-testing, provenance, the draft, the install check, publishing | `release.yml` and `packaging/release/*.sh` | See below. |
| Homebrew formula | `packaging/release/update-tap.sh` (`update-formula.sh`) | |
| The action's series tag, the plugin version | `move-tag.sh`, `bump-plugin.sh` | |

[dist](https://github.com/axodotdev/cargo-dist) (v0.33.0) was evaluated to replace `release.yml`. It can build the same four targets (static musl through `musl-tools`), write sha256 checksums, attest and publish a Homebrew formula, but switching would cost more than it saves:

- It always packages a binary as an archive named `aver-<target-triple>.tar.xz`. It cannot ship the bare `aver-<os>-<arch>` files that `site/install.sh`, `action/install-aver.sh`, the Homebrew formula, binstall's overrides and every pinned copy of the action download.
- It creates the GitHub release with all its files in one step, either directly or by undrafting a draft immediately. So nothing can install the draft on a clean runner before it is public.
- Its workflow triggers only on a tag push or a `workflow_dispatch` that makes the tag itself. Hand-editing it (for the ci-ok gate, the smoke test or pinned actions beyond `github-action-commits`) needs `allow-dirty = ["ci"]`, after which `dist` no longer maintains it.
- It installs itself with `curl | sh` during the run.

## What the release workflow does

`release.yml` runs on a `v<major>.<minor>.<patch>[-<pre>]` tag pushed by hand or on a `workflow_dispatch` on such a tag (what the `tag` job sends). Series tags such as `v0.4` do not match the tag filter, so moving one starts nothing.

| Job | What it does | Fails or stops when |
|---|---|---|
| `gate` | Reads the version from `Cargo.toml` and checks the ref (`packaging/release/gate.sh`). | The run is not on a tag; the tag is not `v<version>`; the commit is not on `main`; it has no `ci-ok` check run from GitHub Actions, or that run did not succeed. |
| `build` | Four binaries: static musl Linux x86_64 and aarch64, macOS arm64 and x86_64 (`build.sh`), each smoke-tested on its runner (`smoke.sh`: `--version`, statically linked on Linux, `aver demo` showing `FAIL_OWN` then `PASS`). | A build, the static link or the demo fails. |
| `draft` | Checks every `.sha256`, writes `SHA256SUMS` (`sums.sh`), attests build provenance for each binary, and creates the draft release with generated notes or replaces a re-run's draft assets (`draft.sh`). | A binary is missing or its checksum is wrong; a release with the tag is already published. |
| `install` | On fresh Ubuntu x86_64 and arm64 and macOS runners, downloads the draft's assets and installs them with `site/install.sh` into an empty home, then smoke-tests the installed binary (`install-check.sh … draft`). | Any install or smoke test fails. The draft stays a draft. |
| `publish` | Publishes the draft as the latest release (or as a prerelease), then installs it again from the public download URLs, by tag and as `latest`. | Publishing fails, or the public download does not install. |
| `tokens` | Reports which optional secrets exist, with a notice for each missing one. | Never. |
| `homebrew` | Rewrites `Formula/aver.rb` in `wonkylamp/homebrew-tap` from the published `SHA256SUMS` and pushes it (`update-tap.sh`). Skipped without `HOMEBREW_TAP_TOKEN` and for prereleases. | The tap push fails. |
| `action-tag` | Moves the series tag (`v0.4`) to the released commit through the Git refs API (`move-tag.sh`), for `uses: wonkylamp/aver/action@v0.4`. Skipped for prereleases. | The API refuses. |
| `plugin` | Opens a pull request `release/plugin-<version>` that sets the version in `integrations/claude-code/.claude-plugin/plugin.json` and `.claude-plugin/marketplace.json` (`bump-plugin.sh`). Skipped without `RELEASE_PR_TOKEN`, for prereleases, or when the plugin already has the version or the pull request is open. | The push or the pull request fails. |

A series ref in `uses:` installs the release named by the action's own `Cargo.toml` version (`action/install-aver.sh`), not a build from source.

The `tag` job in `ci.yml` runs only on a push to `main`, after `ci-ok` passed (a red CI skips it). It needs `contents: write` for the tag and `actions: write` for the dispatch, and it runs one at a time (`concurrency: release-tag`). It is safe to repeat: an existing tag makes release-plz do nothing, and nothing is dispatched.

## Secrets

The release needs none: the tag and the dispatch use the workflow's own token. Two secrets are optional. Without them the release still publishes, the run shows a notice and the job shows as skipped.

| Secret | Used by | Needs |
|---|---|---|
| `HOMEBREW_TAP_TOKEN` | `homebrew` | A fine-grained token with contents write on `wonkylamp/homebrew-tap`. |
| `RELEASE_PR_TOKEN` | `plugin` | A fine-grained token with contents and pull requests write on `wonkylamp/aver`. The workflow's own token could push the branch, but pull requests it opens do not start CI. |

A tag ruleset that restricts who may create `v*` tags must let GitHub Actions create them, or the `tag` job fails.

Without a token, do the step by hand after the release:

```bash
TAP_TOKEN=<token> packaging/release/update-tap.sh 0.4.2
```

```bash
PR_TOKEN=<token> GH_REPO=wonkylamp/aver packaging/release/bump-plugin.sh 0.4.2
```

## When something fails

- **The `tag` job failed: version went down.** The merged version is lower than a released one. Merge a pull request that sets a higher version.
- **The `tag` job did not run.** `ci-ok` failed on the merged commit. Fix `main` (or re-run CI if it was a flake). A re-run of the CI run tags the commit once `ci-ok` passes.
- **The tag exists but no release ran** (the dispatch failed). Start it on the tag: `gh workflow run release.yml --ref v0.4.2`.
- **Gate refused.** The run is not on a tag, the tag and version differ, or `ci-ok` is missing or failed on the commit. For a hand-pushed tag on a commit CI never ran on, run CI on it (`gh workflow run ci.yml --ref v0.4.2`), wait for it to pass, then re-run the Release workflow. A tag that names the wrong version can be deleted (`git push origin :refs/tags/v0.4.2`), since nothing was published.
- **A build, install or publish job failed.** Fix the cause and re-run the failed jobs, or the whole run. A re-run replaces the draft's assets and never touches a published release, and `publish` does nothing on an already-published release. If the fix needs new code, delete the draft and the tag, then release the next patch version.
- **`homebrew`, `action-tag` or `plugin` failed after publishing.** The release is out. Re-run the failed job, or run its script by hand as above. Each is safe to repeat.

## Checking a release

Each binary carries signed SLSA build provenance from the `draft` job:

```bash
gh attestation verify aver-linux-x86_64 --repo wonkylamp/aver
```

`SHA256SUMS` in the release lists every binary. `site/install.sh` checks a download against it, and `action/install-aver.sh` checks against the binary's own `.sha256`.

`packaging/release-local.sh <commit>` builds the same four assets on a Mac, smoke-testing the macOS ones, for comparing a release or for releasing by hand when Actions is unavailable.

## Testing the release workflows

`action/test/workflows.sh` (part of `make test-action`) runs `ci.yml`, `release.yml` and `aver.yml` job by job against a mock GitHub API. The release flows cover a full release with both secrets, a re-run after a failed install, a prerelease, each gate refusal (a dispatch on a branch included), and a series tag starting nothing. As on GitHub, a job whose `if:` calls no status function (`always()`, `success()`, `failure()`, `cancelled()`) runs only when every job upstream of it succeeded, so one skipped job anywhere upstream skips it. In `ci.yml`, the steps of CI's own jobs are stubbed, but their `if:` still runs, so a push to `main` skips `docs`, `eval` and `test-native` as a real push does. `release-plz/action` is replaced by a stand-in with the same inputs and outputs (`action/test/release-plz-stand-in.py`). The flows check that a merge without a version change tags nothing, that a bump tags an annotated `v<version>` and dispatches the release (which then runs), that the same push again does nothing, that a bump with red CI tags nothing, and that a lower version fails. The real `packaging/release/*.sh` scripts run; `cargo` builds a stub binary. Each job may write only what its `permissions:` allow.

Only GitHub can show: real release-plz against the API, the dispatch starting a run, the real musl builds on the runners, the attestation, the visibility of drafts, and pushes made with real tokens.
