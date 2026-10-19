#!/usr/bin/env bash
# The Claude Code onboarding journey of a brand-new user, end to end, with
# no model call and no network: the plugin validated and installed from the
# local marketplace, its hooks on a machine without aver, /aver's steps by
# hand (install.sh from a local release, aver doctor, aver init --yes), a
# worktree session through the committed hooks (brief, WorktreeCreate, an
# edit, Stop blocking until aver verify, WorktreeRemove), the plugin and
# the committed settings acting once per event, and a teammate without aver.
#
# Everything lives in a throwaway HOME, CLAUDE_CONFIG_DIR and AVER_HOME.
#
#   tests/e2e/plugin-onboarding.sh           # aver from PATH
#   AVER=<binary> tests/e2e/plugin-onboarding.sh
#   E2E_KEEP=1 ...                           # keep the work directory
#
# Needs git and a POSIX sh; `claude` (for validate and install) is used when
# found and the plugin steps are skipped with a note otherwise.
set -uo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
real_aver=${AVER:-$(command -v aver || true)}
[ -x "$real_aver" ] || { echo "plugin-onboarding: no aver binary (set AVER)" >&2; exit 2; }
real_claude=${CLAUDE:-$(command -v claude || true)}
real_aver=$(cd "$(dirname "$real_aver")" && pwd)/$(basename "$real_aver")

T=$(mktemp -d "${TMPDIR:-/tmp}/aver-onboard.XXXXXX")
T=$(cd "$T" && pwd -P)
cleanup() { [ -n "${E2E_KEEP:-}" ] && echo "kept $T" || rm -rf "$T"; }
trap cleanup EXIT

# A clean machine: nothing of the real user's, and a PATH without aver.
export HOME="$T/home" CLAUDE_CONFIG_DIR="$T/claude" AVER_HOME="$T/home/.aver"
mkdir -p "$HOME" "$CLAUDE_CONFIG_DIR"
unset AVER_BIN AVER_URL AVER_RUN CLAUDE_PLUGIN_ROOT GIT_DIR GIT_WORK_TREE
export GIT_CONFIG_GLOBAL="$HOME/.gitconfig" GIT_CONFIG_NOSYSTEM=1
git config --global user.name "New User"
git config --global user.email "new@example.com"
git config --global init.defaultBranch main
BASE_PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH=$BASE_PATH
for d in /opt/homebrew/bin /usr/local/bin; do
  if [ -e "$d/aver" ]; then
    echo "plugin-onboarding: $d/aver exists; the plugin's hooks would find it, so 'no aver' cannot be simulated here" >&2
    exit 2
  fi
done

pass=0
failures=()
ok() { pass=$((pass + 1)); printf '  ok   %s\n' "$1"; }
bad() { failures+=("$1"); printf '  FAIL %s\n' "$1"; [ -n "${2:-}" ] && printf '%s\n' "$2" | sed 's/^/       | /'; }
check() { # <name> <cmd…>: ok when the command exits 0
  local name=$1; shift
  if "$@"; then ok "$name"; else bad "$name"; fi
}
notes=()
note() { notes+=("$1"); printf '  note %s\n' "$1"; }
section() { printf '\n== %s\n' "$1"; }
contains() { case $1 in *"$2"*) return 0 ;; *) return 1 ;; esac; }
json_get() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
for k in sys.argv[2].split("."):
    d=d[int(k)] if isinstance(d,list) else d.get(k) if d is not None else None
print("" if d is None else (d if isinstance(d,str) else json.dumps(d)))' "$@"; }

# hook <outfile-prefix> <cwd> <command> <stdin-json>: runs one hook command
# the way Claude Code does (sh -c, input on stdin, CLAUDE_PLUGIN_ROOT set for
# plugin hooks); sets $rc, $out, $err.
hook() {
  local p=$1 dir=$2 cmd=$3 input=$4
  printf '%s' "$input" >"$p.in"
  (cd "$dir" && CLAUDE_PROJECT_DIR=$dir sh -c "$cmd" <"$p.in" >"$p.out" 2>"$p.err")
  rc=$?
  out=$(cat "$p.out")
  err=$(cat "$p.err")
}
input() { # <event> <cwd> [extra json fields]
  printf '{"session_id":"%s","transcript_path":"%s/t.jsonl","cwd":"%s","hook_event_name":"%s"%s}' \
    "${SESSION:-s1}" "$T" "$2" "$1" "${3:-}"
}

# The plugin's hook commands by event, from hooks.json.
plugin_cmd() { # <plugin root> <event>
  python3 -c 'import json,sys
h=json.load(open(sys.argv[1]+"/hooks/hooks.json"))["hooks"].get(sys.argv[2],[])
print("\n".join(x["command"] for g in h for x in g["hooks"]))' "$1" "$2"
}
settings_cmd() { # <repo> <event>
  python3 -c 'import json,sys
h=json.load(open(sys.argv[1]+"/.claude/settings.json"))["hooks"].get(sys.argv[2],[])
print("\n".join(x["command"] for g in h for x in g["hooks"] if " hook " in x["command"]))' "$1" "$2"
}

# A fresh project: fixtures/sh-calc as a new user's repository.
repo=$T/calc
mkdir -p "$repo"
cp -R "$root/fixtures/sh-calc/." "$repo/"
git -C "$repo" init -q
git -C "$repo" add -A
git -C "$repo" commit -qm "calc"

##############################################################################
section "1. plugin: validate and install from the local marketplace"
plugin_root=$root/integrations/claude-code
if [ -x "$real_claude" ]; then
  out=$("$real_claude" plugin validate "$root/integrations/claude-code" </dev/null 2>&1)
  [ $? -eq 0 ] && ok "claude plugin validate integrations/claude-code" || bad "claude plugin validate integrations/claude-code" "$out"
  out=$(cd "$root" && "$real_claude" plugin validate . </dev/null 2>&1)
  [ $? -eq 0 ] && ok "claude plugin validate . (marketplace)" || bad "claude plugin validate ." "$out"
  out=$("$real_claude" plugin marketplace add "$root" </dev/null 2>&1)
  [ $? -eq 0 ] && ok "claude plugin marketplace add <checkout>" || bad "marketplace add" "$out"
  out=$("$real_claude" plugin install aver@aver --scope user </dev/null 2>&1)
  [ $? -eq 0 ] && ok "claude plugin install aver@aver" || bad "plugin install" "$out"
  installed=$(json_get "$CLAUDE_CONFIG_DIR/plugins/installed_plugins.json" plugins.aver@aver.0.installPath 2>/dev/null)
  if [ -n "$installed" ] && [ -f "$installed/hooks/hooks.json" ]; then
    ok "installed copy at \$CLAUDE_CONFIG_DIR/plugins/cache"
    plugin_root=$installed
  else
    bad "installed plugin path in installed_plugins.json" "$(cat "$CLAUDE_CONFIG_DIR/plugins/installed_plugins.json" 2>&1)"
  fi
  details=$("$real_claude" plugin details aver@aver </dev/null 2>&1)
  contains "$details" "SessionStart, Stop, SubagentStop" && ok "plugin details lists its three hooks" || bad "plugin details hooks" "$details"
  case $(printf '%s\n' "$details" | sed -n 's/^ *Skills ([0-9]*) *//p') in
    *"aver, aver"*) note "the /aver command and the aver skill share the name aver ($(printf '%s\n' "$details" | grep -o 'Skills (.*' | tr -s ' '))" ;;
  esac
else
  echo "  note: claude is not on PATH; skipped validate and install (hooks run from the checkout)"
fi
export CLAUDE_PLUGIN_ROOT=$plugin_root

##############################################################################
section "2. plugin hooks with no aver on PATH"
for event in SessionStart Stop SubagentStop; do
  while IFS= read -r cmd; do
    [ -n "$cmd" ] || continue
    extra=
    case $event in
      SessionStart) extra=',"source":"startup"' ;;
      Stop) extra=',"stop_hook_active":false,"last_assistant_message":"Done."' ;;
      SubagentStop) extra=',"stop_hook_active":false,"agent_id":"a1","agent_type":"general-purpose","agent_transcript_path":"/x/a.jsonl"' ;;
    esac
    hook "$T/noaver-$event" "$repo" "$cmd" "$(input "$event" "$repo" "$extra")"
    [ $rc -eq 0 ] && ok "$event exits 0 without aver" || bad "$event exit $rc without aver" "$err"
    if [ "$event" = SessionStart ]; then
      ctx=$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["hookSpecificOutput"]["additionalContext"])' 2>&1)
      contains "$ctx" "/aver" && ok "SessionStart tells the session to run /aver" || bad "SessionStart /aver hint" "$out"
    else
      [ -z "$out$err" ] && ok "$event prints nothing" || bad "$event output without aver" "$out$err"
    fi
  done < <(plugin_cmd "$plugin_root" "$event")
done
[ ! -e "$AVER_HOME" ] && ok "no aver home created" || bad "hooks without aver created $AVER_HOME"

##############################################################################
section "3. /aver by hand: install.sh, aver doctor, aver init --yes"
case $(uname -s)-$(uname -m) in
  Darwin-arm64) asset=aver-darwin-aarch64 ;;
  Darwin-x86_64) asset=aver-darwin-x86_64 ;;
  Linux-x86_64) asset=aver-linux-x86_64 ;;
  Linux-aarch64) asset=aver-linux-aarch64 ;;
  *) echo "unsupported platform" >&2; exit 2 ;;
esac
version=$("$real_aver" --version | awk '{print $2}')
rel=$T/release/download/v$version
mkdir -p "$rel"
cp "$real_aver" "$rel/$asset"
(cd "$rel" && shasum -a 256 "$asset" >SHA256SUMS)
out=$(SHELL=/bin/zsh AVER_RELEASE_BASE_URL="file://$T/release" AVER_VERSION="$version" sh "$root/site/install.sh" 2>&1)
rc=$?
echo "$out" >"$T/install.log"
[ $rc -eq 0 ] && [ -x "$HOME/.local/bin/aver" ] && ok "install.sh puts aver in ~/.local/bin" || bad "install.sh exit $rc" "$out"
contains "$out" "is not on your PATH" && contains "$out" "~/.zshrc" && ok "install.sh says how to add ~/.local/bin to PATH (zsh)" || bad "install.sh PATH hint" "$out"
# /aver runs `aver init --yes` (aver.toml and the agent tools); install.sh
# names `aver setup` (the agent tools alone). Both are documented; noted.
contains "$out" "aver setup" && note "install.sh's next steps name 'aver setup', /aver runs 'aver init --yes'"
export PATH="$HOME/.local/bin:$BASE_PATH"
check "aver on PATH is the installed copy" test "$(command -v aver)" = "$HOME/.local/bin/aver"

out=$(cd "$repo" && aver doctor 2>&1)
rc=$?
echo "$out" >"$T/doctor.log"
[ $rc -eq 0 ] && ok "aver doctor exits 0" || bad "aver doctor exit $rc" "$out"

out=$(cd "$repo" && aver init --yes 2>&1)
rc=$?
echo "$out" >"$T/init.log"
[ $rc -eq 0 ] && ok "aver init --yes exits 0" || bad "aver init --yes exit $rc" "$out"
check "aver init kept the fixture's aver.toml" cmp -s "$repo/aver.toml" "$root/fixtures/sh-calc/aver.toml"
check "aver init wrote .claude/settings.json" test -f "$repo/.claude/settings.json"
check "aver init wrote the aver skill" test -f "$repo/.claude/skills/aver/SKILL.md"
check "aver init wrote /aver-verify" test -f "$repo/.claude/commands/aver-verify.md"
for event in SessionStart Stop SubagentStop WorktreeCreate WorktreeRemove; do
  c=$(settings_cmd "$repo" "$event")
  contains "$c" "command -v aver" && ok "settings $event hook guards on command -v aver" || bad "settings $event hook" "$c"
done
git -C "$repo" add -A
git -C "$repo" commit -qm "Set up Aver"
check "the setup commits cleanly" test -z "$(git -C "$repo" status --porcelain)"

# Apps started from the Dock get launchd's PATH, without ~/.local/bin: the
# plugin's hooks still find aver there (scripts/aver.sh); the committed
# settings' hooks, guarded by `command -v aver`, do nothing.
SESSION=dock1
hook "$T/dock-plugin" "$repo" "$(plugin_cmd "$plugin_root" SessionStart)" "$(input SessionStart "$repo" ',"source":"startup"')"
[ $rc -eq 0 ] && contains "$out" "aver workspace new" && ok "with launchd's PATH the plugin's SessionStart finds ~/.local/bin/aver" || bad "plugin SessionStart with launchd PATH (exit $rc)" "$out$err"
SESSION=dock2
(export PATH=$BASE_PATH; hook "$T/dock-settings" "$repo" "$(settings_cmd "$repo" SessionStart)" "$(input SessionStart "$repo" ',"source":"startup"')"; [ -z "$(cat "$T/dock-settings.out")" ] && echo silent >"$T/dock-settings.flag")
[ -f "$T/dock-settings.flag" ] && note "with launchd's PATH the committed settings' hooks find no aver and do nothing (only the plugin's act)"
sleep 6 # past the claims' grace

##############################################################################
section "4. a worktree session through the committed hooks, aver on PATH"
# The plugin is installed too; each event is claimed once, so the settings'
# hook below does the work and the plugin's (run after) does nothing.
SESSION=root1
hook "$T/root-start" "$repo" "$(settings_cmd "$repo" SessionStart)" "$(input SessionStart "$repo" ',"source":"startup"')"
[ $rc -eq 0 ] && contains "$out" "aver workspace new" && ok "SessionStart in the checkout points to aver workspace new" || bad "root SessionStart (exit $rc)" "$out$err"

SESSION=wt1
hook "$T/wt-create" "$repo" "$(settings_cmd "$repo" WorktreeCreate)" "$(input WorktreeCreate "$repo" ',"name":"fix-sub"')"
ws=$(printf '%s\n' "$out" | tail -n 1)
if [ $rc -eq 0 ] && [ -f "$ws/.aver/workspace.toml" ]; then
  ok "WorktreeCreate prints an Aver workspace directory"
else
  bad "WorktreeCreate (exit $rc)" "$out
$err"
  ws=
fi
if [ -n "$ws" ]; then
  [ -z "$(git -C "$ws" status --porcelain 2>&1)" ] && ok "git status is clean in the new workspace" || bad "git status in the workspace" "$(git -C "$ws" status --porcelain 2>&1)"

  hook "$T/wt-start" "$ws" "$(settings_cmd "$repo" SessionStart)" "$(input SessionStart "$ws" ',"source":"startup"')"
  [ $rc -eq 0 ] && contains "$out" "aver verify" && ok "SessionStart in the workspace gives the brief" || bad "workspace SessionStart (exit $rc)" "$out$err"
  hook "$T/wt-start-plugin" "$ws" "$(plugin_cmd "$plugin_root" SessionStart)" "$(input SessionStart "$ws" ',"source":"startup"')"
  [ $rc -eq 0 ] && [ -z "$out" ] && ok "the plugin's SessionStart for the same event does nothing" || bad "plugin SessionStart duplicate (exit $rc)" "$out$err"

  hook "$T/stop0" "$ws" "$(settings_cmd "$repo" Stop)" "$(input Stop "$ws" ',"stop_hook_active":false,"last_assistant_message":"Nothing to do."')"
  [ $rc -eq 0 ] && [ -z "$out" ] && ok "Stop with no change lets the session stop" || bad "Stop on an unchanged workspace (exit $rc)" "$out$err"

  # The edit: a sub function with a test for it.
  printf 'sub() { echo $(($1 - $2)); }\n' >>"$ws/calc.sh"
  printf 'subtracts :: [ "$(sub 5 3)" = 2 ]\n' >>"$ws/test/add.tests"

  hook "$T/stop1" "$ws" "$(settings_cmd "$repo" Stop)" "$(input Stop "$ws" ',"stop_hook_active":false,"last_assistant_message":"Added sub."')"
  decision=$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("decision",""))' 2>/dev/null)
  [ $rc -eq 0 ] && [ "$decision" = block ] && ok "Stop blocks on an unverified change" || bad "Stop after the edit (exit $rc, decision '$decision')" "$out$err"
  reason=$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("reason",""))' 2>/dev/null)
  contains "$reason" "aver verify" && ok "the block says to run aver verify" || bad "Stop block reason" "$reason"
  printf '%s\n' "$reason" >"$T/stop-reason.txt"

  hook "$T/stop1b" "$ws" "$(plugin_cmd "$plugin_root" Stop)" "$(input Stop "$ws" ',"stop_hook_active":false,"last_assistant_message":"Added sub."')"
  [ $rc -eq 0 ] && [ -z "$out" ] && ok "the plugin's Stop for the same event does nothing" || bad "plugin Stop duplicate (exit $rc)" "$out$err"

  out=$(cd "$ws" && aver verify 2>"$T/verify.err")
  rc=$?
  echo "$out" >"$T/verify.out"
  [ $rc -eq 0 ] && ok "aver verify in the workspace exits 0" || bad "aver verify exit $rc" "$out
$(tail -n 20 "$T/verify.err")"
  contains "$out" "PASS" && ok "aver verify says PASS" || bad "aver verify verdict" "$(printf '%s\n' "$out" | head -n 20)"

  sleep 6 # past the claim's grace, as a real next Stop would be
  hook "$T/stop2" "$ws" "$(settings_cmd "$repo" Stop)" "$(input Stop "$ws" ',"stop_hook_active":true,"last_assistant_message":"Verified."')"
  [ $rc -eq 0 ] && [ -z "$out" ] && ok "Stop lets go after the PASS" || bad "Stop after verify (exit $rc)" "$out$err"

  # Committing in the worktree keeps the branch; WorktreeRemove then keeps
  # the directory and says how to fetch. Without a commit it removes it.
  hook "$T/wt-remove" "$repo" "$(settings_cmd "$repo" WorktreeRemove)" "$(input WorktreeRemove "$repo" ",\"worktree_path\":\"$ws\"")"
  [ $rc -eq 0 ] && ok "WorktreeRemove exits 0" || bad "WorktreeRemove (exit $rc)" "$out$err"
  [ ! -e "$ws" ] && ok "WorktreeRemove removed the directory" || bad "WorktreeRemove left $ws" "$out$err"
fi

##############################################################################
section "5. plugin and settings together: each event acts once"
# Claude Code starts every matching hook of an event at once.
both() { # <prefix> <dir> <event> <input>
  local p=$1 dir=$2 ev=$3 in=$4
  printf '%s' "$in" >"$p.in"
  (cd "$dir" && sh -c "$(settings_cmd "$repo" "$ev")" <"$p.in" >"$p.s.out" 2>"$p.s.err") &
  local a=$!
  (cd "$dir" && sh -c "$(plugin_cmd "$plugin_root" "$ev")" <"$p.in" >"$p.p.out" 2>"$p.p.err") &
  local b=$!
  wait $a; rc_s=$?
  wait $b; rc_p=$?
}
SESSION=dup1
both "$T/dup-start" "$repo" SessionStart "$(input SessionStart "$repo" ',"source":"startup"')"
n=$(cat "$T/dup-start.s.out" "$T/dup-start.p.out" | grep -c additionalContext)
[ $rc_s -eq 0 ] && [ $rc_p -eq 0 ] && [ "$n" = 1 ] && ok "SessionStart: one of the two hooks answers" || bad "SessionStart dedup: $n answers (exit $rc_s/$rc_p)" "$(cat "$T"/dup-start.*.out "$T"/dup-start.*.err)"

SESSION=dup2
wtin=$(input WorktreeCreate "$repo" ',"name":"dup-wt"')
printf '%s' "$wtin" >"$T/dupwt.in"
before=$(ls "$AVER_HOME/work"/*/ 2>/dev/null | wc -l)
(cd "$repo" && sh -c "$(settings_cmd "$repo" WorktreeCreate)" <"$T/dupwt.in" >"$T/dupwt.a" 2>"$T/dupwt.ae") &
a=$!
(cd "$repo" && sh -c "$(settings_cmd "$repo" WorktreeCreate)" <"$T/dupwt.in" >"$T/dupwt.b" 2>"$T/dupwt.be") &
b=$!
wait $a; ra=$?
wait $b; rb=$?
da=$(tail -n 1 "$T/dupwt.a"); db=$(tail -n 1 "$T/dupwt.b")
after=$(ls "$AVER_HOME/work"/*/ 2>/dev/null | wc -l)
[ $ra -eq 0 ] && [ $rb -eq 0 ] && [ -n "$da" ] && [ "$da" = "$db" ] && ok "two WorktreeCreate hooks print the same directory" || bad "WorktreeCreate dedup (exit $ra/$rb): '$da' vs '$db'" "$(cat "$T"/dupwt.ae "$T"/dupwt.be)"
[ $((after - before)) -eq 1 ] && ok "one workspace directory made, not two" || bad "WorktreeCreate dedup made $((after - before)) directories"
ws2=$da
if [ -d "$ws2" ]; then
  printf 'mul() { echo $(($1 * $2)); }\n' >>"$ws2/calc.sh"
  printf 'multiplies :: [ "$(mul 2 3)" = 6 ]\n' >>"$ws2/test/add.tests"
  SESSION=dup3
  both "$T/dup-stop" "$ws2" Stop "$(input Stop "$ws2" ',"stop_hook_active":false,"last_assistant_message":"Added mul."')"
  n=$(cat "$T/dup-stop.s.out" "$T/dup-stop.p.out" | grep -c '"decision"')
  [ $rc_s -eq 0 ] && [ $rc_p -eq 0 ] && [ "$n" = 1 ] && ok "Stop: one block, not two" || bad "Stop dedup: $n blocks (exit $rc_s/$rc_p)" "$(cat "$T"/dup-stop.*)"
  SESSION=dup4
  both "$T/dup-sub" "$ws2" SubagentStop "$(input SubagentStop "$ws2" ',"stop_hook_active":false,"agent_id":"a9","agent_type":"general-purpose"')"
  [ $rc_s -eq 0 ] && [ $rc_p -eq 0 ] && ok "SubagentStop from both exits 0" || bad "SubagentStop both (exit $rc_s/$rc_p)" "$(cat "$T"/dup-sub.*)"
  hook "$T/dup-rm" "$repo" "$(settings_cmd "$repo" WorktreeRemove)" "$(input WorktreeRemove "$repo" ",\"worktree_path\":\"$ws2\"")"
  [ $rc -eq 0 ] && [ ! -e "$ws2" ] && ok "WorktreeRemove of the shared directory" || bad "WorktreeRemove after dedup (exit $rc)" "$out$err"
fi

##############################################################################
section "6. a teammate without aver clones the repository"
mate=$T/mate
mkdir -p "$mate/home"
git clone -q "$repo" "$mate/calc"
(
  export HOME="$mate/home" PATH=$BASE_PATH
  unset AVER_HOME CLAUDE_PLUGIN_ROOT
  export GIT_CONFIG_GLOBAL="$HOME/.gitconfig"
  git config --global user.name Mate
  git config --global user.email mate@example.com
  r=$mate/calc
  fails=0
  for event in SessionStart Stop SubagentStop; do
    hook "$mate/$event" "$r" "$(settings_cmd "$r" "$event")" "$(input "$event" "$r")"
    if [ $rc -eq 0 ] && [ -z "$out$err" ]; then ok "teammate: $event exits 0 silently"; else bad "teammate: $event (exit $rc)" "$out$err"; fi
  done
  hook "$mate/wtc" "$r" "$(settings_cmd "$r" WorktreeCreate)" "$(input WorktreeCreate "$r" ',"name":"mate-wt"')"
  d=$(printf '%s\n' "$out" | tail -n 1)
  if [ $rc -eq 0 ] && [ -d "$d" ] && [ "$(git -C "$d" rev-parse --abbrev-ref HEAD)" = worktree-mate-wt ]; then
    ok "teammate: WorktreeCreate falls back to a plain git worktree"
  else
    bad "teammate: WorktreeCreate (exit $rc)" "$out$err"
  fi
  [ -z "$err" ] || bad "teammate: WorktreeCreate wrote to stderr" "$err"
  hook "$mate/wtr" "$r" "$(settings_cmd "$r" WorktreeRemove)" "$(input WorktreeRemove "$r" ",\"worktree_path\":\"$d\"")"
  [ $rc -eq 0 ] && [ ! -e "$d" ] && ok "teammate: WorktreeRemove removes it" || bad "teammate: WorktreeRemove (exit $rc)" "$out$err"
  [ ! -e "$HOME/.aver" ] && ok "teammate: no ~/.aver created" || bad "teammate: ~/.aver created"
  printf '%s\n' "${failures[@]+"${failures[@]}"}" >"$mate/failures"
  echo $pass >"$mate/pass"
)
pass=$(cat "$mate/pass")
failures=()
while IFS= read -r f; do [ -n "$f" ] && failures+=("$f"); done <"$mate/failures"

##############################################################################
printf '\n%d passed, %d failed\n' "$pass" "${#failures[@]}"
for f in "${failures[@]+"${failures[@]}"}"; do printf '  - %s\n' "$f"; done
for f in "${notes[@]+"${notes[@]}"}"; do printf '  note: %s\n' "$f"; done
[ ${#failures[@]} -eq 0 ]
