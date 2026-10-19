#!/usr/bin/env bash
# Puts the binaries, their .sha256 files and SHA256SUMS (from sums.sh) in
# <dir> on the draft release of <tag>: created with generated notes, or,
# when the workflow is re-run, the existing draft's assets replaced. A
# published release is never touched.
#
#   packaging/release/draft.sh <tag> <dir> <prerelease: true|false>
set -euo pipefail

[ $# -eq 3 ] || { echo "usage: $0 <tag> <dir> <prerelease>" >&2; exit 2; }
tag=$1
dir=$2
prerelease=$3

cd "$dir"
[ -f SHA256SUMS ] || { echo "::error::$dir has no SHA256SUMS; run packaging/release/sums.sh first"; exit 1; }
files=(aver-* SHA256SUMS)

if draft=$(gh release view "$tag" --json isDraft --jq .isDraft 2>/dev/null); then
  [ "$draft" = true ] || { echo "::error::release $tag is already published; release a new version instead"; exit 1; }
  gh release upload "$tag" "${files[@]}" --clobber
  echo "Replaced the assets of the draft release $tag"
else
  flags=(--draft --verify-tag --title "$tag" --generate-notes)
  [ "$prerelease" = true ] && flags+=(--prerelease)
  gh release create "$tag" "${files[@]}" "${flags[@]}"
  echo "Created the draft release $tag"
fi
