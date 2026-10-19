#!/bin/sh
# make bench-native: how long `aver verify` waits natively (ARCHITECTURE.md
# §2.4.6) against the tests themselves, on these cases:
#
#   calc  fixtures/calc, a one-line source edit: verify wall time (target:
#         under 2 s) next to a plain `node --test`.
#   aver  this repository, a one-line edit in aver-core: verify wall time
#         against a warm `cargo test` of the rust check's command on the
#         same edit (target: ratio at most 1.05, plus 1 s).
#   adhoc `aver check -- <cmd>` (§2.7.1) on calc (`node --test`), and with
#         adhoc_aver on aver-core (`cargo test -p aver-core --lib`), next to
#         the command itself on the same edit (target: at most 1.2 times it,
#         plus 1 s).
#
# Each case runs on a first edit (cold pools, measured but not the target),
# then on a second one-line edit (measured). Writes bench/native/results.md.
# CASES="calc" runs one case (default "calc aver"). Needs node, cargo and
# git; no Docker.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
AVER=${AVER:-$ROOT/target/release/aver}
CASES=${CASES:-"calc aver"}
OUT=${OUT:-$ROOT/bench/native/results.md}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/aver-bench-native.XXXXXX")
export AVER_HOME="$WORK/home" AVER_NO_DAEMON=1
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$WORK/gitconfig"
printf '[user]\n\tname = bench\n\temail = bench@example.invalid\n[init]\n\tdefaultBranch = main\n' >"$GIT_CONFIG_GLOBAL"

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
since() { perl -e "printf '%.2f', $(now) - $1"; }
RUST_CMD="cargo test --workspace --lib --bins --tests --no-fail-fast --locked"

"$AVER" init >/dev/null
rows=""
notes=""

# verify_in <dir> <label>: one `aver verify`, its wall time in $took and its
# last line in $last.
verify_in() {
  t0=$(now)
  (cd "$1" && "$AVER" verify -q >"$WORK/$2.out" 2>&1) || true
  took=$(since "$t0")
  last=$(tail -n 1 "$WORK/$2.out")
  where=$(grep '^Took' "$WORK/$2.out" | tail -n 1 || true)
  notes="$notes
- $2: $where"
}

if echo "$CASES" | grep -qw calc; then
  repo="$WORK/calc"
  cp -R "$ROOT/fixtures/calc" "$repo"
  git -C "$repo" init -q && git -C "$repo" add -A && git -C "$repo" commit -qm calc
  dir=$(cd "$repo" && "$AVER" workspace new --no-deps | tail -n 1)
  printf '// bench edit 1\n' >>"$dir/src/math.js"
  verify_in "$dir" calc1
  cold=$took
  printf '// bench edit 2\n' >>"$dir/src/math.js"
  verify_in "$dir" calc2
  warm=$took
  t0=$(now)
  (cd "$repo" && node --test >/dev/null 2>&1) || true
  plain=$(since "$t0")
  rows="$rows| calc | $cold s | $warm s | $plain s (\`node --test\`) | $(perl -e "printf '%.2f', $warm / $plain") | \`$last\` |
"
fi

if echo "$CASES" | grep -qw aver; then
  repo="$WORK/strata"
  git clone -q "$ROOT" "$repo"
  dir=$(cd "$repo" && "$AVER" workspace new --no-deps | tail -n 1)
  file=crates/aver-core/src/lib.rs
  printf '// bench edit 1\n' >>"$dir/$file"
  verify_in "$dir" aver1
  cold=$took
  printf '// bench edit 2\n' >>"$dir/$file"
  verify_in "$dir" aver2
  warm=$took
  # The baseline: the rust check's own command in a warm checkout, on the
  # same two edits.
  printf '// bench edit 1\n' >>"$repo/$file"
  (cd "$repo" && $RUST_CMD >/dev/null 2>&1) || true
  printf '// bench edit 2\n' >>"$repo/$file"
  t0=$(now)
  (cd "$repo" && $RUST_CMD >/dev/null 2>&1) || true
  plain=$(since "$t0")
  rows="$rows| aver (one-line edit in aver-core) | $cold s | $warm s | $plain s (warm \`cargo test\`) | $(perl -e "printf '%.2f', $warm / $plain") | \`$last\` |
"
fi

# check_in <dir> <label> <cmd…>: one `aver check -- <cmd…>`, its wall time
# in $took and its last line in $last.
check_in() {
  d=$1 label=$2
  shift 2
  t0=$(now)
  (cd "$d" && "$AVER" check -q -- "$@" >"$WORK/$label.out" 2>&1) || true
  took=$(since "$t0")
  last=$(tail -n 1 "$WORK/$label.out")
}

# adhoc: `aver check -- <the agent's own test command>` (§2.7.1) after a
# one-line edit, against the same command run directly in a warm checkout
# on the same edit (target: at most 1.2 times it, plus 1 s).
if echo " $CASES " | grep -q ' adhoc '; then
  repo="$WORK/adhoc-calc"
  cp -R "$ROOT/fixtures/calc" "$repo"
  git -C "$repo" init -q && git -C "$repo" add -A && git -C "$repo" commit -qm calc
  dir=$(cd "$repo" && "$AVER" workspace new --no-deps | tail -n 1)
  set -- node --test test/divide.test.js
  printf '// bench edit 1\n' >>"$dir/src/math.js"
  check_in "$dir" adhoc-calc1 "$@"
  cold=$took
  printf '// bench edit 2\n' >>"$dir/src/math.js"
  check_in "$dir" adhoc-calc2 "$@"
  warm=$took
  printf '// bench edit 2\n' >>"$repo/src/math.js"
  t0=$(now)
  (cd "$repo" && "$@" >/dev/null 2>&1) || true
  plain=$(since "$t0")
  rows="$rows| adhoc: \`aver check -- node --test test/divide.test.js\` (calc) | $cold s | $warm s | $plain s | $(perl -e "printf '%.2f', $warm / $plain") (target $(perl -e "printf '%.2f', (1.2 * $plain + 1) / $plain")) | \`$last\` |
"
fi

# adhoc_aver (its own case: two Rust build pools, about 9 GB each).
if echo " $CASES " | grep -q ' adhoc_aver '; then
  repo="$WORK/adhoc-strata"
  git clone -q "$ROOT" "$repo"
  dir=$(cd "$repo" && "$AVER" workspace new --no-deps | tail -n 1)
  file=crates/aver-core/src/lib.rs
  set -- cargo test -p aver-core --lib
  printf '// bench edit 1\n' >>"$dir/$file"
  check_in "$dir" adhoc-aver1 "$@"
  cold=$took
  printf '// bench edit 2\n' >>"$dir/$file"
  check_in "$dir" adhoc-aver2 "$@"
  warm=$took
  printf '// bench edit 1\n' >>"$repo/$file"
  (cd "$repo" && "$@" >/dev/null 2>&1) || true
  printf '// bench edit 2\n' >>"$repo/$file"
  t0=$(now)
  (cd "$repo" && "$@" >/dev/null 2>&1) || true
  plain=$(since "$t0")
  rows="$rows| adhoc: \`aver check -- cargo test -p aver-core --lib\` (one-line edit in aver-core) | $cold s | $warm s | $plain s (warm) | $(perl -e "printf '%.2f', $warm / $plain") (target $(perl -e "printf '%.2f', (1.2 * $plain + 1) / $plain")) | \`$last\` |
"
fi

{
  echo "# Native verify benchmark"
  echo
  echo "\`make bench-native\` on $(uname -sm), $(date -u +%Y-%m-%d). Each case: \`aver workspace new\`, a one-line edit and \`aver verify\` (cold pools), then a second one-line edit and \`aver verify\` (measured), against running the tests directly on the same edit in a warm checkout. Target (plan §3.5): calc under 2 s; aver at most 1.05 times warm \`cargo test\` plus 1 s."
  echo
  echo "| Case | First verify (cold) | Verify after a one-line edit | The tests alone | Ratio | Verdict |"
  echo "|---|---|---|---|---|---|"
  printf '%s' "$rows"
  echo
  echo "Where each verify's time went (its report's last timing line):"
  echo "$notes"
} >"$OUT"
cat "$OUT"
# Host dependency stores are read-only on purpose.
chmod -R u+w "$WORK" 2>/dev/null || true
rm -rf "$WORK"
