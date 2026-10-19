#!/bin/sh
# End-to-end tests of the team flows of `aver run` with scripted fake agents
# (tests/e2e/batch/agent.sh) instead of models: several agents on one task,
# batches with `after:`, retries and budgets, `aver combine`, leases and
# notices between tasks, `aver rerun --onto`, and Ctrl-C during a run.
#
# Runs the `aver` on PATH (or $AVER) against fixtures/sh-calc in a throwaway
# home and repository; needs only sh and git. No network, no `gh`.
#   tests/e2e/run-batch.sh            every scenario
#   tests/e2e/run-batch.sh 3 5        scenarios 3 and 5
#   KEEP=1 tests/e2e/run-batch.sh     keep the work directory
set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
AVER=${AVER:-aver}
command -v "$AVER" >/dev/null || { echo "no aver on PATH (set AVER)" >&2; exit 2; }
AGENT="sh $here/batch/agent.sh"
work=$(mktemp -d "${TMPDIR:-/tmp}/aver-e2e-batch.XXXXXX")
work=$(cd "$work" && pwd -P)
[ -n "${KEEP:-}" ] || trap 'rm -rf "$work"' EXIT
fails=0
passes=0

say() { printf '\n== %s\n' "$*"; }
ok() { passes=$((passes + 1)); printf '  ok   %s\n' "$*"; }
bad() { fails=$((fails + 1)); printf '  FAIL %s\n' "$*"; }

# run <name> <cmd…>: runs the command with output in $work/<name>.out and its
# exit code in $rc.
run() {
  name=$1
  shift
  out="$work/$name.out"
  "$@" >"$out" 2>&1
  rc=$?
}

# exits <code>: the last run exited with <code>.
exits() {
  if [ "$rc" -eq "$1" ]; then ok "$name exits $1"; else
    bad "$name exits $rc, not $1"
    sed 's/^/       | /' "$out" | tail -25
  fi
}

# has <regex> [what]: the last run's output matches.
has() {
  if grep -Eq -- "$1" "$out"; then ok "$name: ${2:-$1}"; else
    bad "$name: no line matching /$1/"
    sed 's/^/       | /' "$out" | tail -25
  fi
}

# lacks <regex> [what]: the last run's output does not match.
lacks() {
  if grep -Eq -- "$1" "$out"; then
    bad "$name: has /$1/: $(grep -E -- "$1" "$out" | head -3)"
  else ok "$name: ${2:-no /$1/}"; fi
}

# note <regex> <what>: reports, without failing, an output line worth a look.
notes=0
note() {
  if grep -Eq -- "$1" "$out"; then
    notes=$((notes + 1))
    printf '  NOTE %s: %s\n' "$name" "$2"
    grep -E -- "$1" "$out" | head -2 | sed 's/^/       | /'
  fi
}

# fresh <scenario>: a home, a state directory and a copy of sh-calc as a
# Git repository; cds into the repository.
fresh() {
  dir="$work/$1"
  mkdir -p "$dir/state"
  export AVER_HOME="$dir/home" FAKE_STATE="$dir/state"
  unset FAKE_ROLES
  repo="$dir/repo"
  mkdir -p "$repo"
  cp "$root/fixtures/sh-calc/calc.sh" "$root/fixtures/sh-calc/aver.toml" "$repo/"
  cp -R "$root/fixtures/sh-calc/test" "$repo/test"
  cd "$repo" || exit 2
  git init -q -b main
  git config user.name "Repo Owner"
  git config user.email owner@example.com
  git add -A
  git commit -qm base
  base=$(git rev-parse HEAD)
}

# The agent flags every `aver run` here shares.
RUNFLAGS="--no-tui --no-deps --timeout 120s"

# --- 1: three agents on one task --------------------------------------------
s1() {
  say "1: three agents on one task: honest, gaming, breaking"
  fresh s1
  export FAKE_ROLES="breaker honest gamer"
  # shellcheck disable=SC2086
  run s1-run "$AVER" run "Add mul" -c "mul: mul multiplies two numbers" --agents 3 \
    --agent-cmd "$AGENT" $RUNFLAGS
  exits 0
  has '^  2  custom  PASS ' "the honest agent (#2) ranks first with PASS"
  has '^  3  custom  REVIEW ' "the gamer (#3) is REVIEW, second"
  has '^  1  custom  FAIL_OWN ' "the breaker (#1) is FAIL_OWN, last"
  # Row order: PASS, then REVIEW, then FAIL_OWN.
  order=$(sed -n 's/^  \([0-9]\)  custom  .*/\1/p' "$out" | tr -d '\n')
  [ "$order" = 231 ] && ok "s1-run: ranked 2, 3, 1" || bad "s1-run: ranked $order, not 231"
  has 'aver accept 2' "the hint names the winner"
  unset FAKE_ROLES

  run s1-status "$AVER" status
  exits 0
  has 'run [A-Z0-9]+ · finished' "status: the run finished"
  has '^  2  custom  PASS ' "status: the ranking"

  run s1-diff "$AVER" diff 1 2
  exits 0
  has '^--- 1/calc.sh' "diff 1 2 compares the two attempts"
  has '^\+add\(\) \{ echo \$\(\(\$1 \+ \$2\)\); \}' "diff 1 2 shows #1's broken add against #2's"

  run s1-accept-review "$AVER" accept 3 </dev/null
  [ "$rc" -ne 0 ] && ok "s1-accept-review: accepting the REVIEW attempt is refused (exit $rc)" ||
    bad "s1-accept-review: accept 3 (REVIEW) exited 0"
  has 'approve-review' "the refusal names --approve-review"

  run s1-accept "$AVER" accept 2
  exits 0
  has 'Accepted attempt 2 of run [A-Z0-9]+ as branch aver/' "accept 2 exports a branch"
  branch=$(sed -n 's/.* as branch \(aver\/[^ ]*\) .*/\1/p' "$out")
  if [ -n "$branch" ] && [ "$(git rev-parse "$branch^")" = "$base" ]; then
    ok "s1-accept: $branch's parent is the base"
  else bad "s1-accept: $branch's parent is $(git rev-parse "$branch^" 2>&1), not $base"; fi
  git show "$branch:calc.sh" | grep -q '^mul()' && ok "s1-accept: the branch has mul" ||
    bad "s1-accept: the branch lacks mul"
  git show "$branch:test/add.tests" | grep -q 'adds a negative :: \[' &&
    ok "s1-accept: the branch keeps the real negative test" || bad "s1-accept: the negative test changed"


  run s1-status2 "$AVER" status
  exits 0
  has 'accepted' "status after accept says #2 was accepted"

  # One agent that changes nothing: not done, exit 1.
  export FAKE_ROLES=idle
  # shellcheck disable=SC2086
  run s1-idle "$AVER" run "Add mul" --agents 1 --agent-cmd "$AGENT" $RUNFLAGS
  exits 1
  has 'no change' "an unchanged directory ranks as no change"
  lacks 'done: PASS' "the live line does not call an unchanged directory done"
  lacks 'aver accept 1' "and offers no accept"
  unset FAKE_ROLES
}

# --- 2: a batch with a task after two others ---------------------------------
s2() {
  say "2: a batch: #3 after #1 and #2"
  fresh s2
  cat >"$dir/tasks.txt" <<'EOF'
Add mul role=honest
  - mul: mul multiplies two numbers
Add sub role=sub
  - sub: sub subtracts
Add square role=square
  after: #1, #2
  - sq: sq squares a number
EOF
  # shellcheck disable=SC2086
  run s2-batch "$AVER" run --tasks "$dir/tasks.txt" --agent-cmd "$AGENT" --parallel 2 $RUNFLAGS
  exits 0
  has '^3 tasks · 3 ready' "every task ready"
  has '#3 started as run [A-Z0-9]+ on aver/combine-' "#3 starts on the combine of #1 and #2"
  has '^#3 .* PASS ' "#3 passes (it saw mul and sub)"
  combine=$(sed -n 's/.*#3 started as run [A-Z0-9]* on \(aver\/combine-[^ ]*\).*/\1/p' "$out")
  if [ -n "$combine" ] && git show "$combine:calc.sh" | grep -q '^mul()' &&
    git show "$combine:calc.sh" | grep -q '^sub()'; then
    ok "s2-batch: $combine holds mul and sub"
  else bad "s2-batch: combine branch '$combine' lacks mul or sub"; fi

  run s2-accept3 "$AVER" accept 3
  exits 0
  b3=$(sed -n 's/.* as branch \(aver\/[^ ]*\) .*/\1/p' "$out")
  [ -n "$b3" ] && [ "$(git rev-parse "$b3^")" = "$(git rev-parse "$combine")" ] &&
    ok "s2-accept3: #3's branch is stacked on the combine" ||
    bad "s2-accept3: $b3's parent is not $combine"

  say "2b: a failing prerequisite blocks; --retry"
  fresh s2b
  cat >"$dir/tasks.txt" <<'EOF'
Add mul role=breaker
  - mul: mul multiplies two numbers
Add sub role=sub
  - sub: sub subtracts
Add square role=square
  after: #1, #2
  - sq: sq squares a number
Add mul again role=second-try
  - mul: mul multiplies two numbers
EOF
  # shellcheck disable=SC2086
  run s2b-batch "$AVER" run --tasks "$dir/tasks.txt" --agent-cmd "$AGENT" --parallel 2 --retry 1 $RUNFLAGS
  exits 1
  has '^4 tasks · 2 ready · 1 broke something · 1 blocked by a prerequisite' "the counts line"
  has '#3 not started: #1 did not pass: FAIL_OWN' "#3 is blocked by #1"
  has '^#3 .* blocked ' "the summary marks #3 blocked"
  has '#4 retrying \(1 of 1\) as run [A-Z0-9]+' "#4 is retried"
  has '^#4 .* PASS ' "#4 passes on its retry"
  lacks 'square: started without' "#3 never started"
  note 'overlap: #1 and' "overlap lines name #1, which broke something and is not ready"
  printf '%s\n' "$out" >"$dir/batch-out-path"

  say "2c: --batch-verifies"
  fresh s2c
  cat >"$dir/tasks.txt" <<'EOF'
Add mul role=honest
  - mul: mul multiplies two numbers
Add sub role=sub
  - sub: sub subtracts
EOF
  # shellcheck disable=SC2086
  run s2c-batch "$AVER" run --tasks "$dir/tasks.txt" --agent-cmd "$AGENT" --parallel 1 --batch-verifies 1 $RUNFLAGS
  exits 1
  has '^#1 .* PASS ' "#1 ran and passed"
  has '^#2 .* budget +the batch used 1 of 1 verifies' "#2 never started: over budget"
  has '1/1 verifies' "the summary shows the verifies spent"
  note "in its kept directory's place" "the budget hint speaks of a kept directory, but #2 never started"
}

# --- 3: combine with two tasks on the same lines -----------------------------
s3() {
  say "3: combine --all-pass and --partial, two tasks on one line"
  fresh s3
  cat >"$dir/tasks.txt" <<'EOF'
Add mul role=mul-inline
  - mul: mul multiplies two numbers
Add sub role=sub-inline
  - sub: sub subtracts
Add neg role=neg
  - neg: neg negates
EOF
  # shellcheck disable=SC2086
  run s3-batch "$AVER" run --tasks "$dir/tasks.txt" --agent-cmd "$AGENT" --parallel 3 $RUNFLAGS
  exits 0
  has '^3 tasks · 3 ready' "three ready"
  has 'overlap: #1 and #2 both change calc.sh \(overlapping lines\)' "#1 and #2 overlap on calc.sh"

  run s3-all "$AVER" combine --all-pass
  [ "$rc" -ne 0 ] && ok "s3-all: combine --all-pass fails on the conflict (exit $rc)" ||
    bad "s3-all: combine --all-pass exited 0 on a conflict"
  has 'calc.sh' "it names the conflicting file"
  has -- '--partial' "it suggests --partial"
  git branch --list 'aver/combine-*' | grep -q . && bad "s3-all: a combine branch was written" ||
    ok "s3-all: no branch written"

  run s3-partial "$AVER" combine --all-pass --partial
  exits 1 # it left #2 out (docs/CLI.md)
  cb=$(git branch --list 'aver/combine-*' | sed 's/^[* ]*//' | head -1)
  if [ -n "$cb" ]; then
    ok "s3-partial: wrote $cb"
    git show "$cb:calc.sh" | grep -q 'mul()' && ok "s3-partial: keeps #1 (mul)" || bad "s3-partial: lacks #1"
    git show "$cb:neg.sh" >/dev/null 2>&1 && ok "s3-partial: keeps #3 (neg)" || bad "s3-partial: lacks #3"
    git show "$cb:calc.sh" | grep -q 'sub()' && bad "s3-partial: kept #2 (conflicts with #1)" ||
      ok "s3-partial: leaves out #2"
    [ "$(git rev-parse "$cb^")" = "$base" ] && ok "s3-partial: parent is the base" ||
      bad "s3-partial: parent is not the base"
  else bad "s3-partial: no combine branch"; fi
  has '#2' "the output names #2 as left out"
  has 'rerun 2 --onto' "it suggests rebuilding #2 on the combine"
}

# --- 4: leases and notices ---------------------------------------------------
s4() {
  say "4: leases and overlap notices between two tasks"
  fresh s4
  run s4-wsA "$AVER" workspace new . -c "mul: mul multiplies"
  exits 0
  wsA=$(grep -E '^/' "$out" | tail -1)
  run s4-wsB "$AVER" workspace new . -c "sub: sub subtracts"
  exits 0
  wsB=$(grep -E '^/' "$out" | tail -1)
  [ -d "$wsA" ] && [ -d "$wsB" ] && [ "$wsA" != "$wsB" ] && ok "two workspace directories" ||
    { bad "workspace paths '$wsA' '$wsB'"; return; }

  cd "$wsA" || return
  run s4-leaseA "$AVER" lease calc.sh --reason "rewriting add"
  exits 0
  printf '%s\n' 'mul() { echo $(($1 * $2)); }' >>calc.sh
  printf '%s\n' 'multiplies [crit:mul] :: [ "$(mul 3 4)" = 12 ]' >>test/add.tests
  run s4-verifyA "$AVER" verify
  exits 0
  has 'DONE: PASS' "A verifies PASS"

  cd "$wsB" || return
  run s4-leasesB "$AVER" lease
  exits 0
  has 'calc.sh' "B sees A's lease on calc.sh"
  has 'rewriting add' "with A's reason"
  printf '%s\n' 'sub() { echo $(($1 - $2)); }' >>calc.sh
  printf '%s\n' 'subtracts [crit:sub] :: [ "$(sub 5 3)" = 2 ]' >>test/add.tests
  run s4-noticesB "$AVER" notices
  exits 0
  has 'calc.sh' "B's notices name calc.sh"
  run s4-verifyB "$AVER" verify
  [ "$rc" -ne 0 ] && ok "s4-verifyB: B's edit under A's lease is not done (exit $rc)" ||
    bad "s4-verifyB: B edited a leased file and verified done"
  has 'lease' "B's report names the lease"

  cd "$repo" || return
  run s4-repo "$AVER" notices --repo
  exits 0
  has 'calc.sh' "the repo view names calc.sh"
  has 'rewriting add' "and the lease"
  run s4-repo-json "$AVER" notices --repo --json
  exits 0
  run s4-leases "$AVER" lease --repo
  exits 0
  has 'calc.sh' "lease --repo lists A's lease"
}

# --- 5: rerun --onto ---------------------------------------------------------
s5() {
  say "5: aver rerun <n> --onto <m>"
  fresh s5
  cat >"$dir/tasks.txt" <<'EOF'
Add mul role=breaker
  - mul: mul multiplies two numbers
Add sub role=sub
  - sub: sub subtracts
Add square role=square
  after: #1, #2
  - sq: sq squares a number
EOF
  # shellcheck disable=SC2086
  run s5-batch "$AVER" run --tasks "$dir/tasks.txt" --agent-cmd "$AGENT" --parallel 2 $RUNFLAGS
  exits 1
  has '#3 not started' "#3 blocked"
  # `--onto 1` alone would start #3 without #2's work.
  has 'then `aver combine 1 2`, then `aver rerun 3 --onto <the combine.s branch>`' \
    "the hint combines both prerequisites before rerunning #3"

  export FAKE_ROLES=honest
  run s5-rerun1 "$AVER" rerun 1 --agent-cmd "$AGENT" --no-tui --no-deps
  unset FAKE_ROLES
  exits 0
  has 'PASS' "#1 rerun passes"

  run s5-combine "$AVER" combine 1 2
  exits 0
  cb=$(git branch --list 'aver/combine-*' | sed 's/^[* ]*//' | head -1)
  [ -n "$cb" ] && ok "s5-combine: wrote $cb" || bad "s5-combine: no combine branch"

  run s5-rerun3 "$AVER" rerun 3 --onto "$cb" --agent-cmd "$AGENT" --no-tui --no-deps
  exits 0
  lacks 'square: started without' "#3 starts with mul and sub"
  has 'PASS' "#3 passes on the combine"

  run s5-status "$AVER" status
  exits 0
}

# --- 6: Ctrl-C during a run --------------------------------------------------
# start_sleepers <name>: `aver run` with two agents that sleep, in the
# background in its own process group ($pid), once both agents started.
start_sleepers() {
  export FAKE_ROLES="${2:-sleeper} ${2:-sleeper}" FAKE_MARK="$dir/state/started-$1"
  set -m
  # shellcheck disable=SC2086
  "$AVER" run "Add mul" -c "mul: mul multiplies two numbers" --agents 2 --agent-cmd "$AGENT" \
    $RUNFLAGS >"$work/$1.out" 2>&1 &
  pid=$!
  set +m
  i=0
  # Each agent appends its process group to the mark.
  while [ "$(cat "$FAKE_MARK" 2>/dev/null | wc -l)" -lt 2 ] && [ $i -lt 100 ]; do
    sleep 0.1
    i=$((i + 1))
  done
  groups=$(cat "$FAKE_MARK" 2>/dev/null)
  sleep 1
  unset FAKE_ROLES FAKE_MARK
  name=$1
  out="$work/$1.out"
}

# stopped <name>: waits for $pid; its exit code in $rc, and checks no agent
# is left.
stopped() {
  t0=$(date +%s)
  wait "$pid"
  rc=$?
  [ $(($(date +%s) - t0)) -lt 20 ] && ok "$name: stopped within 20 s" ||
    bad "$name: took $(($(date +%s) - t0)) s to stop"
  sleep 1
  left=""
  for g in $groups; do
    if kill -0 -- "-$g" 2>/dev/null; then
      left="$left $g"
      kill -KILL -- "-$g"
    fi
  done
  [ -z "$groups" ] && bad "$name: the agents never recorded their groups"
  [ -z "$left" ] && ok "$name: no agent process left" || bad "$name: agent groups$left survived"
}

s6() {
  say "6: SIGINT during a run, then status and a rerun"
  fresh s6
  # The first Ctrl-C stops the agents and ends the run cleanly (exit 1:
  # nothing passed); only aver gets it, so the agents are stopped by aver.
  start_sleepers s6-run
  kill -INT "$pid"
  stopped
  exits 1
  has 'stopped|killed|interrupted' "the agents are reported stopped"
  lacks 'aver accept [0-9]' "no killed attempt is offered for accepting"
  run_id=$(sed -n 's/^aver: run \([A-Z0-9]*\) .*/\1/p' "$out")

  run s6-status "$AVER" status
  exits 0
  has "$run_id" "status names the interrupted run"
  lacks ' running|working' "status does not call it running"
  note "$run_id · finished" "status calls the interrupted run finished"

  export FAKE_ROLES="honest honest"
  run s6-rerun "$AVER" rerun "$run_id" --agent-cmd "$AGENT" --no-tui --no-deps
  unset FAKE_ROLES
  exits 0
  has 'PASS' "the rerun passes"
  run s6-status2 "$AVER" status
  exits 0
  has 'PASS' "status shows the rerun's PASS"

  # A second Ctrl-C exits 130 at once (docs/CLI.md).
  # Agents that ignore SIGTERM keep the first stop waiting for its SIGKILL.
  start_sleepers s6-twice stubborn
  kill -INT "$pid"
  sleep 1
  kill -INT "$pid"
  stopped
  exits 130
  run s6-doctor "$AVER" doctor
  has '' "doctor runs after a double Ctrl-C"
}

all="1 2 3 4 5 6"
for s in ${*:-$all}; do
  "s$s"
  cd "$root" || exit 2
done
printf '\n%s passed, %s failed, %s notes (outputs in %s)\n' "$passes" "$fails" "$notes" "$work"
[ "$fails" -eq 0 ]
