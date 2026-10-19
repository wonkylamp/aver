#!/usr/bin/env bash
# Workspace benchmark: 1,000 opens in-process and over signed HTTP, storage
# per workspace, and edit + snapshot of 100 workspaces. Writes
# bench/results/workspaces.json.
#
#   bench/workspaces.sh                                   # synthetic 2,000-file base
#   bench/workspaces.sh --home DIR --snapshot snap_...    # an imported snapshot
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --release --quiet -p aver --example bench_workspaces -- "$@"
