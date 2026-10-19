#!/usr/bin/env bash
# Manual trial of `aver run` with the installed Claude Code CLI on a copy of
# fixtures/calc. Not run in CI: it spends real tokens and needs Node and
# a logged-in `claude`. Uses a throwaway AVER_HOME unless one is given.
#
#   scripts/try-run.sh                # 2 agents
#   AGENTS=3 scripts/try-run.sh
#   KEEP=1 scripts/try-run.sh         # keep the temporary home and repo
set -euo pipefail
cd "$(dirname "$0")/.."

AGENTS="${AGENTS:-2}"
command -v claude >/dev/null || { echo "claude (Claude Code) is not on PATH" >&2; exit 1; }

cargo build --release --quiet -p aver
AVER="$PWD/target/release/aver"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aver-try-run.XXXXXX")"
export AVER_HOME="${AVER_HOME:-$WORK/home}"
cleanup() {
  if [ -z "${KEEP:-}" ]; then rm -rf "$WORK"; else echo "kept $WORK"; fi
}
trap cleanup EXIT

REPO="$WORK/calc"
mkdir -p "$REPO"
cp -R fixtures/calc/. "$REPO/"
rm -rf "$REPO/node_modules"
git -C "$REPO" init -q --initial-branch=main
git -C "$REPO" add -A
git -C "$REPO" -c user.name=trial -c user.email=trial@example.invalid commit -qm calc

"$AVER" init >/dev/null
cd "$REPO"
set +e
"$AVER" run "Add a negate(x) function to src/math.js that returns -x" \
  --agents "$AGENTS" --agent claude --timeout 15m \
  -c "neg: negate(x) returns -x for positive and negative numbers"
status=$?
set -e
"$AVER" status
echo
echo "Try: (cd $REPO && $AVER accept <n>)   # set KEEP=1 to keep this trial"
exit "$status"
