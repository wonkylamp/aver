#!/usr/bin/env bash
# Opens the pull request that releases <version> when it merges
# (docs/RELEASING.md): on a branch release/v<version> from the current
# commit, sets Cargo.toml's version, Cargo.lock's workspace entries, the
# action refs the docs show (wonkylamp/aver/action@v<version>), the README's
# version badge and the init-ci goldens, runs `make ci-local`, commits,
# pushes the branch and opens the pull request with gh. It never tags:
# ci.yml tags the merged commit once its ci-ok passes.
#
#   scripts/release.sh <version> [--local]
#
# --local stops after the commit, pushing nothing.
# RELEASE_CHECK overrides the check command (default `make ci-local`).
# The plugin's version is bumped after the release is published, by
# release.yml's plugin job.
set -euo pipefail

local_only=false
[ "${2:-}" = --local ] && local_only=true
[ $# -eq 1 ] || { [ $# -eq 2 ] && $local_only; } || { echo "usage: $0 <version> [--local], e.g. 0.4.2" >&2; exit 2; }
new=${1#v}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
die() { echo "error: $*" >&2; exit 1; }

[[ "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "'$1' is not a version like 0.4.2 or 0.5.0-rc.1"
[ -z "$(git status --porcelain)" ] || die "the working tree has changes; commit or stash them first"
old=$(awk '
  /^\[workspace\.package\]/ { p = 1; next }
  /^\[/ { p = 0 }
  p && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
' Cargo.toml)
[ -n "$old" ] || die "Cargo.toml has no [workspace.package] version"
[ "$old" != "$new" ] || die "Cargo.toml is already at $new"
git rev-parse -q --verify "refs/tags/v$new" >/dev/null && die "the tag v$new already exists"
branch=release/v$new
git rev-parse -q --verify "refs/heads/$branch" >/dev/null && die "the branch $branch already exists"

# Literal replacement in the files that contain <from>.
replace() { # <from> <to> <file>...
  python3 - "$@" <<'PY'
import sys
old, new, *files = sys.argv[1:]
for path in files:
    text = open(path).read()
    open(path, "w").write(text.replace(old, new))
PY
}

echo "== aver $old -> $new on $branch" >&2
git switch -q -c "$branch"
awk -v new="$new" '
  /^\[workspace\.package\]/ { p = 1 }
  /^\[/ && !/^\[workspace\.package\]/ { p = 0 }
  p && $1 == "version" && !done { sub(/"[^"]*"/, "\"" new "\""); done = 1 }
  { print }
' Cargo.toml >Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
cargo update --workspace --offline --quiet

refs=$(git grep -lF -e "wonkylamp/aver/action@v$old" -e "AVER_VERSION=v$old" -- '*.md' '*.yml' || true)
for f in $refs; do
  replace "wonkylamp/aver/action@v$old" "wonkylamp/aver/action@v$new" "$f"
  replace "AVER_VERSION=v$old" "AVER_VERSION=v$new" "$f"
done
replace "version-$old-informational" "version-$new-informational" README.md
replace "alt=\"Version $old\"" "alt=\"Version $new\"" README.md

echo "== goldens" >&2
AVER_UPDATE_GOLDEN=1 cargo test --quiet -p aver --test init_ci >/dev/null
grep -rqF "# v$new" crates/aver/tests/golden/init-ci || die "the init-ci goldens do not name v$new"

echo "== ${RELEASE_CHECK:-make ci-local}" >&2
sh -c "${RELEASE_CHECK:-make ci-local}"

git add -A
git commit -q -m "Release $new: version, action refs and goldens"
echo "Committed $(git rev-parse --short HEAD) on $branch: aver $new" >&2
if $local_only; then
  echo "Next: git push -u origin $branch, open its pull request, and merge it; ci.yml tags and releases v$new." >&2
  exit 0
fi
git push -u origin "$branch"
gh pr create --base main --head "$branch" --title "Release $new" \
  --body "Merging this releases aver $new: once ci-ok passes on main, ci.yml's tag job tags v$new and starts release.yml (docs/RELEASING.md)."
