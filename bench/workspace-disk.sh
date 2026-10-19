#!/usr/bin/env bash
# Disk workspace benchmark: directory creation and sync on date-fns and on a
# synthetic 16,000-file TypeScript repository with a 20,000-file
# node_modules, against `git worktree add`. Writes bench/results/disk.json
# (and bench/results/disk-copy.json for the copy strategy, the ext4 path).
#
#   bench/workspace-disk.sh               # default strategy for this host, then copy
#   COUNT=50 bench/workspace-disk.sh      # more workspaces per repository
set -euo pipefail
cd "$(dirname "$0")/.."

REV="717ce0a807ea4c6b540d015b5c408723175b2838"
DEST="bench/repos/date-fns"
COUNT="${COUNT:-20}"

if [ ! -d "$DEST" ]; then
  mkdir -p bench/repos
  rm -rf "$DEST.tmp"
  git init --bare --quiet "$DEST.tmp"
  git -C "$DEST.tmp" fetch --quiet --depth 1 https://github.com/date-fns/date-fns "$REV"
  mv "$DEST.tmp" "$DEST"
fi

cargo build --release --quiet -p aver --example bench_disk
mkdir -p bench/results
./target/release/examples/bench_disk --repo "$DEST" --rev "$REV" --synthetic \
  --count "$COUNT" --out bench/results/disk.json >/dev/null
./target/release/examples/bench_disk --repo "$DEST" --rev "$REV" --synthetic \
  --count 5 --clone copy --out bench/results/disk-copy.json >/dev/null
echo "==> wrote bench/results/disk.json and bench/results/disk-copy.json"
