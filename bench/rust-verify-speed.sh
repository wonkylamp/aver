#!/usr/bin/env bash
# Rust verify speed against plain cargo, on a scratch clone of this
# repository at HEAD (ARCHITECTURE.md §2.4.2, bench/dogfood/LOG.md round 12).
# Times, in order:
#
#   1. cargo test --workspace, cold (an empty target) and warm (after a
#      one-line change to crates/aver-core/src/lib.rs);
#   2. aver workspace new on the clone (its target is warm), and the disk the
#      new directory's target takes;
#   3. aver verify after a one-line change in the workspace, cold (a new home,
#      so the base side runs too), then aver check and aver verify after a
#      second one-line change, warm (the base side is cached).
#
# Each step's wall time goes to $OUT/times.tsv and the commands' output next
# to it; verify's own per-run timings are in $OUT/verify-*.json. Needs about
# 20 GiB free (a debug cargo target of this repository is large).
#
#   bench/rust-verify-speed.sh                      # ./target/release/aver, built first
#   AVER=/path/to/aver OUT=/tmp/speed bench/rust-verify-speed.sh
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
OUT=${OUT:-$repo/bench/results/rust-verify-speed}
if [ -z "${AVER:-}" ]; then
  cargo build --release --quiet -p aver --manifest-path "$repo/Cargo.toml"
  AVER=$repo/target/release/aver
fi
rm -rf "$OUT"
mkdir -p "$OUT"
clone=$OUT/clone
home=$OUT/home
git clone --quiet --no-hardlinks "$repo" "$clone"
export AVER_HOME=$home
unset CARGO_TARGET_DIR

now() { perl -MTime::HiRes=time -e 'printf "%.1f\n", time'; }
step() {
  local name=$1
  shift
  local start end code
  start=$(now)
  set +e
  "$@" >"$OUT/$name.log" 2>&1
  code=$?
  set -e
  end=$(now)
  printf '%s\t%s\t%.1f\n' "$name" "$code" "$(echo "$end - $start" | bc)" | tee -a "$OUT/times.tsv"
}
edit() { echo "// rust-verify-speed: $2" >>"$1/crates/aver-core/src/lib.rs"; }

printf 'step\texit\tseconds\n' >"$OUT/times.tsv"
step cargo-test-cold sh -c "cd '$clone' && cargo test --workspace"
edit "$clone" "cargo warm"
step cargo-test-warm sh -c "cd '$clone' && cargo test --workspace"
git -C "$clone" checkout --quiet -- crates/aver-core/src/lib.rs

step workspace-new sh -c "cd '$clone' && '$AVER' workspace new . -c 'speed: a one-line change' > '$OUT/workspace.txt'"
ws=$(tail -n 1 "$OUT/workspace.txt")
du -sk "$ws/target" | awk '{printf "workspace target\t%.2f GiB\n", $1 / 1048576}' | tee -a "$OUT/times.tsv"

edit "$ws" "verify cold"
step verify-cold sh -c "cd '$ws' && '$AVER' verify --json > '$OUT/verify-cold.json'"
edit "$ws" "verify warm"
step check-warm sh -c "cd '$ws' && '$AVER' check"
step verify-warm sh -c "cd '$ws' && '$AVER' verify --json > '$OUT/verify-warm.json'"
du -sk "$ws/target" | awk '{printf "workspace target after\t%.2f GiB\n", $1 / 1048576}' | tee -a "$OUT/times.tsv"
"$AVER" daemon stop >/dev/null 2>&1 || true
echo "==> $OUT/times.tsv"
