#!/usr/bin/env bash
# Opens a pull request that sets the Claude Code plugin's version, in its
# plugin.json and in .claude-plugin/marketplace.json, to <version>. Runs
# after the release is published, so a plugin update never points /aver at
# a CLI release that is not there yet. Does nothing when main already has
# that version or the pull request is open.
#
#   packaging/release/bump-plugin.sh <version>
#
# Run in a checkout of main. PR_TOKEN can push a branch and open a pull
# request (a personal or app token, so CI runs on it). REPO_URL overrides
# https://github.com/$GH_REPO.git.
set -euo pipefail

[ $# -eq 1 ] || { echo "usage: $0 <version>" >&2; exit 2; }
version=$1
repo=${GH_REPO:?GH_REPO is not set}
token=${PR_TOKEN:?PR_TOKEN is not set}
url=${REPO_URL:-https://github.com/$repo.git}
root=$(cd "$(dirname "$0")/../.." && pwd)
branch=release/plugin-$version
plugin=integrations/claude-code/.claude-plugin/plugin.json
market=.claude-plugin/marketplace.json

cd "$root"
current=$(jq -r .version "$plugin")
if [ "$current" = "$version" ]; then
  echo "The plugin is already at $version"
  exit 0
fi
open=$(GH_TOKEN=$token gh api "repos/$repo/pulls?state=open&head=${repo%%/*}:$branch" --jq length)
if [ "$open" != 0 ]; then
  echo "A pull request from $branch is already open"
  exit 0
fi

set_version() { # <file> <jq path>
  jq --arg v "$version" "$2 = \$v" "$1" >"$1.tmp"
  mv "$1.tmp" "$1"
}
set_version "$plugin" .version
set_version "$market" '(.plugins[] | select(.name == "aver") | .version)'

auth=$(printf 'x-access-token:%s' "$token" | base64 | tr -d '\n')
git checkout -q -B "$branch"
git add "$plugin" "$market"
git -c user.name="aver release" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
  commit -q -m "Claude Code plugin $version, for the aver $version release"
git -c "http.https://github.com/.extraheader=AUTHORIZATION: basic $auth" push -q --force "$url" "HEAD:refs/heads/$branch"
GH_TOKEN=$token gh api -X POST "repos/$repo/pulls" -f title="Claude Code plugin $version" -f head="$branch" -f base=main \
  -f body="Sets the plugin's version to $version in $plugin and $market, now that the aver $version release is published. Opened by release.yml." --jq .html_url
