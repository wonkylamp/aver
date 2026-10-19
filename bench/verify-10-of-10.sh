#!/bin/sh
# bench/verify-10-of-10.sh: whether Aver gives the verdict REDESIGN.md §6
# promises every time. Each scenario is a base and a head commit on a
# scratch copy of fixtures/sh-calc (shell checks: no Node, no Docker);
# `aver ci --fresh` verifies the head against the base N times (default 10),
# each in its own throwaway home, and the scenario passes only when all N
# runs give the expected verdict and exit code.
#
#   pass        a new function with its test         PASS             exit 0
#   regression  add() broken                         FAIL_OWN         exit 1
#   b1          the test command runs no test        BLOCKED_NOT_OWN  exit 1
#   b2          the runner exits 4 before any test   BLOCKED_NOT_OWN  exit 1
#   b3          the head's build rewrites the built  REVIEW           exit 3
#               test file
#   b4          add() broken, its test file deleted  FAIL_OWN         exit 1
#   b8          test ids built from the slot path    PASS             exit 0
#               and the process id; a comment edit
#   b9          a fixture key and .env.example; a    GREEN            exit 0
#               workspace, a green verify, a breaking
#               edit, `aver restore --last-green`
#               (its own driver, run_b9; times in OUT)
#   sensitive   green tests and an edited CI         REVIEW           exit 3
#               workflow
#   b14         add() broken; the only check runs a  BLOCKED_NOT_OWN  exit 1
#               tool from node_modules/.bin that is
#               installed on neither side
#   b15         add() broken; its pytest-style test  BLOCKED_NOT_OWN  exit 1
#               module fails to collect on both sides
#
#   b13         on a copy of fixtures/calc (Node; its UNPROVEN 3 UNPROVEN 3
#               check selects tests by name): divide()   PASS 0 PASS 0
#               fixed with an untested `if` added to
#               sub(), then to divide() itself; the
#               fix alone, and the second with a test
#               asserting on its `if` (its own driver,
#               run_b13: four heads on one base; each
#               UNPROVEN names the untested lines)
#
#   b10         four `aver workspace new` workspaces EXPORTED         exit 0
#               each fix a function and append a
#               test to test/add.tests; `aver
#               combine` merges them by union, all
#               four PASS with no rerun
#
# b5 and b11 drive a workspace directory instead (`run_<scenario>`: aver
# workspace new on a fresh copy, edit, verify, accept, each run in its own
# home) and expect a verdict and a list of exit codes:
#
#   b5   the agent rewrites add's tests: REVIEW; verify, accept         REVIEW 0 2 2 0
#        --approve-review as an agent (CLAUDECODE=1), accept as a
#        person, accept --approve-review as a person
#   b11  the agent commits "fix: handle negative zero" in the Git       PASS 0 0 0
#        view; git push origin, verify, accept (branch named
#        aver/fix-handle-negative-zero-<id>, or the line says otherwise)
#
# B12 is an `aver run` property gated by verdict_integrity.rs alone (docs/BENCHMARKS.md);
# B6 and B7 need Node and Cargo and are gated by crates/aver/tests/verdict_integrity.rs.
# N=3 runs fewer; SCENARIOS="b1 b4" runs some. Writes OUT
# (bench/results/verify-10-of-10.md). Needs git and a release build
# (`cargo build --release -p aver`, or AVER=<binary>).
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
AVER=${AVER:-$ROOT/target/release/aver}
N=${N:-10}
SCENARIOS=${SCENARIOS:-"pass regression b1 b2 b3 b4 b5 b8 b9 b10 b11 sensitive b13 b14 b15"}
OUT=${OUT:-$ROOT/bench/results/verify-10-of-10.md}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/aver-10-of-10.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
export AVER_HOME="$WORK/home"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$WORK/gitconfig"
printf '[user]\n\tname = bench\n\temail = bench@example.invalid\n[init]\n\tdefaultBranch = main\n' >"$GIT_CONFIG_GLOBAL"

[ -x "$AVER" ] || { echo "no aver binary at $AVER; run cargo build --release -p aver" >&2; exit 2; }

# A run.sh like the fixture's that reads its cases from $DIR, then runs
# $PRE first; the scenarios that change the harness change it on both sides.
runner() {
  cat <<EOF
#!/bin/sh
out=\${1:-unit.xml}
$1
cases=""
n=0
failed=0
for f in $2; do
  while IFS= read -r line; do
    case \$line in '' | '#'*) continue ;; esac
    name=\${line%% :: *}
    cmd=\${line#* :: }
    n=\$((n + 1))
    if (. ./calc.sh && eval "\$cmd") >/dev/null 2>&1; then
      cases="\$cases<testcase classname=\"\$f\" name=\"\$name\"/>"
    else
      failed=\$((failed + 1))
      cases="\$cases<testcase classname=\"\$f\" name=\"\$name\"><failure message=\"failed\"/></testcase>"
    fi
  done <"\$f"
done
$3
printf '<?xml version="1.0"?>\n<testsuites><testsuite name="sh-calc">%s</testsuite></testsuites>\n' "\$cases" >"\$out"
[ "\$failed" -eq 0 ]
EOF
}

# base_<scenario> and head_<scenario> edit the working tree in $1.
base_pass() { :; }
head_pass() {
  echo 'sub() { echo $(($1 - $2)); }' >>"$1/calc.sh"
  echo 'subtracts :: [ "$(sub 5 3)" = 2 ]' >"$1/test/sub.tests"
}
base_regression() { :; }
head_regression() { echo 'add() { echo $(($1 - $2)); }' >"$1/calc.sh"; }
base_b1() { printf '#!/bin/sh\necho "no test files found"\nprintf "<testsuites/>\\n" >"$1"\n' >"$1/test/run.sh"; }
head_b1() { echo '# adds' >>"$1/calc.sh"; }
base_b2() { printf '#!/bin/sh\necho "ERROR: Unknown config option: cache_dir" >&2\nexit 4\n' >"$1/test/run.sh"; }
head_b2() { echo '# adds' >>"$1/calc.sh"; }
base_b3() {
  runner 'mkdir -p dist/test; for t in test/*.tests; do cp "$t" "dist/test/$(basename "$t" .tests).test.js"; done; sh ./build.sh' 'dist/test/*.test.js' '' >"$1/test/run.sh"
  echo ':' >"$1/build.sh"
}
head_b3() {
  echo 'add() { echo 0; }' >"$1/calc.sh"
  printf 'printf "adds two numbers :: true\\nadds a negative :: true\\n" >dist/test/add.test.js\n' >"$1/build.sh"
}
base_b4() { :; }
head_b4() {
  echo 'add() { echo 0; }' >"$1/calc.sh"
  rm "$1/test/add.tests"
  echo 'still runs :: true' >"$1/test/other.tests"
}
base_b8() {
  # shellcheck disable=SC2016
  runner '' 'test/*.tests' 'cases="$cases<testcase classname=\"test/add.tests\" name=\"at[$PWD/test/data]\"/><testcase classname=\"test/add.tests\" name=\"pid[$$]\"/>"' >"$1/test/run.sh"
}
head_b8() { echo '# adds two numbers' >>"$1/calc.sh"; }
base_sensitive() { :; }
head_sensitive() {
  mkdir -p "$1/.github/workflows"
  echo 'on: pull_request_target' >"$1/.github/workflows/ci.yml"
}

# B14: no green without a run. The tool is never installed, so both sides
# end in "calc-check: command not found"; the base explains nothing.
base_b14() {
  # shellcheck disable=SC2016
  printf '[[check]]\nid  = "tool"\nrun = ["sh", "-c", "PATH=\\"$PWD/node_modules/.bin:$PATH\\" calc-check"]\n' >"$1/aver.toml"
}
head_b14() { echo 'add() { echo $(($1 - $2)); }' >"$1/calc.sh"; }

# B9 drives workspaces instead of `aver ci`. The key and token are split so
# this script holds no secret-like string itself.
base_b9() {
  mkdir -p "$1/tests/fixtures"
  printf '%s\nabc\n%s\n' "-----BEGIN RSA PRIV""ATE KEY-----" "-----END RSA PRIV""ATE KEY-----" >"$1/tests/fixtures/key.pem"
  echo "AWS_ACCESS_KEY_ID=AKIA""ABCDEFGHIJKLMNOP" >"$1/.env.example"
}
head_b9() { echo '# calc' >>"$1/calc.sh"; }
# wall_ms <stdout> <stderr> <command>...: runs the command with its output
# in those files (stderr appended) and prints its wall-clock milliseconds,
# timed inside one perl so no process start is counted; fails as it did.
wall_ms() {
  perl -MTime::HiRes=time -e 'my ($o, $e) = splice @ARGV, 0, 2;
    open my $out, ">&", \*STDOUT or die; open STDOUT, ">", $o or die; open STDERR, ">>", $e or die;
    my $t = time; my $r = system @ARGV; printf $out "%.0f\n", (time - $t) * 1000; exit($r ? 1 : 0)' "$@"
}
# The median of the numbers in file $1, one per line.
median() { sort -n "$1" | awk '{ v[NR] = $1 } END { if (NR) print (NR % 2 ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2) }'; }
# Prints GREEN 0 when each step does what B9 promises, else the step that
# did not; appends the warm creation's wall-clock time to $WORK/b9-new-ms
# and the restore's to $WORK/b9-restore-ms.
run_b9() {
  home="$WORK/home-b9-$2"
  rm -rf "$home"
  AVER_HOME=$home "$AVER" workspace new "$1" --no-deps --label b9 --json >"$3.new" 2>"$3.log" || { echo "workspace-new-refused 2"; return; }
  grep -q 'tests/fixtures/key.pem' "$3.log" || { echo "no-warning 2"; return; }
  ws=$(sed -n 's/.*"dir": *"\([^"]*\)".*/\1/p' "$3.new" | head -n 1)
  warm=$(wall_ms "$3.warm" "$3.log" env AVER_HOME="$home" "$AVER" workspace new "$1" --no-deps --label b9-warm) || { echo "warm-new-failed 2"; return; }
  echo 'sub() { echo $(($1 - $2)); }' >>"$ws/calc.sh"
  echo 'subtracts :: [ "$(sub 5 3)" = 2 ]' >"$ws/test/sub.tests"
  green=$(cat "$ws/calc.sh")
  (cd "$ws" && AVER_HOME=$home "$AVER" verify >>"$3.log" 2>&1) || { echo "green-verify-not-done 1"; return; }
  echo 'add() { echo 0; }' >>"$ws/calc.sh"
  if (cd "$ws" && AVER_HOME=$home "$AVER" check >>"$3.log" 2>&1); then echo "break-not-seen 0"; return; fi
  restore=$(cd "$ws" && wall_ms "$3.restore" "$3.log" env AVER_HOME="$home" "$AVER" restore --last-green) || { echo "restore-failed 2"; return; }
  [ "$(cat "$ws/calc.sh")" = "$green" ] || { echo "not-restored 1"; return; }
  echo "$warm" >>"$WORK/b9-new-ms"
  echo "$restore" >>"$WORK/b9-restore-ms"
  echo "GREEN 0"
}

# b10 is not a base and a head: run_b10 <repo> <i> makes the base, four
# workspaces on it in a fresh home (each fixes one function, appends its
# test and verifies), combines them and prints "<status> <exit>", status
# EXPORTED only when all four PASS with one run per side.
b10_ws() {
  ws=$(AVER_HOME="$home" "$AVER" workspace new "$repo" --no-deps --label "$1" -c "$1: $1 works" 2>/dev/null | tail -n 1)
  sed "s/^$1() { echo 0; }\$/$1() { echo \$(($2)); }/" "$ws/calc.sh" >"$ws/calc.new" && mv "$ws/calc.new" "$ws/calc.sh"
  printf '[crit:%s] %s works :: [ "$(%s)" = %s ]\n' "$1" "$1" "$3" "$4" >>"$ws/test/add.tests"
  (cd "$ws" && AVER_HOME="$home" "$AVER" verify >/dev/null 2>&1)
}
run_b10() {
  repo="$1-$2"
  rm -rf "$repo"
  cp -R "$ROOT/fixtures/sh-calc" "$repo"
  home="$WORK/home-b10-$2"
  json="$WORK/out-b10-$2.json"
  printf '\nsub() { echo 0; }\n\nmul() { echo 0; }\n\nneg() { echo 0; }\n\ndbl() { echo 0; }\n' >>"$repo/calc.sh"
  git -C "$repo" init -q
  commit "$repo" base >/dev/null
  # shellcheck disable=SC2016
  b10_ws sub '$1 - $2' 'sub 5 3' 2 && b10_ws mul '$1 * $2' 'mul 4 3' 12 &&
    b10_ws neg '0 - $1' 'neg 4' -4 && b10_ws dbl '$1 * 2' 'dbl 4' 8 ||
    { echo "a-workspace-did-not-verify 1"; return; }
  set +e
  (cd "$repo" && AVER_HOME="$home" "$AVER" combine --json sub mul neg dbl) >"$json" 2>"$json.log"
  code=$?
  set -e
  status=$(grep -o '"status": *"[a-z_]*"' "$json" | head -n 1 | sed 's/.*"\([a-z_]*\)"$/\1/')
  passes=$(grep -c '"verdict": *"PASS"' "$json" || true)
  reruns=0
  for r in $(grep -o '"receipt": *"rcpt_[a-z0-9]*"' "$json" | sed 's/.*"\(rcpt_[a-z0-9]*\)"$/\1/'); do
    if AVER_HOME="$home" "$AVER" receipt "$r" | tr -d ' \n' | grep -q '\["run_[a-z0-9]*","run_'; then
      reruns=$((reruns + 1))
    fi
  done
  if [ "$status" = exported ] && [ "$passes" -eq 4 ] && [ "$reruns" -eq 0 ]; then
    echo "EXPORTED $code"
  else
    echo "${status:-none}:${passes}-pass:${reruns}-rerun $code"
  fi
}

# B15: a test module that does not import gives no evidence. The runner
# reports add's module as pytest's collection error on both sides, so the
# broken add() never runs; its failure on the base explains nothing.
base_b15() {
  cat >"$1/test/run.sh" <<'RUN'
#!/bin/sh
printf '<?xml version="1.0"?>\n<testsuites><testsuite name="pytest"><testcase classname="" name="test.test_add" file="test/test_add.py"><error message="collection failure">ModuleNotFoundError: No module named calc_version</error></testcase><testcase classname="test.test_other" name="test_true" file="test/test_other.py"/></testsuite></testsuites>\n' >"${1:-unit.xml}"
exit 1
RUN
  sed 's/format = "junit"/format = "pytest-junit"/' "$1/aver.toml" >"$1/aver.toml.new"
  mv "$1/aver.toml.new" "$1/aver.toml"
}
head_b15() { echo 'add() { echo $(($1 - $2)); }' >"$1/calc.sh"; }

# B13: hunk necessity. Four heads on one fixtures/calc base, each
# verified with `aver ci --fresh` against the div criteria; prints
# "<verdict>[:<lines>] <exit>" per head, <lines> what summary.md names as
# untested.
b13_math() {
  # $1: the line sub() gains, or nothing; $2: divide()'s body.
  cat <<EOF
function add(a, b) {
  return a + b;
}

function sub(a, b) {
$1  return a - b;
}

function mul(a, b) {
  return a * b;
}

function divide(a, b) {
$2}

module.exports = { add, sub, mul, divide };
EOF
}
run_b13() {
  repo="$1-calc-$2"
  rm -rf "$repo"
  cp -R "$ROOT/fixtures/calc" "$repo"
  git -C "$repo" init -q
  base=$(commit "$repo" base)
  printf -- '- div-basic: divide returns the quotient\n- div-zero: divide throws on division by zero\n' >"$repo.criteria"
  zero="  if (b === 0) throw new Error('division by zero');
"
  backdoor="  if (a === 1337) return 0;
"
  quotient="  return a / b;
"
  got=""
  for head in sub divide fixed covered; do
    git -C "$repo" checkout -q "$base"
    case $head in
      sub) b13_math "$backdoor" "$zero$quotient" ;;
      divide) b13_math "" "$zero$backdoor$quotient" ;;
      fixed) b13_math "" "$zero$quotient" ;;
      covered)
        b13_math "" "$zero$backdoor$quotient"
        printf "const test = require('node:test');\nconst assert = require('node:assert/strict');\nconst { divide } = require('../src/math');\n\ntest('[crit:div-basic] divide of 1337 is 0', () => {\n  assert.equal(divide(1337, 5), 0);\n});\n" >"$repo/test/answer.test.js"
        ;;
    esac >"$repo/src/math.js"
    sha=$(commit "$repo" "$head")
    out="$3-$head"
    set +e
    "$AVER" ci --repo "$repo" --base "$base" --head "$sha" --criteria "$repo.criteria" --out-dir "$out" --fresh >"$out.log" 2>&1
    code=$?
    set -e
    verdict=$(grep -o '"verdict": *"[A-Z_]*"' "$out/receipt.json" 2>/dev/null | head -n 1 | sed 's/.*"\([A-Z_]*\)"$/\1/')
    lines=$(sed -n 's/.*Reverting your change at `\([^`]*\)`.*/\1/p' "$out/summary.md" 2>/dev/null | tr '\n' ',' | sed 's/,$//')
    got="$got ${verdict:-none}${lines:+:$lines} $code"
  done
  echo "${got# }"
}

expected() {
  case $1 in
    pass | b8) echo "PASS 0" ;;
    regression | b4) echo "FAIL_OWN 1" ;;
    b1 | b2 | b14 | b15) echo "BLOCKED_NOT_OWN 1" ;;
    b3 | sensitive) echo "REVIEW 3" ;;
    b5) echo "REVIEW 0 2 2 0" ;;
    b9) echo "GREEN 0" ;;
    b10) echo "EXPORTED 0" ;;
    b11) echo "PASS 0 0 0" ;;
    b13) echo "UNPROVEN:src/math.js:6-6 3 UNPROVEN:src/math.js:15-15 3 PASS 0 PASS 0" ;;
  esac
}

# Scenarios with a driver of their own: `run_<scenario> <repo> <out>`
# prints what it saw in the form expected() gives.

# aver as a person: none of the agent markers accept --approve-review
# refuses, and AVER_TEST_HUMAN=1 stands in for the terminal it asks for.
human() { env -u AVER_RUN -u AVER_WORKSPACE -u CLAUDECODE -u CODEX_SANDBOX AVER_TEST_HUMAN=1 "$AVER" "$@"; }

# A workspace directory of a fresh copy of $1 (in $2), on the neg
# criterion, with neg and its test written; prints the directory.
solved_dir() {
  cp -R "$1" "$2/repo"
  dir=$(cd "$2/repo" && human workspace new -c "neg: neg(x) returns -x" 2>"$2/new.log" | tail -n 1)
  echo 'neg() { echo $((0 - $1)); }' >>"$dir/calc.sh"
  echo '[crit:neg] negates :: [ "$(neg 2)" = -2 ]' >"$dir/test/neg.tests"
  echo "$dir"
}

# The verdict a workspace report's first line names.
verdict_of() { sed -En '1s/^(DONE|NOT DONE|NEEDS HUMAN REVIEW): ([A-Z_]+).*/\2/p' "$1"; }

run_b5() (
  mkdir -p "$3" && export AVER_HOME="$3/home"
  dir=$(solved_dir "$1" "$3")
  echo 'add() { echo "$(($1 + $2)).0"; }' >>"$dir/calc.sh"
  printf 'adds two numbers :: [ "$(add 2 3)" = 5.0 ]\nadds a negative :: [ "$(add 2 -3)" = -1.0 ]\n' >"$dir/test/add.tests"
  cd "$dir"
  set +e
  human verify >"$3/verify.log" 2>"$3/verify.err"; v=$?
  CLAUDECODE=1 "$AVER" accept --approve-review >"$3/agent.log" 2>&1; a=$?
  human accept >"$3/human.log" 2>&1; h=$?
  human accept --approve-review >"$3/approved.log" 2>&1; ok=$?
  echo "$(verdict_of "$3/verify.log") $v $a $h $ok"
)

run_b11() (
  mkdir -p "$3" && export AVER_HOME="$3/home"
  dir=$(solved_dir "$1" "$3")
  cd "$dir"
  set +e
  git add -A && git commit -qm 'fix: handle negative zero'
  git push -q origin "$(git branch --show-current)" >"$3/push.log" 2>&1; p=$?
  human verify >"$3/verify.log" 2>"$3/verify.err"; v=$?
  human accept >"$3/accept.log" 2>&1; a=$?
  branch=$(sed -n 's/.* as branch \([^ ]*\) .*/\1/p' "$3/accept.log" | head -n 1)
  subject=$(git -C "$3/repo" log -1 --format=%s "$branch" 2>/dev/null)
  case "$branch|$subject" in
    aver/fix-handle-negative-zero-????"|fix: handle negative zero") named="" ;;
    *) named=" named:$branch:$subject" ;;
  esac
  echo "$(verdict_of "$3/verify.log") $p $v $a$named"
)

commit() { git -C "$1" add -A && git -C "$1" commit -qm "$2" && git -C "$1" rev-parse HEAD; }

rows=""
failed=0
for s in $SCENARIOS; do
  repo="$WORK/$s"
  cp -R "$ROOT/fixtures/sh-calc" "$repo"
  git -C "$repo" init -q
  if ! command -v "base_$s" >/dev/null 2>&1; then
    commit "$repo" base >/dev/null
  else
    "base_$s" "$repo"
    base=$(commit "$repo" base)
    "head_$s" "$repo"
    head=$(commit "$repo" head)
  fi
  want=$(expected "$s")
  seen=""
  ok=0
  i=1
  while [ "$i" -le "$N" ]; do
    out="$WORK/out-$s-$i"
    if command -v "run_$s" >/dev/null 2>&1; then
      got=$("run_$s" "$repo" "$i" "$out")
    else
      set +e
      "$AVER" ci --repo "$repo" --base "$base" --head "$head" --out-dir "$out" --fresh >"$out.log" 2>&1
      code=$?
      set -e
      verdict=$(grep -o '"verdict": *"[A-Z_]*"' "$out/receipt.json" 2>/dev/null | head -n 1 | sed 's/.*"\([A-Z_]*\)"$/\1/')
      got="${verdict:-none} $code"
    fi
    [ "$got" = "$want" ] && ok=$((ok + 1))
    case " $seen " in *" $got;"*) ;; *) seen="$seen $got;" ;; esac
    i=$((i + 1))
  done
  [ "$ok" -eq "$N" ] || failed=$((failed + 1))
  echo "$s: $ok of $N gave $want; seen:$seen" >&2
  rows="$rows| $s | $want | $ok of $N |$(echo "$seen" | sed 's/;$//') |
"
done

{
  echo "# Verify N of N"
  echo
  echo "\`bench/verify-10-of-10.sh\` on $(uname -sm), $(date -u +%Y-%m-%d): each scenario verified $N times with \`aver ci --fresh\` (b5 and b11: a workspace directory, verified and accepted), each in a throwaway home."
  echo
  echo "| Scenario | Expected (verdict, exit) | Runs as expected | Seen |"
  echo "|---|---|---|---|"
  printf '%s' "$rows"
  if [ -s "$WORK/b9-new-ms" ]; then
    echo
    echo "B9 times over $(wc -l <"$WORK/b9-new-ms" | tr -d ' ') runs (debug or release as built; targets: warm creation under 100 ms, restore under 50 ms):"
    echo
    echo "- warm \`aver workspace new\` (one directory, the repository already imported at HEAD), wall clock: median $(median "$WORK/b9-new-ms") ms, runs $(tr '\n' ' ' <"$WORK/b9-new-ms")"
    echo "- \`aver restore --last-green\`, wall clock: median $(median "$WORK/b9-restore-ms") ms, runs $(tr '\n' ' ' <"$WORK/b9-restore-ms")"
  fi
} >"$OUT"
cat "$OUT"
[ "$failed" -eq 0 ]
