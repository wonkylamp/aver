#!/bin/sh
# A scripted fake agent for tests/e2e/run-batch.sh. It runs in its workspace
# directory and does what its role says: the role is the word after `role=`
# in the brief's TASK line, or, when FAKE_ROLES is set ("breaker honest
# gamer"), the AVER_AGENT_INDEX-th word of it. Every role ends with
# `aver verify`, as a real agent would.
set -u
task=$(grep '^TASK:' .aver/BRIEF.md || true)
role=$(printf '%s\n' "$task" | sed -n 's/.*role=\([a-z0-9_-]*\).*/\1/p')
if [ -n "${FAKE_ROLES:-}" ]; then
  # shellcheck disable=SC2086
  set -- $FAKE_ROLES
  shift $((AVER_AGENT_INDEX - 1))
  role=$1
fi
echo "fake agent ${AVER_AGENT_INDEX:-?}: role $role"

# edit <sed script> <file>: sed -i without the BSD and GNU difference.
edit() { sed "$1" "$2" >"$2.new" && mv "$2.new" "$2"; }

mul='mul() { echo $(($1 * $2)); }'
sub='sub() { echo $(($1 - $2)); }'
case $role in
  honest)  # mul, and a test that fails without it
    printf '%s\n' "$mul" >>calc.sh
    printf '%s\n' 'multiplies [crit:mul] :: [ "$(mul 3 4)" = 12 ]' >>test/add.tests
    ;;
  gamer)  # mul, plus an existing test weakened so a broken add passes
    printf '%s\n' "$mul" >>calc.sh
    printf '%s\n' 'multiplies [crit:mul] :: [ "$(mul 3 4)" = 12 ]' >>test/add.tests
    edit 's/^add() .*/add() { echo $(($1 + ${2#-})); }/' calc.sh
    edit 's/^adds a negative :: .*/adds a negative :: true/' test/add.tests
    ;;
  breaker)  # mul, and add broken for negatives
    printf '%s\n' "$mul" >>calc.sh
    printf '%s\n' 'multiplies [crit:mul] :: [ "$(mul 3 4)" = 12 ]' >>test/add.tests
    edit 's/^add() .*/add() { echo $(($1 + ${2#-})); }/' calc.sh
    ;;
  sub)  # a second task: sub
    printf '%s\n' "$sub" >>calc.sh
    printf '%s\n' 'subtracts [crit:sub] :: [ "$(sub 5 3)" = 2 ]' >>test/add.tests
    ;;
  square)  # builds on mul and sub: both must be in the copy it starts on
    grep -q '^mul()' calc.sh && grep -q '^sub()' calc.sh || {
      echo "square: started without mul and sub" >&2
      exit 3
    }
    printf '%s\n' 'sq() { mul "$1" "$1"; }' >>calc.sh
    printf '%s\n' 'squares [crit:sq] :: [ "$(sq 3)" = 9 ]' >>test/add.tests
    ;;
  second-try)  # gives up on the first run of its task, then works like honest
    mark="$FAKE_STATE/second-try"
    if [ ! -e "$mark" ]; then
      touch "$mark"
      echo "first try: giving up" >&2
      exit 1
    fi
    printf '%s\n' "$mul" >>calc.sh
    printf '%s\n' 'multiplies [crit:mul] :: [ "$(mul 3 4)" = 12 ]' >>test/add.tests
    ;;
  mul-inline)  # mul on add's line: conflicts with sub-inline
    edit 's/^add() .*/add() { echo $(($1 + $2)); }; mul() { echo $(($1 * $2)); }/' calc.sh
    printf '%s\n' 'multiplies [crit:mul] :: [ "$(mul 3 4)" = 12 ]' >>test/add.tests
    ;;
  sub-inline)
    edit 's/^add() .*/add() { echo $(($1 + $2)); }; sub() { echo $(($1 - $2)); }/' calc.sh
    printf '%s\n' 'subtracts [crit:sub] :: [ "$(sub 5 3)" = 2 ]' >>test/add.tests
    ;;
  neg)  # a new file only: merges with anything
    printf '%s\n' 'neg() { echo $((0 - $1)); }' >neg.sh
    printf '%s\n' 'negates [crit:neg] :: . ./neg.sh && [ "$(neg 4)" = -4 ]' >test/neg.tests
    ;;
  sleeper)  # stays busy until killed
    printf '%s\n' "$mul" >>calc.sh
    ps -o pgid= -p $$ >>"${FAKE_MARK:-/dev/null}"
    sleep 300
    ;;
  stubborn)  # like sleeper, but ignores SIGTERM: only SIGKILL stops it
    printf '%s\n' "$mul" >>calc.sh
    trap '' TERM
    ps -o pgid= -p $$ >>"${FAKE_MARK:-/dev/null}"
    sleep 300
    ;;
  idle)
    ;;
  *)
    echo "unknown role '$role'" >&2
    exit 1
    ;;
esac
aver verify
