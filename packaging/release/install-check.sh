#!/usr/bin/env bash
# Installs a release with site/install.sh into a fresh HOME, as a user
# would, and checks the installed aver: `--version` and `aver demo`.
#
#   packaging/release/install-check.sh <tag> <version> draft|tag|latest
#
# draft: the draft's assets, downloaded with gh (a draft has no public URL),
# served to install.sh from a file:// mirror of the release layout.
# tag: the published release by its tag (AVER_VERSION=<tag>).
# latest: the published release install.sh picks with no AVER_VERSION.
# AVER_RELEASE_BASE_URL, for tag and latest, overrides GitHub's releases URL.
set -euo pipefail

[ $# -eq 3 ] || { echo "usage: $0 <tag> <version> draft|tag|latest" >&2; exit 2; }
tag=$1
version=$2
mode=$3
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/aver-install-check.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/home"

env=(HOME="$tmp/home" AVER_INSTALL_DIR="$tmp/bin")
case "$mode" in
  draft)
    mkdir -p "$tmp/mirror/download/$tag"
    gh release download "$tag" --dir "$tmp/mirror/download/$tag" --pattern 'aver-*' --pattern SHA256SUMS
    env+=(AVER_VERSION="$tag" AVER_RELEASE_BASE_URL="file://$tmp/mirror")
    ;;
  tag) env+=(AVER_VERSION="$tag") ;;
  latest) ;;
  *) echo "unknown mode '$mode'" >&2; exit 2 ;;
esac
[ "$mode" = draft ] || [ -z "${AVER_RELEASE_BASE_URL:-}" ] || env+=(AVER_RELEASE_BASE_URL="$AVER_RELEASE_BASE_URL")

env -u AVER_VERSION -u AVER_RELEASE_BASE_URL "${env[@]}" sh "$root/site/install.sh"
"$root/packaging/release/smoke.sh" "$tmp/bin/aver" "$version"
echo "install.sh ($mode) installed aver $version"
