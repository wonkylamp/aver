#!/usr/bin/env bash
# Runs a release binary before it ships: `--version` must print
# `aver <version>`, a Linux binary must be statically linked, and `aver demo`
# (offline, in a throwaway directory) must end in its PASS.
#
#   packaging/release/smoke.sh <binary> <version>
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <binary> <version>" >&2; exit 2; }
bin=$1
version=$2
fail() {
  echo "::error title=Smoke test failed::$(basename "$bin"): $*"
  exit 1
}

[ -x "$bin" ] || fail "not an executable file"
got=$("$bin" --version) || fail "--version exited $?"
[ "$got" = "aver $version" ] || fail "--version printed '$got', not 'aver $version'"

case "$(basename "$bin")" in
  *linux*)
    kind=$(file -b "$bin")
    case "$kind" in
      *"statically linked"* | *"static-pie linked"*) ;;
      *) fail "not statically linked: $kind" ;;
    esac
    ;;
esac

log=$(mktemp "${TMPDIR:-/tmp}/aver-smoke.XXXXXX")
trap 'rm -f "$log"' EXIT
"$bin" demo >"$log" 2>&1 || { cat "$log"; fail "aver demo exited $?"; }
for line in "NOT DONE: FAIL_OWN" "DONE: PASS"; do
  grep -qF "$line" "$log" || { cat "$log"; fail "aver demo printed no '$line'"; }
done
echo "$(basename "$bin"): aver $version, demo passed"
