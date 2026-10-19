#!/usr/bin/env bash
# Points the action's series tag (v0.4 for 0.4.x) at the released commit,
# creating it the first time, so `uses: wonkylamp/aver/action@v0.4` follows
# patch releases. action/install-aver.sh maps that ref to the release whose
# version the commit's Cargo.toml names.
#
#   packaging/release/move-tag.sh <series tag> <commit>
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <series tag> <commit>" >&2; exit 2; }
series=$1
sha=$2
repo=${GH_REPO:?GH_REPO is not set}
[[ "$series" =~ ^v[0-9]+\.[0-9]+$ ]] || { echo "::error::'$series' is not a series tag like v0.4"; exit 2; }

if current=$(gh api "repos/$repo/git/ref/tags/$series" --jq .object.sha 2>/dev/null); then
  if [ "$current" = "$sha" ]; then
    echo "$series already points at $sha"
    exit 0
  fi
  gh api -X PATCH "repos/$repo/git/refs/tags/$series" -f sha="$sha" -F force=true >/dev/null
  echo "Moved $series from $current to $sha"
else
  gh api -X POST "repos/$repo/git/refs" -f ref="refs/tags/$series" -f sha="$sha" >/dev/null
  echo "Created $series at $sha"
fi
