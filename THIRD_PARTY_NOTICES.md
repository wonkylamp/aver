# Third-party notices

Aver is licensed under the MIT License ([LICENSE](LICENSE)). This file lists the third-party material that this repository contains or ships, and its license.

## Bundled in this repository

| Material | Where | License | Source |
|---|---|---|---|
| Atkinson Hyperlegible Next (latin subset, variable weight) | `site/fonts/atkinson-hyperlegible-next-latin-wght-normal.woff2` | SIL Open Font License 1.1, text in `site/fonts/OFL-atkinson-hyperlegible-next.txt` | Copyright 2020-2024 The Atkinson Hyperlegible Next Project Authors, https://github.com/googlefonts/atkinson-hyperlegible-next |
| JetBrains Mono (latin subset, variable weight) | `site/fonts/jetbrains-mono-latin-wght-normal.woff2` | SIL Open Font License 1.1, text in `site/fonts/OFL-jetbrains-mono.txt` | Copyright 2020 The JetBrains Mono Project Authors, https://github.com/JetBrains/JetBrainsMono |
| `@noble/ed25519` 3.2.0 | bundled into `site/viewer.js` | MIT, below | Copyright (c) 2019 Paul Miller, https://github.com/paulmillr/noble-ed25519 |
| `@noble/hashes` 2.4.0 (BLAKE3) | bundled into `site/viewer.js` | MIT, below | Copyright (c) 2022 Paul Miller, https://github.com/paulmillr/noble-hashes |
| Contributor Covenant 2.1 | `CODE_OF_CONDUCT.md` | CC-BY-4.0, attribution in the file | https://www.contributor-covenant.org/version/2/1/code_of_conduct/ |

The rest of `site/viewer.js` and all of `site/landing.js` are built from this repository's own code (`site/src/`). The Node projects in `fixtures/` and `bench/eval/tasks/` were written for Aver and have no npm dependencies. `crates/aver/tests/reports/` holds sample test-runner reports (Jest JSON, Mocha xunit) used as a parser test input.

## Data

| Data | Where | License | Source |
|---|---|---|---|
| AIDev dataset: the list of agent-authored pull requests replayed by the replay study | `bench/replay/manifest.json` | CC-BY-4.0 | Hao Li, Haoxiang Zhang, Ahmed E. Hassan, AIDev, https://huggingface.co/datasets/hao-li/AIDev |

The replay study lists repository names, pull request numbers and commit ids of public GitHub repositories. It does not copy their code: `bench/replay/replay.ts` fetches each repository read-only when it runs.

## Fetched at run time, not shipped

These are downloaded by scripts or by Aver itself and are not part of this repository or its release binaries: the packages Aver installs for a project's checks from its lockfile, and date-fns (`bench/import.sh`, MIT) for the import benchmark.

## Rust and npm dependencies

The Rust crates in `Cargo.lock` and the npm packages in `site/package-lock.json` are not vendored, except the `@noble` packages bundled as listed above. Their licenses are in their packages. The release binaries link the Rust crates statically, so a release should ship their license texts; `cargo about generate` or `cargo deny check licenses` produces and checks that list from `Cargo.lock`.

## MIT License (`@noble/ed25519`, `@noble/hashes`)

```
The MIT License (MIT)

Copyright (c) 2019 Paul Miller (https://paulmillr.com)   [@noble/ed25519]
Copyright (c) 2022 Paul Miller (https://paulmillr.com)   [@noble/hashes]

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```
