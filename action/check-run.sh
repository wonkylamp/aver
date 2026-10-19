#!/usr/bin/env bash
# Posts the check run named Aver (with review: approve or mode: approve) on
# the head commit, so branch protection can require it: success, failure or
# action_required for a verdict a person has to approve, success again once
# aver ci approve binds an approval to that commit. With the check input the
# name is "Aver (<check>)", one per matrix job. Runs after aver, in its own
# step: the token never reaches the step that runs the pull request's code.
set -euo pipefail

conclusion=${CONCLUSION:?CONCLUSION is not set}
name=Aver
if [ -n "${CHECK:-}" ]; then
  name="Aver ($CHECK)"
fi
verdict=${VERDICT:-}
case "$conclusion" in
  success)
    if [ "${APPROVED:-}" = true ]; then
      title="Aver: approved by ${REVIEWER:-a reviewer}"
      why="A person approved this head commit after reading what Aver could not prove."
    else
      title="Aver: ${verdict:-PASS}"
      why="The verdict is acceptable under the workflow's fail-on. The job summary has the receipt."
    fi ;;
  action_required)
    title="Aver: needs maintainer review (${verdict:-REVIEW})"
    why="Nothing broke, but nothing proved the change safe either. A maintainer approves the pull request on this commit to clear this check; a push after that needs a new approval. The job summary has the details." ;;
  failure)
    title="Aver: ${verdict:-error}"
    why="The verdict fails the workflow's fail-on, or aver ci failed. The job summary has the details." ;;
  *)
    echo "::error title=aver::unknown check run conclusion '$conclusion'"
    exit 2 ;;
esac
run_url="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}/actions/runs/${GITHUB_RUN_ID:-0}"
if ! gh api "repos/$GITHUB_REPOSITORY/check-runs" \
  -f name="$name" -f head_sha="${HEAD_SHA:?HEAD_SHA is not set}" \
  -f status=completed -f conclusion="$conclusion" -f details_url="$run_url" \
  -f "output[title]=$title" -f "output[summary]=$why" >/dev/null; then
  echo "::warning title=aver check run::could not post the $name check run (the job needs permissions: checks: write; a fork's pull request gets a read-only token)"
fi
