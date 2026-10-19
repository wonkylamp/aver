#!/usr/bin/env bash
# Search and tree benchmark: date-fns at the pinned commit (fetched bare by
# bench/import.sh if absent), imported into a fresh store; times
# workspace search and recursive listing over a workspace with one edit.
# Results go to bench/results/search.json.
set -euo pipefail

cd "$(dirname "$0")/.."

REV="717ce0a807ea4c6b540d015b5c408723175b2838"
DEST="bench/repos/date-fns"

if [ ! -d "$DEST" ]; then
  bench/import.sh
fi

mkdir -p bench/results
cargo run --release --quiet -p aver --example bench_search -- \
  "$DEST" "$REV" bench/results/search.json
