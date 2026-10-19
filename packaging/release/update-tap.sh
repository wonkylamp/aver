#!/usr/bin/env bash
# Commits Formula/aver.rb for <version> to the Homebrew tap, filled by
# packaging/homebrew/update-formula.sh from the published SHA256SUMS. Does
# nothing when the tap already has that formula.
#
#   packaging/release/update-tap.sh <version>
#
# TAP_TOKEN can push to the tap. TAP_URL overrides
# https://github.com/wonkylamp/homebrew-tap.git.
set -euo pipefail

[ $# -eq 1 ] || { echo "usage: $0 <version>" >&2; exit 2; }
version=$1
root=$(cd "$(dirname "$0")/../.." && pwd)
url=${TAP_URL:-https://github.com/wonkylamp/homebrew-tap.git}
token=${TAP_TOKEN:?TAP_TOKEN is not set}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/aver-tap.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

auth=$(printf 'x-access-token:%s' "$token" | base64 | tr -d '\n')
git_() { git -c "http.https://github.com/.extraheader=AUTHORIZATION: basic $auth" "$@"; }

git_ clone -q --depth 1 "$url" "$tmp/tap"
mkdir -p "$tmp/tap/Formula"
"$root/packaging/homebrew/update-formula.sh" "$version" "$tmp/tap/Formula/aver.rb"
cd "$tmp/tap"
git add Formula/aver.rb
if git diff --cached --quiet; then
  echo "The tap already has aver $version"
  exit 0
fi
git -c user.name="aver release" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
  commit -q -m "aver $version"
git_ push -q origin HEAD
echo "Pushed Formula/aver.rb for aver $version to the tap"
