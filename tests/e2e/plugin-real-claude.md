# Plugin onboarding with real Claude Code (5 minutes, by hand)

`tests/e2e/plugin-onboarding.sh` (`make test-onboarding`) runs this journey with no model: it feeds the hooks the JSON Claude Code would send. This script confirms the same journey in Claude Code itself. It spends a few model turns, so run it in your own terminal.

Everything runs in a throwaway home, so your `~/.claude` and `~/.aver` are not touched. Claude Code needs to sign in again there.

## 1. A clean machine (1 minute)

```bash
export T=$(mktemp -d) && export HOME=$T/home CLAUDE_CONFIG_DIR=$T/claude && mkdir -p $HOME && export PATH=/usr/bin:/bin:/usr/sbin:/sbin:$(dirname "$(command -v claude)")
```

Check that `command -v aver` prints nothing. Then make a project from the fixture (run this from the root of your Aver checkout):

```bash
cp -R fixtures/sh-calc $T/calc && cd $T/calc && git init -q && git add -A && git -c user.name=t -c user.email=t@x commit -qm calc
```

## 2. Install the plugin, then /aver (2 minutes)

Start `claude` in `$T/calc` and sign in. Then:

1. `/plugin marketplace add <path to your Aver checkout>`, then `/plugin install aver@aver`, then restart `claude`.
   - Expect: the session starts with no hook error.
   - Expect: Claude knows that "Aver plugin is installed, but the `aver` CLI is missing". Ask "what did the session start hook say?".
2. Type `/aver` and look at the menu before you press Enter.
   - Note which entries appear (`/aver`, `/aver:aver`) and which description each shows. The plugin has a command and a skill that are both named `aver`. Choose the one whose description starts with "Set up Aver here".
   - Expect: Claude runs `command -v aver` and offers the two install methods. Choose the install script. To stay offline, tell Claude to run this command instead (`dist/` comes from `packaging/release-local.sh`, or use the release directory that the onboarding script builds):
     `AVER_RELEASE_BASE_URL=file://<dir> AVER_VERSION=0.4.1 sh <checkout>/site/install.sh`
   - Expect: the script says that `~/.local/bin` is not on `PATH`. Claude says how to add it.
   - Expect: Claude runs `aver doctor`, then asks before it runs `aver init --yes`. Afterwards it lists `.claude/settings.json`, the skill, and the commands.
3. Quit `claude`. Run `git add -A && git commit -qm "Set up Aver"`.

## 3. A worktree session (2 minutes)

Run `export PATH=$HOME/.local/bin:$PATH`, then start `claude --worktree add-sub` in `$T/calc`.

- Expect: the start takes a second or two, with no error. `pwd` (ask Claude) is under `$HOME/.aver/work/calc/wt-add-sub`, not `.claude/worktrees/`.
- Ask: "Add a sub function to calc.sh and a test line for it in test/add.tests. Do not run aver yourself."
- Expect: when Claude tries to stop, the Stop hook blocks it once with "NOT DONE: you changed 2 files…". Claude then runs `aver check` and `aver verify`.
- Expect: `aver verify` prints `DONE: PASS`, and Claude stops with no second block.
- Quit and choose to remove the worktree.
  - Expect: no error, and `ls $HOME/.aver/work/calc` no longer lists `wt-add-sub`.

Each event acts once: open `/hooks` to see both the plugin's hooks and the project's hooks. Expect only one brief at session start and only one Stop block, not two.

## 4. A teammate without aver (30 seconds)

```bash
export PATH=/usr/bin:/bin:/usr/sbin:/sbin:$(dirname "$(command -v claude)") && git clone -q $T/calc $T/mate && cd $T/mate
```

Run `/plugin uninstall aver@aver` in `claude` first, or run this step with a second throwaway `CLAUDE_CONFIG_DIR`. Then start `claude --worktree mate`.

- Expect: no hook error. The worktree is `.claude/worktrees/mate` on the branch `worktree-mate`. Stopping is never blocked.

## What to report

Report each hook error, a doubled brief or block, a confusing message (copy it exactly), and which `/aver` entry ran in step 2. Then clean up with `rm -rf "$T"`.
