#!/bin/bash
# setup.sh <clip>: the state a clip's tapes start from, under /tmp/aver-social/<clip>/
# (left/ for the plain tool, right/ and home/ for Aver). Never ~/.aver.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/prompt.sh"
clip=$1
W=/tmp/aver-social/$clip
export AVER_HOME=$W/home
rm -rf "$W"
mkdir -p "$W/left" "$W/right"

calc() { "$here/../hero/make-repo.sh" "$1" >/dev/null; }

# merge-day: sh-calc with three stub functions; each agent fixes one and
# appends its test to the same file (bench/verify-10-of-10.sh's b10, three tasks).
shcalc() {
  cp -R "$here/../../fixtures/sh-calc" "$1"
  printf '\nsub() { echo 0; }\n\nmul() { echo 0; }\n\nneg() { echo 0; }\n' >>"$1/calc.sh"
  git -C "$1" init -q -b main
  git -C "$1" config user.name demo
  git -C "$1" config user.email demo@example.com
  git -C "$1" add -A
  git -C "$1" commit -qm base
}
fix() { # <dir> <fn> <expr> <call> <expected>
  sed "s/^$2() { echo 0; }\$/$2() { echo \$(($3)); }/" "$1/calc.sh" >"$1/calc.new" && mv "$1/calc.new" "$1/calc.sh"
  printf '[crit:%s] %s works :: [ "$(%s)" = %s ]\n' "$2" "$2" "$4" "$5" >>"$1/test/add.tests"
}
tasks=("sub|\$1 - \$2|sub 5 3|2" "mul|\$1 * \$2|mul 4 3|12" "neg|0 - \$1|neg 4|-4")

case $clip in
  lie-detector | quiet-rewrite)
    calc "$W/left/calc"
    calc "$W/right/calc"
    (cd "$W/right/calc" && aver workspace new --label agent-1 2>/dev/null | tail -n 1) >"$W/ws"
    ;;
  empty-test | scale)
    calc "$W/left/calc"
    calc "$W/right/calc"
    ;;
  merge-day)
    shcalc "$W/left/calc"
    shcalc "$W/right/calc"
    for t in "${tasks[@]}"; do
      IFS='|' read -r n e c x <<<"$t"
      git -C "$W/left/calc" checkout -q -b "$n" main
      fix "$W/left/calc" "$n" "$e" "$c" "$x"
      git -C "$W/left/calc" commit -qam "$n works"
      git -C "$W/left/calc" checkout -q main
      ws=$(cd "$W/right/calc" && aver workspace new --no-deps --label "$n" -c "$n: $n works" 2>/dev/null | tail -n 1)
      fix "$ws" "$n" "$e" "$c" "$x"
      (cd "$ws" && aver verify 2>/dev/null | sed -n "1s/^/$n: /p") >&2
    done
    ;;
  *) echo "unknown clip $clip" >&2; exit 2 ;;
esac
