#!/usr/bin/env bash
# Creates or updates the one pull request comment that carries the receipt
# summary, found by the marker on its first line. Problems here never fail the
# job: the verdict comes from the action's last step.
set -euo pipefail

marker='<!-- aver-receipt -->'
# GitHub rejects comment bodies over 65536 characters.
max_chars=65000
out_dir=${OUT_DIR:?OUT_DIR is not set}
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}
event_path=${GITHUB_EVENT_PATH:?GITHUB_EVENT_PATH is not set}

# Messages go to the step log even while gh_api output is redirected.
exec 3>&1

notice() {
  echo "::notice title=aver comment::$1" >&3
  exit 0
}

number=$(jq -r '.pull_request.number // empty' "$event_path")
[ -n "$number" ] || notice "not a pull request event, so no comment"

head_repo=$(jq -r '.pull_request.head.repo.full_name // empty' "$event_path")
base_repo=$(jq -r '.pull_request.base.repo.full_name // empty' "$event_path")
if [ "$head_repo" != "$base_repo" ]; then
  notice "the pull request comes from a fork (${head_repo:-a deleted repository}), whose token is read-only, so no comment. The job summary and the aver-receipt artifact have the receipt."
fi

[ -f "$out_dir/summary.md" ] || notice "aver ci wrote no summary.md, so no comment. The step log has the error."

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
errors="$scratch/errors"

# Runs gh api; a permission error ends the step with a notice, any other
# error with a warning.
gh_api() {
  if gh api "$@" 2>"$errors"; then
    return 0
  fi
  if grep -qiE 'HTTP 403|Resource not accessible|must have (admin|push|write)' "$errors"; then
    notice "the token cannot comment on this pull request (permission denied), so no comment. Grant pull-requests: write to post one."
  fi
  echo "::warning title=aver comment::gh api $1 failed: $(tr '\n' ' ' <"$errors")" >&3
  exit 0
}

gh_api --paginate "repos/$repo/issues/$number/comments?per_page=100" >"$scratch/comments"
# Only a bot's comment counts: anyone can post a comment that starts with the
# marker, and updating theirs would let them edit the verdict afterwards.
existing=$(jq -r --arg marker "$marker" \
  '.[] | select(.user.type == "Bot" and ((.body // "") | startswith($marker))) | .id' \
  "$scratch/comments" | head -n 1)

{
  echo "$marker"
  cat "$out_dir/summary.md"
} | jq -Rs --argjson max "$max_chars" '
  "\n\n_Summary truncated to fit a GitHub comment. The aver-receipt artifact has the full receipt._\n" as $note
  | {body: (if length > $max then .[:($max - ($note | length))] + $note else . end)}' >"$scratch/body.json"

if [ -n "$existing" ]; then
  gh_api -X PATCH "repos/$repo/issues/comments/$existing" --input - <"$scratch/body.json" >/dev/null
  echo "Updated comment $existing on #$number"
else
  gh_api -X POST "repos/$repo/issues/$number/comments" --input - <"$scratch/body.json" >/dev/null
  echo "Posted a comment on #$number"
fi
