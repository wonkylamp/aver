#!/usr/bin/env bash
# Picks the base and head commits to compare (a pull request's, a merge
# group's, or a push's; a new branch's push from its merge base with the
# default branch) and makes sure both exist in the repository, fetching them when the checkout is shallow, along with enough
# history to find their merge base. Outputs base, head, merge-base and skip.
set -euo pipefail

output=${GITHUB_OUTPUT:-/dev/null}
repo=${WORKING_DIRECTORY:-.}
event=${GITHUB_EVENT_NAME:-}
event_path=${GITHUB_EVENT_PATH:-}
zero=0000000000000000000000000000000000000000

die() {
  echo "::error title=aver::$1"
  exit 2
}

skip() {
  echo "::notice title=aver::$1"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '## Aver skipped\n\n%s\n' "$1" >>"$GITHUB_STEP_SUMMARY"
  fi
  printf 'skip=true\nbase=\nhead=\n' >>"$output"
  exit 0
}

field() {
  [ -n "$event_path" ] && [ -f "$event_path" ] || die "GITHUB_EVENT_PATH does not name a file"
  jq -r "$1 // empty" "$event_path"
}

base=${INPUT_BASE:-}
head=${INPUT_HEAD:-}
if [ -n "$base" ] || [ -n "$head" ]; then
  [ -n "$base" ] && [ -n "$head" ] || die "set both the base and head inputs, or neither"
else
  case "$event" in
    pull_request | pull_request_review | pull_request_review_comment)
      base=$(field .pull_request.base.sha)
      head=$(field .pull_request.head.sha)
      ;;
    pull_request_target)
      die "do not run aver on pull_request_target: it executes the pull request's code with a write token and the base repository's secrets. Use pull_request."
      ;;
    merge_group)
      base=$(field .merge_group.base_sha)
      head=$(field .merge_group.head_sha)
      ;;
    push)
      base=$(field .before)
      head=$(field .after)
      [ "$head" != "$zero" ] || skip "This push deleted the branch, so there is nothing to verify."
      if [ "$base" = "$zero" ]; then
        # A new branch: compare with where it left the default branch.
        default=$(field .repository.default_branch)
        [ -n "$default" ] || skip "This push created the branch and the event names no default branch to compare with."
        new_branch=$default
        if git -C "$repo" fetch --no-tags --depth=1 origin "$default" >/dev/null 2>&1; then
          base=$(git -C "$repo" rev-parse FETCH_HEAD)
        else
          skip "This push created the branch and $default could not be fetched to compare with."
        fi
      fi
      ;;
    *)
      die "the $event event carries no base and head commits; set the base and head inputs"
      ;;
  esac
  [ -n "$base" ] && [ -n "$head" ] || die "the $event event payload has no base or head commit"
fi

for rev in "$base" "$head"; do
  case "$rev" in -*) die "invalid revision '$rev'" ;; esac
  if ! git -C "$repo" cat-file -e "$rev^{commit}" 2>/dev/null; then
    echo "Fetching $rev, which the checkout does not have"
    git -C "$repo" fetch --no-tags --depth=1 origin "$rev" || die "could not fetch $rev from origin"
    git -C "$repo" cat-file -e "$rev^{commit}" 2>/dev/null || die "$rev is not a commit"
  fi
done

# A pull request's recorded base is the base branch's tip when the request
# was last updated, which can be ahead of the commit the head forked from.
# aver ci then compares with their merge base and records both in the
# receipt, so the history down to the merge base must be present.
merge_base() { git -C "$repo" merge-base "$base" "$head" 2>/dev/null || true; }
mb=$(merge_base)
if [ -z "$mb" ] && [ "$(git -C "$repo" rev-parse --is-shallow-repository)" = true ]; then
  for depth in 64 512 4096; do
    echo "Fetching $depth more commits of history to find the merge base"
    git -C "$repo" fetch --no-tags --deepen="$depth" origin "$base" "$head" >/dev/null 2>&1 || true
    mb=$(merge_base)
    [ -z "$mb" ] || break
  done
  if [ -z "$mb" ]; then
    echo "Fetching the full history to find the merge base"
    git -C "$repo" fetch --no-tags --unshallow origin "$base" "$head" >/dev/null 2>&1 || true
    mb=$(merge_base)
  fi
fi

if [ -n "${new_branch:-}" ]; then
  [ -n "$mb" ] || skip "This push created the branch, which shares no history with $new_branch."
  [ "$mb" != "$(git -C "$repo" rev-parse "$head^{commit}")" ] || skip "This push created the branch with no commits beyond $new_branch."
  echo "This push created the branch: comparing with its merge base with $new_branch"
  base=$mb
fi

printf 'skip=false\nbase=%s\nhead=%s\nmerge-base=%s\n' "$base" "$head" "$mb" >>"$output"
if [ -z "$mb" ]; then
  echo "::warning title=aver::$base and $head share no history in this checkout, so aver ci compares with $base as given"
elif [ "$mb" != "$(git -C "$repo" rev-parse "$base^{commit}")" ]; then
  echo "Base $base is not an ancestor of head $head: aver ci compares with their merge base $mb and records both"
fi
echo "Comparing head $head with base $base"
