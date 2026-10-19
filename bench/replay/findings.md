# Replay study: what Aver must fix to work on real repositories

The pull requests come from AIDev (Hao Li, Haoxiang Zhang, Ahmed E. Hassan; https://huggingface.co/datasets/hao-li/AIDev, CC-BY-4.0), as listed in [manifest.json](manifest.json).

## Round 6.1 status

Round 6.1 acted on the FAIL_OWN triage below: a test whose base copy fails on the head while the head's own version of it passes (or was deleted) is `CHANGED`, a frozen-harness edit is `CHANGED` instead of `NEW`, a native dependency the head adds that the sandbox cannot build gives `ENV`, and the new verdict `REVIEW` (no `NEW`, no `ENV`, nothing unproven, some `CHANGED`) asks a person to review only the changed expectations (docs/VERDICTS.md). Only `NEW` entries can change cause, so only the 13 `FAIL_OWN` and 2 `BLOCKED_NOT_OWN` receipts of the round-6 rerun could change verdict; those 15 were rerun with the round-6.1 build on the same replay home ([results-r61.jsonl](results-r61.jsonl)). Every run was a cache hit from round 6, so the evidence is identical and only the rules differ.

| Pull request | Round 6 | Round 6.1 | NEW | CHANGED | ENV |
|---|---|---|---:|---:|---:|
| dotenvx/dotenvx#634 | FAIL_OWN | REVIEW | 0 | 9 | 0 |
| yamadashy/repomix#537 | FAIL_OWN | REVIEW | 0 | 11 | 0 |
| yamadashy/repomix#822 | FAIL_OWN | REVIEW | 0 | 9 | 0 |
| dotenvx/dotenvx#679 | FAIL_OWN | REVIEW | 0 | 4 | 0 |
| intellectronica/ruler#159 | FAIL_OWN | REVIEW | 0 | 4 | 0 |
| arabold/docs-mcp-server#152 | FAIL_OWN | REVIEW | 0 | 2 | 0 |
| paralleldrive/cuid2#104 | FAIL_OWN | REVIEW | 0 | 1 | 0 |
| jeremyckahn/rekapi#96 | FAIL_OWN | REVIEW | 0 | 1 | 0 |
| ekalinin/sitemap.js#455 | FAIL_OWN | BLOCKED_NOT_OWN | 0 | 1 | 0 |
| intellectronica/ruler#17 | FAIL_OWN | BLOCKED_NOT_OWN | 0 | 1 | 0 |
| arabold/docs-mcp-server#215 | FAIL_OWN | BLOCKED_NOT_OWN | 0 | 0 | 12 |
| intellectronica/ruler#37 | FAIL_OWN | FAIL_OWN | 22 | 0 | 0 |
| microsoft/poml#17 | FAIL_OWN | FAIL_OWN | 2 | 1 | 0 |
| lirantal/npq#375 | BLOCKED_NOT_OWN | BLOCKED_NOT_OWN | 0 | 0 | 0 |
| paralleldrive/cuid2#98 | BLOCKED_NOT_OWN | BLOCKED_NOT_OWN | 0 | 0 | 0 |

Across the 32 pull requests: PASS 16 (unchanged), REVIEW 8, BLOCKED_NOT_OWN 5, FAIL_OWN 2, no receipt 1 (round 6: PASS 16, FAIL_OWN 13, BLOCKED_NOT_OWN 2).

- All 7 test-change pull requests are `REVIEW`, each `CHANGED` entry with the diff of its test file in `summary.md`. cuid2#104 is command-level (riteway via `node`), so its `CHANGED` rests on the head-tests run of the whole check passing with the PR's `src/index-test.js`.
- The 4 benign harness edits: rekapi#96 is `REVIEW`; repomix#822's `prepare` script joins its test changes in `REVIEW`; sitemap.js#455 and ruler#17 are `BLOCKED_NOT_OWN`, the first because its only check fails on both sides without per-test results (native `unrs-resolver`), the second because 13 Jest timeouts under load are flaky on the base and failed on every head run (inconclusive). Neither is the PR's fault, and neither is a pass.
- docs-mcp-server#215 is `BLOCKED_NOT_OWN`: its 12 new tests load `tree-sitter-python`, which the head adds with an install script, whose rebuild failed without network or compiler; each entry's note names the package.
- **FAIL_OWN precision: 2 of 2.** ruler#37 imports `fs-extra`, which no `package.json` declares (no failed build, so it stays `NEW`). poml#17 adds two test suites that fail to load (they import `vscode`) in the PR's own versions too, under the Jest roots the base configures; its `judging::jest.config.js` (`CHANGED`) drops that directory from the roots, which is what hid them upstream. The verdict is right by the rules: the PR's own new tests fail under the project's harness. A reader who counts suites that can only run inside VS Code as environmental would score it 1 of 2.

## Round 6 status

Round 6 fixed findings 1 to 8 and 12, and partly 9 and 13 (details below). A rerun of the same harness at the round-6 build ([results.md](results.md)) has reached 32 of the 59 pull requests so far; it waits for a quiet machine before each pull request and resumes where it stopped. On those 32, **useful receipts** (the project's tests ran on both sides with per-test results) went from 0 to 27 (84%); by the first run's hand triage, 9 of the 32 were informative. Every one of the 32 produced a receipt (8 were import errors before).

| # | Finding | Round 6 |
|---:|---|---|
| 1 | Secret scanner rejects imports | Fixed. `aver ci` lists possible secrets (path, pattern, severity, never the value) in `diagnostics.json` and `summary.md` and imports anyway; `--strict-secrets` restores rejection. Binary files are not scanned, `.env.*.example` templates are not secrets, fixtures, tests, examples and Markdown are low severity. The daemon still rejects by default. 6 of the 32 runs listed findings. |
| 2 | One command-level entry | Fixed for node:test, Vitest, Jest, Mocha and tap (18+): the default check adds the runner's reporter when the test script invokes one unambiguously, and collects a JUnit file a Vitest script already writes. 27 of 32 receipts are per test. Remaining command-level runs: custom scripts (n8n-mcp-server's Jest wrapper, cuid2's riteway files run with `node`), and runners that crash before writing a report (npq's unhandled DNS rejection, sitemap.js's missing native binding). |
| 3 | Command-level red on both sides is PASS | Fixed: such an entry is inconclusive and the verdict is `BLOCKED_NOT_OWN` (npq#375). |
| 4 | Read-only `node_modules` | Fixed: a writable directory of symlinks over the read-only layer. lumen, docs-mcp-server and rekapi now run per test. |
| 5 | Install scripts skipped | Partly fixed: a network-less `npm rebuild` runs dependency install scripts, and `prepare` runs as a setup command (opensea-sdk now runs 41 tests). Scripts that download or compile native bindings still fail without network or a compiler (sitemap.js's `unrs-resolver`, docs-mcp-server#215's `tree-sitter-python`, pcui's `canvas`); the failure is shown at the top of every run's output. |
| 6 | Tests need a build | Fixed when the package's entry points are build output (`npm run build` as a setup command; mikepenz now runs 86 tests). pcui's CI builds with `build:es6`, not `build`, so its tests still miss their files (pilot run). |
| 7 | Blobs over 64 MiB | Fixed: imported empty and listed (ruler#159, ruler#268). |
| 8 | Recorded base not an ancestor | Fixed: `aver ci` compares with the merge base and signs both commits (`ci.requested_base`); the Action fetches the history. |
| 9 | CPU starvation, FLAKY base turned into PASS | Partly: a test flaky on the base that fails on every head run is inconclusive (`BLOCKED_NOT_OWN`), and the docs give CPU and timeout guidance. Jest's 5 s test timeouts still turn into FLAKY entries on a loaded machine (ruler#17, homebridge-config-ui-x). |
| 10 | The head controls the harness | Already fixed in round 4 (poml#17 now runs the base's Jest roots). |
| 11 | Base on the head's dependencies | Already fixed in round 4.1. |
| 12 | Output bound drops the error | Fixed: lines over 4 KiB are cut before the bound. |
| 13 | Diagnostics defects | The duplicated error and key wording were fixed in round 4.1. `diagnostics.json` now carries the import notes and a note on inconclusive entries; install output is still not in it. |

### FAIL_OWN precision on the rerun

13 of the 32 receipts are `FAIL_OWN`. Each was checked against the head's own version of the tests (the head-tests runs in the replay home), the pull request's diff and upstream CI where recorded:

| Pull request | NEW | What drives it | Classification |
|---|---:|---|---|
| ruler#37 | 22 | The head imports `fs-extra`, which is not a dependency; the suites fail in the head's own tests too. Closed unmerged. | True catch |
| repomix#537 | 11 | Existing tests the PR updated: the base's copies fail on the new tool API, the PR's versions pass. Merged. | Artefact: base copies of modified tests (round 4.1 rule) |
| repomix#822 | 9 | 8 modified tests as above (PR's versions pass; upstream CI green); `judging::package.json#scripts` for an added `prepare` script. | Artefact: base copies; harness edit benign |
| dotenvx#634 | 9 | The PR deletes `logRadar.js` and its test file; the base's copy of the deleted test cannot load the module. Merged. | Artefact: base copy of a deliberately deleted test |
| dotenvx#679 | 4 | Modified tests; the PR's versions pass. | Artefact: base copies of modified tests |
| ruler#159 | 4 | Modified tests (the config path moved); the PR's versions pass. Merged. | Artefact: base copies of modified tests |
| docs-mcp-server#152 | 2 | Modified tests (a new charset field); the PR's versions pass. Merged. | Artefact: base copies of modified tests |
| cuid2#104 | 1 | Command-level (riteway). The base's copy of `src/index-test.js` expects `bufToBigInt` to return a BigInt; the PR replaced BigInt with bignumber.js and changed that test. Upstream CI green. | Artefact: base copy of a modified test (the exported helper's return type did change) |
| docs-mcp-server#215 | 12 | New tests for a new native dependency, `tree-sitter-python`, whose binding has no `linux-arm64` prebuild and cannot compile without network or a compiler. | Artefact: environment (native module) |
| poml#17 | 3 | `judging::jest.config.js`: the PR drops `packages/poml-vscode/tests` from Jest's `roots`, which hides two suites already red at base; its two new suites there import `vscode` and fail to load under the base's roots, in the PR's own versions too. | Harness edit hides red suites (gaming-shaped, though those suites can only run inside VS Code); no logic regression |
| ruler#17 | 1 | Only `judging::package.json#scripts` (adds a `test:coverage` script). Under the base harness 67 tests pass; 13 FLAKY are Jest timeouts under load. | Artefact: harness rule; edit benign |
| rekapi#96 | 1 | Only `judging::vite.config.mjs` (the UMD `fileName` becomes a function, the PR's whole purpose). All 179 tests pass under the base harness. | Artefact: harness rule; edit benign |
| sitemap.js#455 | 1 | Only `judging::jest.config.cjs` (ts-jest `diagnostics.ignoreCodes: [151002]`). The tests cannot run under either harness here (native binding); upstream CI was green. | Artefact: harness rule; edit benign |

So 1 of 13 `FAIL_OWN` verdicts is a true catch. Seven come from judging existing tests with the base's copies when the pull request deliberately changed behaviour and updated (or deleted) those tests: the PR's own versions pass. Four come only from harness edits that were benign and under which the tests passed (or could not run at all); poml#17's harness edit did remove red suites. One is the environment. Neither rule was changed in round 6; this is the evidence for changing them.


These findings come from running 59 real agent pull requests through `aver ci` at commit `1a0727d`. See [results-r4.md](results-r4.md) for the setup and every verdict of that run, and [triage.json](triage.json) for the diagnosis of each pull request. Each finding names the pull requests it affected and quotes the evidence from `aver ci`'s output or from the stored output of the check runs.

In summary: of 59 pull requests, 16 got no receipt. Another 17 got a receipt whose single check was broken on both sides by Aver's sandbox or default environment, so the check was classed `PREEXISTING` and the verdict was `PASS`. Only 18 receipts (31%) came from runs where the project's tests completed on both sides. The study produced no `ENV` entries: every environment problem ended in a green verdict.

The findings are ranked by how many pull requests they blocked or left with no signal.

| # | Finding | PRs affected | Effect |
|---:|---|---:|---|
| 1 | The secret scanner rejects the whole import over test fixtures and docs | 14 | No receipt |
| 2 | The default check gives one command-level entry, so any red base hides the change | 23 (every PREEXISTING receipt) | PASS whatever the PR did |
| 3 | A sandbox failure on both sides becomes `PREEXISTING`, never `ENV` | 16 | PASS instead of BLOCKED_NOT_OWN |
| 4 | `node_modules` is read-only, which breaks vite and vitest | 7 | Startup error on both sides |
| 5 | `--ignore-scripts` skips `prepare` and native-binding postinstalls | 5 | Generated code and bindings missing |
| 6 | Only `npm test` runs, so tests that need a build fail | 3 | Missing `dist/` or `lib/` |
| 7 | Blobs over 64 MiB reject the whole import | 2 | No receipt |
| 8 | The recorded base is often not an ancestor of the head | 7 | The diff includes newer base commits, reversed |
| 9 | CPU starvation causes test timeouts, and a FLAKY base turns a head that always fails into PASS | 1 | Unearned PASS |
| 10 | The head controls the harness | 1 | Red suites removed, PASS |
| 11 | The base runs on the head's dependency layer | 1 | The base fails for reasons the head created |
| 12 | The output bound can drop the error entirely | 1 | The failure cannot be diagnosed |
| 13 | Defects in diagnostics and the summary | all | Triage needs a read of `aver.db` |

## 1. The secret scanner rejects the whole import (14 PRs, 7 repositories)

**Blocked:** dotenvx/dotenvx #634, #679, #619; yamadashy/repomix #537, #538, #590, #822; microsoft/vscode-vsce #1166; motdotla/dotenv #884; czlonkowski/n8n-mcp #244, #316, #319; microsoft/poml #69, #125.

If any file matches a secret pattern, `aver ci` refuses to import the base revision. It exits 2 and writes no receipt:

```text
aver: importing the base revision "bcb2cfb0…": import rejected, 31 violation(s):
  "src/cli/examples.js": looks like a secret (pem-private-key)
  "tests/.env": looks like a secret (dotenv-file)
  …
  "tests/monorepo/apps/unencrypted/.env": looks like a secret (dotenv-file)
```

The scanner flagged these files:

- `.env*` fixtures under test directories: dotenvx, dotenv, and vscode-vsce `src/test/fixtures/env/.env`.
- PEM private keys that are fixtures for secret scanners: repomix `tests/core/security/workers/securityCheckWorker.test.ts`, and vscode-vsce `src/test/fixtures/secrets/noSecret1.ts` and `noSecret2.ts`. The vsce fixture names say "no secret".
- PEM examples in documentation: dotenv `README.md` and `README-es.md`.
- Templates: n8n-mcp `.env.n8n.example` and `.env.test.example`. The README says that templates are not secrets, but the scanner does not exclude the `.example` suffix.
- A binary file: n8n-mcp `data/nodes.db`, a SQLite database, matched `aws-access-key-id`.
- A CI variables file: poml `.env.azure-pipelines`.
- Fake keys in unit tests: n8n-mcp telemetry tests (`aws-access-key-id`) and a disabled benchmark (`slack-token`).

In CI mode the content is already committed and public, so refusing it protects nothing. It also makes Aver unusable on any repository that tests secret handling or ships `.env` examples, and these are popular repositories (dotenv, dotenvx, repomix, vsce).

**What to change:** in `aver ci`, report findings as a warning instead of refusing the import, or scan only the paths that the change adds or modifies. Skip binary blobs and `*.example`, `*.sample` and `*.template` files. Let a repository allowlist paths, for example with an `aver.toml` key such as `[secrets] allow = ["tests/**"]`. Also fix the duplicated message (finding 13).

## 2. A command-level check hides everything behind one red test (23 PRs)

Without an `aver.toml`, the only check is `npm test` with no JUnit report, so each of the 43 receipts has exactly one entry, `test`. If the base fails for any reason, that entry is `PREEXISTING` and the verdict is `PASS`, whatever the head does. This happened in 23 of the 43 receipts (every row with PRE = 1 in results.md). These are examples where the hidden part mattered:

- lirantal/npq #375 (Copilot, merged while upstream CI was red, 331 test lines deleted): one test does a DNS lookup of `gmail.com` and crashes jest on both sides. 18 suites passed before the crash, but the receipt cannot say whether the agent's rewrite broke any of them.
- ekalinin/sitemap.js #461: 323 of 332 tests pass at base and 347 of 356 at head. The 9 failures are CLI tests that need a build (finding 6). The receipt does not show the pull request's 24 new tests.
- ProjectOpenSea/opensea-sdk #1773 adds one unit test, but mocha dies before reaching it on both sides.
- homebridge-config-ui-x #2581, #2556 and #2553: one e2e file that needs the network fails, and the pull requests' endpoint tests are hidden behind it.
- lumen-notes/lumen #493, #513 and #492: all 148 or 170 tests pass, and then vitest exits 1 (finding 4).

Per-test results were within reach. The test script of mikepenz/release-changelog-builder-action already writes JUnit (`vitest --run --reporter=default --reporter=junit --outputFile=./junit.xml`). Aver ignores the file because a report must be declared and must be under `/aver/out`. In the other direction, panphora/overtype prints `Failed: 1/10` and exits 0 on both sides, and a command-level check cannot see that failure.

**What to change:** detect the test runner from the test script and the lockfile, and turn on its built-in JUnit reporter by default. vitest (`--reporter=junit --outputFile=…`), `node --test` (`--test-reporter=junit --test-reporter-destination=…`) and mocha (`--reporter xunit --reporter-options output=…`) include one. jest needs the `jest-junit` package, which a project may not have. Also collect a JUnit file that the test script already writes. Until then, state in the summary that a command-level `PREEXISTING` entry means the run gives no regression signal.

## 3. A sandbox failure on both sides becomes `PREEXISTING`, never `ENV` (16 PRs)

In the 16 receipts triaged as Aver's fault (findings 4, 5, 6 and 9), the sandbox broke the check in the same way on both sides. Cause rule 4 calls that `PREEXISTING`, and the regression-only verdict is `PASS`. `ENV` comes only from `infra` outcomes (OOM, timeout or sandbox error), so a missing module, a read-only directory or a missing generated file never counts. The 43 receipts carry no `ENV` entries.

When the only check failed on both sides for environmental reasons, a `PASS` tells the reviewer that the change is safe although nothing was tested.

**What to change:** in regression-only mode, do not give a plain `PASS` to a receipt whose every entry is a command-level `PREEXISTING`. Use `BLOCKED_NOT_OWN` or a separate "no signal" outcome. It would also help to class common environment signatures as `ENV`: `Cannot find module` under `/work/node_modules` or `dist/`, `EROFS` or `ENOENT` from `mkdir` under `node_modules`, and `EAI_AGAIN`.

## 4. Read-only `node_modules` breaks vite and vitest (7 PRs, 3 repositories)

**Affected:** jeremyckahn/rekapi #96; arabold/docs-mcp-server #181, #152, #215; lumen-notes/lumen #493, #513, #492.

Check sandboxes mount the dependency layer read-only at `/work/node_modules` (ARCHITECTURE.md §2.4.2). Two vite behaviours need to write there:

- Vite bundles an ESM config (`vite.config.mjs`, or `vite.config.ts` in a `"type": "module"` package) into `node_modules/.vite-temp`, so vitest dies at startup on both sides:
  ```text
  failed to load config from /work/vite.config.mjs
  Error: ENOENT: no such file or directory, mkdir '/work/node_modules/.vite-temp'
  ```
  Vite bundles CommonJS configs in memory, so softprops/action-gh-release (`vitest.config.ts` without `"type": "module"`) and homebridge-config-ui-x ran.
- vitest writes its results cache to `node_modules/.vite`. In lumen, all 148 tests pass, and then the run exits 1 on `mkdir '/work/node_modules/.vite'`.

The error says `ENOENT`, not `EROFS`, which hides the cause. A probe in the same image with the same read-only mount showed that `fs.mkdirSync(…, {recursive: true})` reports `ENOENT`, and only the non-recursive call reports `EROFS`.

**What to change:** give each check a writable view of `node_modules`. Either use an overlay with a tmpfs upper layer over the shared read-only layer, or mount tmpfs at the usual cache paths (`node_modules/.vite`, `node_modules/.vite-temp` and `node_modules/.cache`). Both options keep the layer shared and immutable.

## 5. `--ignore-scripts` skips `prepare` and native-binding postinstalls (5 PRs)

**Affected:** ekalinin/sitemap.js #455, #456; ProjectOpenSea/opensea-sdk #1773, #1774; kulshekhar/ts-jest #5036.

- opensea-sdk generates `src/typechain/contracts` through `prepare`, then `build`, then `abi-type-gen`. With scripts off, the directory never exists, and mocha fails on both sides with `Error: Cannot find module './typechain/contracts'`.
- ts-jest builds itself in `prepare`, and `postbuild` writes `.ts-jest-digest`. Without that file, jest cannot load the project's config: `Error: ENOENT: no such file or directory, open '/work/.ts-jest-digest'`.
- sitemap.js: jest's resolver `unrs-resolver` needs a native binding. The lockfile was generated on a Mac and lists only `@unrs/resolver-binding-darwin-x64`. `napi-postinstall` would download the correct binding at install time, but scripts are off. It tries again at test time, with no network: `Failed to install package "@unrs/resolver-binding-linux-arm64-gnu": getaddrinfo EAI_AGAIN registry.npmjs.org`. jest then reports `Preset ts-jest not found`. The sandbox platform is always the host's (`linux/arm64` here), so lockfiles made on x64 machines fail more often on Apple Silicon.

Setting `allow_scripts = true` in `aver.toml` would fix these cases. No real repository has an `aver.toml`, however, so the default decides.

**What to change:** by default, let the install sandbox run the root package's `prepare` script and the dependencies' install scripts, which is where native bindings come from. That sandbox already has network access and no secrets. Alternatively, add a build phase between the install and the checks. Keep `--ignore-scripts` as the strict option.

## 6. Only `npm test` runs, so tests that need a build fail (3 PRs)

**Affected:** mikepenz/release-changelog-builder-action #1458, playcanvas/pcui #459, ekalinin/sitemap.js #461.

The CI of these projects runs `npm run build` before `npm test`, and their tests import the build output:

- mikepenz: `Error: Cannot find module '/work/lib/main.js'`, 10 times. The `build` script is `tsc`.
- pcui: every `node --test` file fails with `ERR_MODULE_NOT_FOUND` for `/work/dist/module/src/components/…/index.mjs`, which rollup produces.
- sitemap.js #461: 9 CLI tests fail with `Cannot find module '/work/dist/esm/cli.js'`. The project's `test:full` script runs `npm run build && jest`.

**What to change:** in the default environment, run `build` before `test` when `package.json` defines it, or read the steps from `.github/workflows/*.yml`. At a minimum, name the missing build in the summary when the failure is `Cannot find module` under `dist/`, `lib/` or `build/`.

## 7. Blobs over 64 MiB reject the whole import (2 PRs)

**Blocked:** intellectronica/ruler #159 and #268. The file `img/ruler-short.gif` is 72,299,857 bytes:

```text
"img/ruler-short.gif": 72299857 bytes exceeds the 67108864-byte blob limit
```

One demo GIF stops the run.

**What to change:** import an oversize blob as a placeholder, or by reference, leave it out of the sandbox tree and record that in the receipt. A test that reads such a file then fails visibly, which is better than no receipt.

## 8. The recorded base is often not an ancestor of the head (7 of 59 PRs)

`aver ci` diffs the two trees it receives. GitHub's `baseRefOid`, which is also the `pull_request.base.sha` that the GitHub Action passes, is the tip of the base branch when the pull request was last updated, not the commit where the head forked. For 7 of the 59 pull requests, that commit is not an ancestor of the head: homebridge-config-ui-x #2556, TechStark/opencv-js #88, motdotla/dotenv #884, kulshekhar/ts-jest #5036, leonardsellem/n8n-mcp-server #32, czlonkowski/n8n-mcp #319 and ProjectOpenSea/opensea-sdk #1774. The judged change then includes the base branch's newer commits, reversed. For homebridge-config-ui-x #2556, the pull request touches 7 files, but the diff that Aver judged has 210, and the head ran 19 test files against the base's 24. Once per-test results exist, each test that those newer base commits added will show as "missing from head report" (`NEW`).

**What to change:** in `aver ci` and the Action, verify against `git merge-base <base> <head>` or against GitHub's test merge commit, and record which one was used. The harness records the recorded base, the merge base and, for merged pull requests, the first parent of the commit that landed the change. A rerun with `--base merge-parent` is therefore possible.

## 9. CPU starvation, and a FLAKY base that turns a head that always fails into PASS (1 PR)

**Affected:** intellectronica/ruler #17.

Every failure on either side was a jest per-test timeout (`thrown: "Exceeded timeout of 5000 ms for a test."`) in e2e tests that run `npm run build`. The base failed twice and then passed. The head failed all three runs, each time on a different set of tests (5, 3 and 1 failures). There are two problems:

- CPU starvation. Each sandbox gets a 2-CPU quota (`--cpus=2`). The base and head runs, and their reruns, run at the same time, and other Docker workloads shared the VM during this run. The failing tests each spawn a `tsc` build. Inside the sandbox, `os.availableParallelism()` reports 2 but `os.cpus().length` reports 8, so any tool that sizes a pool from `os.cpus()` oversubscribes the quota. Aver records no CPU throttling, so the receipt cannot show that the timeouts came from the sandbox, not the code. Options: run the reruns of a side one after another instead of in parallel with the other side, report the cgroup's throttling counters with each run, and treat a timeout-only failure pattern as `ENV`.
- The verdict rule. The entry is `FLAKY` because the base runs disagreed with each other. In regression-only mode `FLAKY` does not block, so the verdict was `PASS`, although no head run ever passed. A head with zero passing runs should not get `PASS`.

## 10. The head controls the harness (1 PR)

**Affected:** microsoft/poml #17, "Fix preview test and extend mocha timeout" (Codex, merged). At base, 2 suites under `packages/poml-vscode/tests` fail. One imports `vscode`, which exists only inside VS Code, and the other contains no tests. The pull request removes that directory from jest's `roots`:

```diff
-  roots: ['<rootDir>/packages/poml/tests', '<rootDir>/packages/poml-vscode/tests'],
+  roots: ['<rootDir>/packages/poml/tests'],
```

The head runs 10 suites instead of 12, `npm test` exits 0, and Aver reports `PASS` (base `fail`, head `pass`, cause `null`), as if the change had fixed something. Round 4 plans to take the harness from the base, and this is a real example of why. Until then, the summary should at least point out a base-fail to head-pass change when the pull request also changes test configuration.

## 11. The base runs on the head's dependency layer (1 PR, masked)

**Affected:** lirantal/npq #375, which removes the `pacote` dependency. As §2.5.1 specifies, the base code ran with the head's `node_modules`, so two base suites failed with `Cannot find module 'pacote'`. The DNS crash (finding 2) masked this here. With per-test results, though, these would be base failures that the head created: real fixes in such a pull request would look like no-ops, and any test that uses the removed dependency would look `PREEXISTING`.

**What to change:** when the lockfile changes, run the base side on the layer built from its own lockfile. Otherwise, flag each base failure that does not occur when the base runs on its own layer.

## 12. The output bound can drop the error entirely (1 PR)

**Affected:** paralleldrive/cuid2 #98. `npm test` loads the `esm` package, which throws before any test runs. Node prints the source line that failed before the error message, and that line in `node_modules/esm/esm.js` is longer than the whole 64 KiB output budget. The stored output (65,600 bytes) is the npm banner followed by minified code, and the error message is lost. Neither the receipt nor the run output shows the cause.

**What to change:** cap the length of each line before applying the head and tail byte bounds, so that the last lines, where the error usually is, survive.

## 13. Defects in diagnostics and the summary (every run)

- **Duplicated error.** For an import rejection, stderr and `summary.md` print the violation list twice: after `…unencrypted/.env": looks like a secret (dotenv-file)` the text `import rejected, 31 violation(s):` and the whole list repeat. Both the error context and its source render the list.
- **Wrong key statement with `--home`.** With a persistent `--home`, `summary.md` still says "The key was generated for this run". Every receipt in this study was signed by the same persistent key.
- **Incomplete reproduce line.** The reproduce snippet omits `--home`. It also prints the absolute local path of `--repo`, which means nothing to someone reading a pull request comment.
- **Run output is not available from `aver ci`.** The `detail` field in the receipt holds the last 2 KiB of one run. To triage this study, the harness had to read run objects and output blobs out of `aver.db` (`dumpRuns` in `replay.ts`). The planned `diagnostics.json` should carry, for each run, the outcome, exit code, duration and a pointer to the full bounded output. It should also carry the duration and output of the dependency install, which are not visible when the install succeeds: paralleldrive/cuid2 #104 took 340 s, and its two check runs took 9 s and 10 s.
- **No choice of Node version.** The image is always `node:22`. The base of nemtsov/json-mask fails one CLI test that matches the JSON error text of an older Node version, and the pull request loosens that assertion. That failure is the project's, but Aver ignores `engines`, `.nvmrc` and the CI matrix, so more projects will hit it.

## Not Aver's fault, for contrast

- **Tests need the network** (npq #375; homebridge-config-ui-x #2581, #2556, #2553; part of mikepenz #1458): DNS lookups and registry or API calls fail with `EAI_AGAIN` in the sandbox, which has no network by design. These failures cost signal only because of finding 2.
- **A real regression caught** (intellectronica/ruler #37, Jules, closed without merging): the head imports `fs-extra`, which is not a dependency, and a module that does not exist. `tsc` fails, and 28 of 109 tests fail on all three head runs while the base passes. This is a correct `FAIL_OWN` and the only `NEW` entry in the study.
- **The project's own failures on the base**: leonardsellem/n8n-mcp-server #23 (a TS7006 error in a test file at base) and nemtsov/json-mask #174 (the Node error-text test).
