#!/usr/bin/env bash
# Fills packaging/homebrew/aver.rb for one release: the version and the
# SHA-256 of each binary, read from the release's SHA256SUMS. Writes the
# formula to <output>, or to stdout without one.
#
#   packaging/homebrew/update-formula.sh 0.2.0 ../homebrew-tap/Formula/aver.rb
#
# AVER_RELEASE_BASE_URL overrides https://github.com/wonkylamp/aver/releases (the
# tests use a file:// directory with the same layout).
set -euo pipefail

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  echo "usage: $0 <version> [output]" >&2
  exit 2
fi
version=${1#v}
output=${2:-}
base_url=${AVER_RELEASE_BASE_URL:-https://github.com/wonkylamp/aver/releases}
template="$(cd "$(dirname "$0")" && pwd)/aver.rb"

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]] || {
  echo "error: '$1' is not a version like 0.2.0" >&2
  exit 2
}

sums=$(curl -fsSL --proto '=https,file' "$base_url/download/v$version/SHA256SUMS") || {
  echo "error: could not download SHA256SUMS of release v$version from $base_url" >&2
  exit 1
}

# The template's leading comment is about the template; the tap gets its own.
formula="# Generated from packaging/homebrew/aver.rb in wonkylamp/aver by update-formula.sh $version.
$(awk 'body || !/^#/ { body = 1; print }' "$template")"
formula=${formula//@VERSION@/$version}
for target in darwin-aarch64 darwin-x86_64 linux-aarch64 linux-x86_64; do
  sha=$(awk -v name="aver-$target" '$2 == name || $2 == "*" name { print $1; exit }' <<<"$sums")
  [[ "$sha" =~ ^[0-9a-f]{64}$ ]] || {
    echo "error: SHA256SUMS of release v$version has no valid entry for aver-$target" >&2
    exit 1
  }
  placeholder="@SHA256_$(tr 'a-z-' 'A-Z_' <<<"$target")@"
  formula=${formula//$placeholder/$sha}
done
if grep -q '@[A-Z0-9_]*@' <<<"$formula"; then
  echo "error: unfilled placeholders remain in the formula" >&2
  exit 1
fi

if [ -n "$output" ]; then
  printf '%s\n' "$formula" >"$output"
  echo "wrote $output for aver $version" >&2
else
  printf '%s\n' "$formula"
fi
