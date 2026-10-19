#!/usr/bin/env bash
# Import benchmark: fetch date-fns at a pinned commit (bare, never checked
# out), then time import, re-import and export round trip in release mode.
# Results go to bench/results/import.json.
set -euo pipefail

cd "$(dirname "$0")/.."

REPO_URL="https://github.com/date-fns/date-fns"
REV="717ce0a807ea4c6b540d015b5c408723175b2838"
DEST="bench/repos/date-fns"

if [ ! -d "$DEST" ]; then
  mkdir -p bench/repos
  rm -rf "$DEST.tmp"
  git init --bare --quiet "$DEST.tmp"
  if ! git -C "$DEST.tmp" fetch --quiet --depth 1 "$REPO_URL" "$REV"; then
    rm -rf "$DEST.tmp"
    git clone --bare --quiet "$REPO_URL" "$DEST.tmp"
  fi
  mv "$DEST.tmp" "$DEST"
fi

mkdir -p bench/results
cargo run --release --quiet -p aver --example bench_import -- \
  "$DEST" "$REV" bench/results/import.json
