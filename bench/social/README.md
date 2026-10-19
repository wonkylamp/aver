# Social clips

Five split-screen comparison clips for X and LinkedIn, recorded from real terminal runs. The left pane is the standard tool and the right pane is Aver. Every line on screen is the real output of the command shown above it.

Each clip is 1200x675 at 15 fps. It is written twice to `assets/social/`, as `<clip>.gif` and as `<clip>.mp4` (H.264, yuv420p). The MP4 is 1200x674 because yuv420p needs an even height, so it drops the bottom row, which is terminal padding.

## Rebuild

```sh
bench/social/build.sh                 # all five
bench/social/build.sh merge-day scale # some
```

The build needs `vhs` and `ffmpeg` in `/opt/homebrew/bin`, Pillow for `/opt/homebrew/bin/python3`, and the installed `~/.local/bin/aver`. It does not build Aver. To record with another binary, such as a fresh `cargo build --release -p aver`, set `AVER_BIN_DIR` to its directory; it goes first on every pane's `PATH`. Every clip works under `/tmp/aver-social/<clip>/` with its own `AVER_HOME`, so it never touches `~/.aver`. The agent markers (`CLAUDECODE`, `AVER_*`) are unset everywhere except where a pane sets one on screen. The build stops when the disk has less than 15 GB free.

For each clip, `build.sh` runs these steps:

1. `setup.sh <clip>` builds the starting state. The `calc` clips use `bench/hero/make-repo.sh`, which needs the npm cache that the hero GIF used, because it installs offline. `merge-day` uses `fixtures/sh-calc`.
2. `vhs` records `tapes/<clip>.left.tape` and `tapes/<clip>.right.tape`. Each tape is prefixed with `tapes/_settings.tape`. Each tape's hidden first line sources `prompt.sh`, which sets the prompt. The prompt shows a green ` ok ` or a red ` exit N ` for the last command.
3. `compose.sh` stacks the two panes. The shorter pane holds its last frame. The script adds the caption bar from `header.py` and writes the GIF (palettegen and paletteuse from a lossless intermediate) and the MP4.

This ffmpeg build has no `drawtext` filter, because it was built without freetype. `header.py` therefore draws the caption bar and the pane titles in Helvetica with Pillow, and ffmpeg overlays the result.

The agents' edits are scripts that run on screen under a descriptive function name. `agent-breaks-sub-and-skips-its-test` runs `bench/hero/sabotage.sh`. `agent-breaks-sub-and-edits-its-assertion` runs `agents/rewrite.sh`. `agent-adds-a-test-that-cannot-fail` runs `agents/empty-test.sh`.

## Clips

| Clip | Caption | Left | Right |
|---|---|---|---|
| `lie-detector` | Your CI says green. Your agent cheated. | The agent breaks `sub` and `.skip`s its test. `npm test` reports pass 5, fail 0, skipped 1 and exits 0. | The same edit in a workspace. `aver verify` gives `NOT DONE: FAIL_OWN` and exits 1. |
| `quiet-rewrite` | Agents can't approve their own homework. | The agent breaks `sub` and changes its assertion from 2 to 8. `npm test` exits 0. | `aver verify` gives `NEEDS HUMAN REVIEW: REVIEW` and exits 0, as a workspace verify does for REVIEW. `CLAUDECODE=1 aver accept --approve-review` is refused and exits 2. |
| `empty-test` | A test that can't fail proves nothing. | The agent adds a `[crit:div]` test that asserts `true`. `npm test` exits 0. | `aver workspace new -c 'div: …'`, the same test, then `aver verify`: `NOT DONE: UNPROVEN`, `div NOT PROVEN (passes_without_change)`, exit 1. |
| `merge-day` | Three agents. One file. No conflicts. | Branches `sub`, `mul` and `neg` each fix one function in `calc.sh` and append a test to `test/add.tests`. `git merge sub && git merge mul && git merge neg` stops at `CONFLICT (content): Merge conflict in test/add.tests` and exits 1. | The same three edits in three workspaces, each verified with PASS during setup. `aver combine sub mul neg` from the repository root merges them line by line, shows PASS for each workspace, and exports `aver/combine-…`. It exits 0. |
| `scale` | 20 agent workspaces before your coffee. | `time for i in $(seq 20); do git worktree add … && (cd … && npm ci --offline …); done` | `time aver workspace new --count 20`, cold, with the commit import included. |

`scale` runs in real time with no speed-up. Each pane's `total` line is bash's `time` for that run. The two panes are recorded one after the other, not at the same moment, so machine load can differ between them. On a loaded machine, `npm ci` can take seconds per run, and then the left pane may not finish inside the tape's 14 s window. Rebuild `scale` when the machine is quiet, and check the last frame for both `total` lines.

## Checking a clip

```sh
ffmpeg -v error -sseof -0.2 -i assets/social/<clip>.gif -frames:v 1 /tmp/last.png
```
