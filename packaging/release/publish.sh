#!/usr/bin/env bash
# Publishes the draft release of <tag>: as the latest release, or as a
# prerelease, which install.sh's default and the Homebrew tap never pick.
# A release already published is left as it is.
#
#   packaging/release/publish.sh <tag> <prerelease: true|false>
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <tag> <prerelease>" >&2; exit 2; }
tag=$1
prerelease=$2

draft=$(gh release view "$tag" --json isDraft --jq .isDraft)
if [ "$draft" != true ]; then
  echo "Release $tag is already published"
  exit 0
fi
if [ "$prerelease" = true ]; then
  gh release edit "$tag" --draft=false --prerelease
else
  gh release edit "$tag" --draft=false --latest
fi
echo "Published $tag"
