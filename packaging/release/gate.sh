#!/usr/bin/env bash
# Refuses a release unless it runs on a tag, the tag names the version in Cargo.toml, the
# checkout is the tagged commit, that commit is on main, and CI's ci-ok
# check run passed on it. Writes version, prerelease and series (vX.Y) to
# $GITHUB_OUTPUT.
#
#   packaging/release/gate.sh <ref> <commit>
#
# Needs GH_REPO and a gh that can read the repository's check runs.
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <ref> <commit>" >&2; exit 2; }
sha=$2
repo=${GH_REPO:?GH_REPO is not set}
root=$(cd "$(dirname "$0")/../.." && pwd)

refuse() {
  echo "::error title=Release refused::$*"
  exit 1
}

version=$(awk '
  /^\[workspace\.package\]/ { p = 1; next }
  /^\[/ { p = 0 }
  p && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
' "$root/Cargo.toml")
case "$1" in
  refs/tags/*) tag=${1#refs/tags/} ;;
  *) refuse "$1 is not a tag. Run the release on its tag: gh workflow run release.yml --ref v$version" ;;
esac
[ -n "$version" ] || refuse "Cargo.toml has no [workspace.package] version"
[ "$tag" = "v$version" ] || refuse "the tag $tag does not name the version in Cargo.toml (v$version). Run scripts/release.sh <version> and tag its commit."
[ "$(git -C "$root" rev-parse HEAD)" = "$sha" ] || refuse "the checkout is not the tagged commit $sha"

status=$(gh api "repos/$repo/compare/main...$sha" --jq .status) ||
  refuse "could not compare $sha with main"
case "$status" in
  identical | behind) ;;
  *) refuse "$tag ($sha) is not on main (compared with main: $status). Release only commits main has." ;;
esac

# filter=latest keeps the newest ci-ok run of each app; only GitHub Actions'
# counts, so another app with checks: write cannot pass the gate.
runs=$(gh api "repos/$repo/commits/$sha/check-runs?check_name=ci-ok&filter=latest&per_page=100") ||
  refuse "could not read the check runs of $sha"
ci=$(jq -r '[.check_runs[] | select(.app.slug == "github-actions")] | first | if . == null then "missing" else "\(.status)/\(.conclusion)" end' <<<"$runs")
case "$ci" in
  completed/success) ;;
  missing) refuse "$sha has no ci-ok check run. Run CI on the tag (gh workflow run ci.yml --ref $tag), wait for it to pass, then re-run this workflow." ;;
  *) refuse "ci-ok on $sha is $ci, not completed/success. Fix main and release a new version, or re-run CI if the failure was a flake, then re-run this workflow." ;;
esac

prerelease=false
case "$version" in *-*) prerelease=true ;; esac
series=v$(cut -d. -f1-2 <<<"${version%%-*}")
{
  echo "version=$version"
  echo "prerelease=$prerelease"
  echo "series=$series"
} >>"${GITHUB_OUTPUT:-/dev/null}"
echo "Releasing aver $version from $sha (ci-ok passed; on main)"
