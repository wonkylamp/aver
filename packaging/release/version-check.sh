#!/usr/bin/env bash
# Fails when Cargo.toml's version is untagged and lower than a version
# already tagged v<major>.<minor>.<patch>[-<pre>], which release-plz would
# otherwise tag (it only asks whether the tag exists). Run in a checkout
# with every tag.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
version=$(awk '
  /^\[workspace\.package\]/ { p = 1; next }
  /^\[/ { p = 0 }
  p && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
' "$root/Cargo.toml")
[ -n "$version" ] || { echo "::error::Cargo.toml has no [workspace.package] version"; exit 1; }
if git -C "$root" rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  echo "v$version is already tagged; nothing to release"
  exit 0
fi

# SemVer order, which sort -V lacks for prereleases: 1.0.0-rc.1 < 1.0.0.
key() { # <version> -> a sort key
  local core=${1%%-*} pre=
  [ "$core" = "$1" ] || pre=${1#*-}
  printf '%s %s %s\n' "$core" "${pre:+0}${pre:-1}" "$1"
}
highest=$(git -C "$root" tag -l 'v[0-9]*.[0-9]*.[0-9]*' | sed 's/^v//' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' |
  while read -r v; do key "$v"; done | sort -k1,1V -k2,2V | tail -n 1 | cut -d' ' -f3)
echo "Cargo.toml: $version; highest tagged: ${highest:-none}"
[ -n "$highest" ] || exit 0
top=$( { key "$version"; key "$highest"; } | sort -k1,1V -k2,2V | tail -n 1 | cut -d' ' -f3)
if [ "$top" != "$version" ]; then
  echo "::error title=Version went down::Cargo.toml says $version, lower than the released v$highest. Set a version above it (scripts/release.sh)."
  exit 1
fi
