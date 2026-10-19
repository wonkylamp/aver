#!/usr/bin/env bash
# Every benchmark on real data, then bench/results.md:
#   1. import, re-import and export round trip of date-fns (bench/import.sh)
#   2. date-fns imported into an AVER_HOME that persists for this run, and
#      1,000 workspace opens on that real snapshot (plus the synthetic base)
#   3. workspace search and recursive listing over the date-fns snapshot
#   4. every check of fixtures/calc run twice on one snapshot natively; the
#      second pass must start zero runs
# Set AVER_BENCH_HOME to reuse a home across runs.
set -euo pipefail
cd "$(dirname "$0")/.."

REV="717ce0a807ea4c6b540d015b5c408723175b2838"
AVER=./target/release/aver

echo "==> import benchmark (date-fns @ ${REV:0:7})"
bench/import.sh

cargo build --release --quiet -p aver --bin aver --example bench_cache
BENCH_HOME="${AVER_BENCH_HOME:-$(mktemp -d -t aver-bench)/home}"
echo "==> date-fns into $BENCH_HOME"
AVER_HOME="$BENCH_HOME" "$AVER" init >/dev/null
SNAPSHOT=$(AVER_HOME="$BENCH_HOME" "$AVER" import bench/repos/date-fns --rev "$REV" |
  node -e 'process.stdout.write(JSON.parse(require("fs").readFileSync(0, "utf8")).snapshot)')
echo "    snapshot $SNAPSHOT"

echo "==> 1,000 workspaces on the date-fns snapshot"
bench/workspaces.sh --home "$BENCH_HOME" --snapshot "$SNAPSHOT" --out bench/results/workspaces-date-fns.json

echo "==> 1,000 workspaces on the synthetic 2,000-file base"
bench/workspaces.sh --out bench/results/workspaces.json

echo "==> search and tree over the date-fns snapshot"
bench/search.sh

echo "==> cache: fixtures/calc checks twice on one snapshot"
./target/release/examples/bench_cache fixtures/calc bench/results/cache.json

node bench/report.ts > bench/results.md
echo "==> wrote bench/results.md"
