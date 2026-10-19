#!/usr/bin/env bash
# Other agent tools and uninstalling, end to end, with no model call and no
# network:
#
#   1. Claude Code on a fresh repository: `aver integrate claude --yes`
#      keeps the team's settings, writes the plugin's files, and a second
#      run changes nothing;
#   2. Codex (AGENTS.md), Cursor, Gemini and VS Code Copilot: each `aver
#      integrate <tool> --yes` writes its file or marked block once and
#      keeps the user's text around it; `aver init` sets up only the tools
#      the machine or the repository shows;
#   3. `aver setup --remove` puts every file back byte for byte, also from a
#      torn record and over a torn file;
#   4. `aver doctor` on a machine without node, go or cargo names each
#      missing tool with its fix.
#
# Everything lives in a throwaway HOME and AVER_HOME.
#
#   tests/e2e/upgrade-tools.sh                     # aver from PATH
#   AVER=<binary> tests/e2e/upgrade-tools.sh
#   E2E_KEEP=1 ...                                 # keep the work directory
#
# Needs git and python3.
set -uo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
abs() { (cd "$(dirname "$1")" && printf '%s/%s\n' "$(pwd)" "$(basename "$1")"); }
AVER=${AVER:-$(command -v aver || true)}
[ -x "$AVER" ] || { echo "upgrade-tools: no aver binary (set AVER)" >&2; exit 2; }
AVER=$(abs "$AVER")

T=$(mktemp -d "${TMPDIR:-/tmp}/aver-upgrade.XXXXXX")
T=$(cd "$T" && pwd -P)
cleanup() { [ -n "${E2E_KEEP:-}" ] && echo "kept $T" || rm -rf "$T"; }
trap cleanup EXIT

export HOME="$T/home" AVER_HOME="$T/home/.aver" AVER_APPS_DIR="$T/apps"
mkdir -p "$HOME" "$AVER_APPS_DIR" "$T/bin" "$T/tools"
unset AVER_BIN AVER_URL AVER_RUN CLAUDE_PLUGIN_ROOT GIT_DIR GIT_WORK_TREE CLAUDE_PROJECT_DIR
export GIT_CONFIG_GLOBAL="$HOME/.gitconfig" GIT_CONFIG_NOSYSTEM=1
git config --global user.name "Upgrade User"
git config --global user.email "upgrade@example.com"
git config --global init.defaultBranch main
# Aver recognises its hook commands by the binary's name, `aver`.
ln -s "$AVER" "$T/bin/aver"
BASE_PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH="$T/bin:$BASE_PATH"
for t in claude codex cursor gemini code; do
  for d in /opt/homebrew/bin /usr/local/bin; do
    if [ -e "$d/$t" ]; then
      echo "upgrade-tools: $d/$t exists; aver finds agent tools there, so a machine without $t cannot be simulated" >&2
      exit 2
    fi
  done
done

pass=0
failures=()
ok() { pass=$((pass + 1)); printf '  ok   %s\n' "$1"; }
bad() { failures+=("$1"); printf '  FAIL %s\n' "$1"; [ -n "${2:-}" ] && printf '%s\n' "$2" | sed 's/^/       | /'; }
skip() { printf '  skip %s\n' "$1"; }
section() { printf '\n== %s\n' "$1"; }
contains() { case $1 in *"$2"*) return 0 ;; *) return 1 ;; esac; }
# expect <name> <text> <needle>: ok when text holds needle.
expect() { if contains "$2" "$3"; then ok "$1"; else bad "$1" "$2"; fi; }
refuse() { if contains "$2" "$3"; then bad "$1" "$2"; else ok "$1"; fi; }
# same <name> <file> <file>: byte for byte.
same() {
  cat "$3" > "$T/.want"
  if cmp -s "$T/.want" "$2"; then ok "$1"; else bad "$1" "$(diff -u "$T/.want" "$2" | head -40)"; fi
}
# jsame <name> <file> <json>: the same JSON value, whatever its layout.
jsame() {
  local d
  if d=$(python3 - "$2" "$3" <<'PY'
import json, sys
got = json.load(open(sys.argv[1]))
want = json.loads(sys.argv[2])
if got != want:
    print("got:  " + json.dumps(got, sort_keys=True))
    print("want: " + json.dumps(want, sort_keys=True))
    sys.exit(1)
PY
  ); then ok "$1"; else bad "$1" "$d"; fi
}
# A listing of every file under a repository with its hash, .git's
# exclude file and Aver's record included, objects and logs left out.
snapshot() {
  (cd "$1" && find . -type f ! -path './.git/objects/*' ! -path './.git/logs/*' \
      ! -path './.git/index' ! -path './.git/ORIG_HEAD' ! -path './.git/COMMIT_EDITMSG' \
      ! -path './.git/hooks/*' -print0 | sort -z | xargs -0 shasum -a 256)
}
snapdirs() { (cd "$1" && find . -type d ! -path './.git*' | sort); }
# unchanged <name> <repo> <snapshot file>
unchanged() {
  local now; now=$(snapshot "$2")
  if [ "$now" = "$(cat "$3")" ]; then ok "$1"; else bad "$1" "$(diff <(cat "$3") <(printf '%s\n' "$now") | head -30)"; fi
}
new_repo() {
  local r="$T/$1"
  mkdir -p "$r" && git -C "$r" init -q && printf '# %s\n' "$1" > "$r/README.md"
  git -C "$r" add README.md && git -C "$r" commit -qm init
  printf '%s\n' "$r"
}
count() { grep -c -- "$2" "$1" 2>/dev/null || true; }
# A fake agent CLI on a PATH of its own.
fake_tool() { mkdir -p "$T/tools/$1"; printf '#!/bin/sh\nexit 0\n' > "$T/tools/$1/$1"; chmod +x "$T/tools/$1/$1"; printf '%s\n' "$T/tools/$1"; }

BEGIN='<!-- BEGIN aver'
END='<!-- END aver -->'

# The user's own Claude Code settings, in their layout.
user_settings='{
  "permissions": {
    "allow": ["Bash(make:*)"]
  },
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [ { "type": "command", "command": "./scripts/lint.sh" } ] }
    ]
  },
  "enabledMcpjsonServers": ["db"]
}
'
user_local='{
  "permissions": { "allow": ["Bash(ls:*)"] },
  "hooks": {
    "Stop": [ { "hooks": [ { "type": "command", "command": "say done" } ] } ]
  }
}
'

################################################################################
section "1. Claude Code on a fresh repository"

r1=$(new_repo fresh)
mkdir -p "$r1/.claude"
printf '%s' "$user_settings" > "$r1/.claude/settings.json"
printf '%s' "$user_local" > "$r1/.claude/settings.local.json"
git -C "$r1" add .claude/settings.json && git -C "$r1" commit -qm "team settings"
snapshot "$r1" > "$T/r1.s0"
snapdirs "$r1" > "$T/r1.d0"

up=$(cd "$r1" && aver integrate claude --yes 2>&1)
expect "integrate claude writes" "$up" "Done."
python3 - "$r1/.claude/settings.json" <<'PY2' && ok "settings.json: rev 4 hooks, the team's hooks and settings kept" || bad "settings.json after integrate" "$(cat "$r1/.claude/settings.json")"
import json, sys
d = json.load(open(sys.argv[1]))
assert d["permissions"] == {"allow": ["Bash(make:*)"]}, d
assert d["enabledMcpjsonServers"] == ["db"], d
assert d["hooks"]["PreToolUse"] == [{"matcher": "Bash", "hooks": [{"type": "command", "command": "./scripts/lint.sh"}]}], d
assert sorted(d["hooks"]) == ["PreToolUse", "SessionStart", "Stop", "SubagentStop", "WorktreeCreate", "WorktreeRemove"], d
cmds = [h["command"] for ev in d["hooks"].values() for g in ev for h in g["hooks"]]
aver = [c for c in cmds if "aver hook" in c]
assert len(aver) == 5 and all("--rev 4" in c for c in aver), aver
assert "AVER_HOME" not in json.dumps(d), "a machine path"
PY2
jsame "settings.local.json: the user's own, untouched" "$r1/.claude/settings.local.json" "$user_local"
for f in .claude/skills/aver/SKILL.md .claude/commands/aver.md; do
  same "$f is the plugin's file" "$r1/$f" "$root/integrations/claude-code/${f#.claude/}"
done
snapshot "$r1" > "$T/r1.s1"
again=$(cd "$r1" && aver integrate claude --yes 2>&1)
expect "a second integrate says it is set up" "$again" "already set up for Aver"
unchanged "a second integrate changes no file" "$r1" "$T/r1.s1"
d2=$(cd "$r1" && aver doctor --verbose 2>&1)
expect "doctor: hooks current" "$d2" "Claude Code hooks are current (rev 4)"
h4=$(cd "$r1" && printf '{"session_id":"s9","hook_event_name":"SessionStart","cwd":"%s","source":"startup"}' "$r1" | sh -c "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["hooks"]["SessionStart"][0]["hooks"][0]["command"])' "$r1/.claude/settings.json")" 2>&1)
expect "the SessionStart hook gives the checkout note" "$h4" "aver workspace new"

################################################################################
section "2. other agent tools"

r2=$(new_repo tools)
printf '# Project\n\nRun `make test` before pushing.\n' > "$r2/AGENTS.md"
printf '# Gemini notes\n\nBe brief.\n' > "$r2/GEMINI.md"
mkdir -p "$r2/.github" "$r2/.cursor/rules"
printf 'Prefer small pull requests.\n' > "$r2/.github/copilot-instructions.md"
printf -- '---\ndescription: house style\nalwaysApply: true\n---\n\nTabs, not spaces.\n' > "$r2/.cursor/rules/style.mdc"
git -C "$r2" add -A && git -C "$r2" commit -qm "agent instructions"
snapshot "$r2" > "$T/r2.s0"
snapdirs "$r2" > "$T/r2.d0"
for f in AGENTS.md GEMINI.md .github/copilot-instructions.md .cursor/rules/style.mdc; do cp "$r2/$f" "$T/r2.$(echo "$f" | tr / _)"; done

# Each tool's file, the user's text before the block untouched, one block.
check_block() { # <name> <file> <user text file>
  local f=$2 n
  n=$(count "$f" "$BEGIN")
  if [ "$n" = 1 ] && [ "$(count "$f" "$END")" = 1 ]; then ok "$1: one Aver block"; else bad "$1: one Aver block" "begin markers: $n"; fi
  if head -c "$(wc -c < "$3")" "$f" | cmp -s - "$3"; then ok "$1: the user's text before it is unchanged"; else bad "$1: the user's text before it is unchanged" "$(head -5 "$f")"; fi
}
for tool in codex gemini vscode cursor; do
  out=$(cd "$r2" && aver integrate "$tool" --yes 2>&1)
  expect "integrate $tool writes" "$out" "Done."
done
out=$(cd "$r2" && aver integrate agents-md --yes 2>&1)
expect "agents-md after codex: the same AGENTS.md block" "$out" "already set up for Aver"
check_block "AGENTS.md" "$r2/AGENTS.md" "$T/r2.AGENTS.md"
check_block "GEMINI.md" "$r2/GEMINI.md" "$T/r2.GEMINI.md"
check_block "copilot-instructions.md" "$r2/.github/copilot-instructions.md" "$T/r2..github_copilot-instructions.md"
same "the user's Cursor rule is untouched" "$r2/.cursor/rules/style.mdc" "$T/r2..cursor_rules_style.mdc"
mdc=$(cat "$r2/.cursor/rules/aver.mdc" 2>/dev/null)
expect "Cursor gets its own rule file" "$mdc" "alwaysApply: false"
snapshot "$r2" > "$T/r2.s1"
for tool in codex gemini vscode agents-md cursor; do
  out=$(cd "$r2" && aver integrate "$tool" --yes 2>&1)
  expect "integrate $tool again: already set up" "$out" "already set up for Aver"
done
unchanged "running every tool again changes no file" "$r2" "$T/r2.s1"

# The user writes below the block and an older Aver's block is replaced in
# place: only the block changes.
printf '\n## Release\n\nTag from main.\n' >> "$r2/AGENTS.md"
cp "$r2/AGENTS.md" "$T/r2.agents.edited"
(cd "$r2" && aver integrate codex --yes >/dev/null 2>&1)
same "text after the block survives a rerun" "$r2/AGENTS.md" "$T/r2.agents.edited"
python3 - "$r2/AGENTS.md" <<'PY'
import sys
p = sys.argv[1]
t = open(p).read()
b = t.index("<!-- BEGIN aver")
e = t.index("<!-- END aver -->")
open(p, "w").write(t[:b] + "<!-- BEGIN aver: written by `aver integrate`; running it again replaces this block -->\n\nAn older Aver's instructions: call the aver MCP tools.\n\n" + t[e:])
PY
out=$(cd "$r2" && aver integrate codex --yes 2>&1)
expect "an older block is replaced" "$out" "Done."
same "an older block is replaced in place" "$r2/AGENTS.md" "$T/r2.agents.edited"

# What `aver init` sets up: Claude Code where it is installed or .claude/
# exists, AGENTS.md where Codex is installed, .codex/ exists or AGENTS.md
# does, nothing else.
init_agents() { # <repo> [extra PATH]
  (cd "$1" && PATH="${2:+$2:}$T/bin:$BASE_PATH" aver init --yes --json 2>/dev/null) |
    python3 -c 'import json,sys; a=json.load(sys.stdin)["agents"]; print(" ".join(sorted(a["tools"])) or "-")'
}
ri=$(new_repo init-none)
expect "init with no agent tool sets up none" "$(init_agents "$ri")" "-"
[ ! -e "$ri/.claude" ] && [ ! -e "$ri/AGENTS.md" ] && ok "init with no agent tool writes no agent file" || bad "init with no agent tool writes no agent file" "$(ls -A "$ri")"
ri=$(new_repo init-claude)
expect "init with claude installed: claude" "$(init_agents "$ri" "$(fake_tool claude)")" "claude"
ri=$(new_repo init-codex)
expect "init with codex installed: codex" "$(init_agents "$ri" "$(fake_tool codex)")" "codex"
ri=$(new_repo init-dotcodex); mkdir "$ri/.codex"
expect "init with .codex/: codex" "$(init_agents "$ri")" "codex"
ri=$(new_repo init-dotclaude); mkdir "$ri/.claude"
expect "init with .claude/: claude" "$(init_agents "$ri")" "claude"
ri=$(new_repo init-agentsmd); printf '# Mine\n' > "$ri/AGENTS.md"; git -C "$ri" add -A; git -C "$ri" commit -qm a
expect "init with an AGENTS.md: agents-md" "$(init_agents "$ri")" "agents-md"
ri=$(new_repo init-others)
others="$(fake_tool cursor):$(fake_tool gemini):$(fake_tool code)"
expect "init with cursor, gemini and code installed sets up none of them" "$(init_agents "$ri" "$others")" "-"
# `aver setup` sets up a found tool only with a sign of it in the
# repository, and says how to set it up anyway.
ri=$(new_repo setup-scoped)
out=$(cd "$ri" && PATH="$others:$T/bin:$BASE_PATH" aver setup --yes 2>&1)
expect "setup skips Cursor without .cursor/" "$out" "Skipping Cursor: nothing in this repository shows it uses it; \`aver setup --tool cursor\` sets it up."
expect "setup skips VS Code without Copilot instructions" "$out" "Skipping VS Code"
mkdir -p "$ri/.cursor"
out=$(cd "$ri" && PATH="$others:$T/bin:$BASE_PATH" aver setup --yes 2>&1)
expect "setup sets up Cursor once .cursor/ exists" "$out" "=== cursor"

################################################################################
section "3. aver setup --remove"

out=$(cd "$r1" && aver setup --remove --yes 2>&1)
expect "remove Claude Code" "$out" "Aver is no longer set up"
unchanged "every file as it was before Aver, byte for byte" "$r1" "$T/r1.s0"
dirs_now=$(snapdirs "$r1")
[ "$dirs_now" = "$(cat "$T/r1.d0")" ] && ok "the directories Aver made are gone" || bad "the directories Aver made are gone" "$(diff <(cat "$T/r1.d0") <(printf '%s\n' "$dirs_now"))"
python3 -c 'import json,sys; e=[x for x in json.load(open(sys.argv[1])).get("entries",[]) if x["repo"]==sys.argv[2]]; sys.exit(1 if e else 0)' "$AVER_HOME/integrations.json" "$r1" \
  && ok "the home forgets the repository" || bad "the home forgets the repository" "$(cat "$AVER_HOME/integrations.json")"
out=$(cd "$r1" && aver setup --remove --yes 2>&1)
expect "a second remove has nothing to do" "$out" "Aver has nothing set up"

out=$(cd "$r2" && aver setup --remove --yes 2>&1)
expect "remove every tool" "$out" "Aver is no longer set up"
for f in AGENTS.md GEMINI.md .github/copilot-instructions.md .cursor/rules/style.mdc; do
  want="$T/r2.$(echo "$f" | tr / _)"
  [ "$f" = AGENTS.md ] && { cp "$want" "$T/r2.agents.want"; printf '\n## Release\n\nTag from main.\n' >> "$T/r2.agents.want"; want="$T/r2.agents.want"; }
  same "$f back byte for byte" "$r2/$f" "$want"
done
[ ! -e "$r2/.cursor/rules/aver.mdc" ] && ok "Cursor's rule file is gone" || bad "Cursor's rule file is gone"

# A torn record: removal takes Aver's parts out of each file.
r3=$(new_repo torn-record)
mkdir -p "$r3/.claude"
printf '%s' "$user_settings" > "$r3/.claude/settings.json"
printf '# Project\n\nRun `make test`.\n' > "$r3/AGENTS.md"
printf '# Gemini notes\n' > "$r3/GEMINI.md"
git -C "$r3" add -A && git -C "$r3" commit -qm init
snapshot "$r3" > "$T/r3.s0"
(cd "$r3" && aver integrate claude --yes && aver integrate codex --yes && aver integrate gemini --yes) >/dev/null 2>&1
rec="$r3/.git/aver/integrate.json"
head -c $(( $(wc -c < "$rec") / 2 )) "$rec" > "$rec.half" && mv "$rec.half" "$rec"
out=$(cd "$r3" && aver setup --remove --yes 2>&1)
expect "a torn record is named and ignored" "$out" "is not a valid record"
expect "remove over a torn record finishes" "$out" "Aver is no longer set up"
same "torn record: AGENTS.md back byte for byte" "$r3/AGENTS.md" <(printf '# Project\n\nRun `make test`.\n')
same "torn record: GEMINI.md back byte for byte" "$r3/GEMINI.md" <(printf '# Gemini notes\n')
jsame "torn record: settings.json is the team's" "$r3/.claude/settings.json" "$user_settings"
same "torn record: settings.json back byte for byte" "$r3/.claude/settings.json" <(printf '%s' "$user_settings")
[ ! -e "$r3/.claude/skills" ] && [ ! -e "$r3/.claude/commands" ] && ok "torn record: the skill and commands are gone" || bad "torn record: the skill and commands are gone" "$(cd "$r3" && find .claude)"
[ ! -e "$rec" ] && ok "torn record: the record is gone" || bad "torn record: the record is gone"

# A torn file with a good record: back to what it was before Aver.
r4=$(new_repo torn-file)
mkdir -p "$r4/.claude"
printf '%s' "$user_settings" > "$r4/.claude/settings.json"
printf '# Project\n' > "$r4/AGENTS.md"
git -C "$r4" add -A && git -C "$r4" commit -qm init
snapshot "$r4" > "$T/r4.s0"
(cd "$r4" && aver integrate claude --yes && aver integrate agents-md --yes) >/dev/null 2>&1
head -c 40 "$r4/.claude/settings.json" > "$T/torn" && cp "$T/torn" "$r4/.claude/settings.json"
out=$(cd "$r4" && aver setup --remove --yes 2>&1)
expect "remove over a torn settings.json finishes" "$out" "Aver is no longer set up"
unchanged "torn file: every file back byte for byte" "$r4" "$T/r4.s0"

# Remove after the user edited a file Aver wrote: their edit stays.
r5=$(new_repo edited)
printf '# Project\n' > "$r5/AGENTS.md"; git -C "$r5" add -A; git -C "$r5" commit -qm init
(cd "$r5" && aver integrate agents-md --yes) >/dev/null 2>&1
printf '\nMine, after Aver.\n' >> "$r5/AGENTS.md"
(cd "$r5" && aver setup --remove --yes) >/dev/null 2>&1
same "an edit after Aver's write stays, Aver's block goes" "$r5/AGENTS.md" <(printf '# Project\n\nMine, after Aver.\n')

################################################################################
section "4. aver doctor without node, go or cargo"

r7=$(new_repo polyglot)
printf '{"name":"p","version":"1.0.0","scripts":{"test":"node --test"}}\n' > "$r7/package.json"
printf '{"name":"p","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"p","version":"1.0.0"}}}\n' > "$r7/package-lock.json"
mkdir -p "$r7/src" "$r7/gopkg"
printf '[package]\nname = "p"\nversion = "0.1.0"\nedition = "2021"\n' > "$r7/Cargo.toml"
printf 'pub fn f() {}\n' > "$r7/src/lib.rs"
printf 'module example.com/p\n\ngo 1.21\n' > "$r7/go.mod"
printf 'package p\n' > "$r7/p.go"
git -C "$r7" add -A && git -C "$r7" commit -qm poly
o=$(cd "$r7" && PATH="$T/bin:/usr/bin:/bin" aver doctor 2>&1; echo "exit=$?")
printf '%s\n' "$o" | sed 's/^/       | /'
expect "doctor: npm missing" "$o" "npm is not on PATH"
expect "doctor: npm's fix" "$o" "fix: install Node.js with npm: https://nodejs.org"
expect "doctor: cargo missing" "$o" "cargo is not on PATH"
expect "doctor: cargo's fix" "$o" "fix: install Rust: https://rustup.rs"
expect "doctor: go missing" "$o" "go is not on PATH"
expect "doctor: go's fix" "$o" "fix: install Go: https://go.dev/dl/"
expect "doctor without the tools still exits 0 (they are warnings)" "$o" "exit=0"
# Go older than go.mod asks for, under GOTOOLCHAIN=local (under the default
# auto, Go fetches the toolchain go.mod names and doctor says nothing).
if command -v go >/dev/null 2>&1 || [ -x /opt/homebrew/bin/go ]; then
  gobin=$(PATH="$PATH:/opt/homebrew/bin:/usr/local/go/bin" command -v go)
  mkdir -p "$T/gobin" && ln -sf "$gobin" "$T/gobin/go"
  printf 'module example.com/p\n\ngo 1.99\n' > "$r7/go.mod"
  o=$(cd "$r7" && GOTOOLCHAIN=local PATH="$T/gobin:$T/bin:/usr/bin:/bin" aver doctor 2>&1; echo "exit=$?")
  printf '%s\n' "$o" | grep -A1 toolchain | sed 's/^/       | /'
  expect "doctor: Go older than go.mod" "$o" "1.99"
  expect "doctor: Go's toolchain fix" "$o" "fix:"
else
  skip "no go here for the toolchain-age check"
fi
r8=$(new_repo pyonly)
printf '[project]\nname = "p"\nversion = "0"\n\n[tool.pytest.ini_options]\ntestpaths = ["tests"]\n\n[project.optional-dependencies]\ntest = ["pytest"]\n' > "$r8/pyproject.toml"
mkdir "$r8/tests" && printf 'def test_x():\n    assert True\n' > "$r8/tests/test_x.py"
git -C "$r8" add -A && git -C "$r8" commit -qm py
mkdir -p "$T/gitbin" && ln -sf "$(command -v git)" "$T/gitbin/git"
o=$(cd "$r8" && PATH="$T/bin:$T/gitbin:/bin:/usr/sbin:/sbin" aver doctor 2>&1; echo "exit=$?")
printf '%s\n' "$o" | sed 's/^/       | /'
expect "doctor: no python" "$o" "neither python nor python3 is on PATH"

################################################################################
printf '\n%d passed, %d failed\n' "$pass" "${#failures[@]}"
if [ "${#failures[@]}" -gt 0 ]; then
  printf '  - %s\n' "${failures[@]}"
  exit 1
fi
