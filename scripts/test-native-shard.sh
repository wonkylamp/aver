#!/usr/bin/env bash
# Runs one shard of `make test-native` (cargo test -p aver -- --ignored) so CI
# can spread the ignored tests (they run Node natively) over parallel jobs.
# Together the shards run every test once:
#
#   workspace  cli_workspace and ci_cli, about 3 minutes between them
#   rest       the library's and binary's tests, the doc-tests, and every test
#              target the shards above do not name, so a new file lands here
#
# Each shard compiles only the targets it runs.
set -euo pipefail
cd "$(dirname "$0")/.."

threads=${NATIVE_TEST_THREADS:-2}
named=(cli_workspace ci_cli)

case "${1:-}" in
  workspace)
    cargo test -p aver --no-fail-fast --test cli_workspace --test ci_cli -- --ignored --test-threads="$threads"
    ;;
  rest)
    args=()
    while IFS= read -r target; do
      skip=
      for n in "${named[@]}"; do [ "$target" = "$n" ] && skip=1; done
      [ -n "$skip" ] || args+=(--test "$target")
    done < <(cargo metadata --no-deps --format-version 1 |
      jq -r '.packages[] | select(.name == "aver") | .targets[] | select(.kind == ["test"]) | select((.["required-features"] // []) | length == 0) | .name')
    [ "${#args[@]}" -gt 0 ] || { echo "no test targets found" >&2; exit 1; }
    cargo test -p aver --no-fail-fast --lib --bins "${args[@]}" -- --ignored --test-threads="$threads"
    cargo test -p aver --no-fail-fast --doc -- --ignored --test-threads="$threads"
    ;;
  *)
    echo "usage: $0 workspace|rest" >&2
    exit 2
    ;;
esac
