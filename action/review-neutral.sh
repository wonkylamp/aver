#!/usr/bin/env bash
# With review: neutral, reports aver ci's exit 3 (REVIEW or UNPROVEN) as a
# neutral check run on the head commit, so the pull request shows it while
# the job stays green. Runs after aver, in its own step: the token never
# reaches the step that runs the pull request's code.
set -euo pipefail

verdict=${VERDICT:-REVIEW}
title="Aver: needs maintainer review ($verdict)"
case "$verdict" in
  UNPROVEN) why="Nothing broke, but nothing proved all of the change: a criterion is unproven, or a changed source file is covered by no check. The job summary has the details." ;;
  *) why="Nothing broke, but the change rewrites test expectations or test harness, which a person has to approve. The job summary shows each change." ;;
esac
if ! gh api "repos/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}/check-runs" \
  -f name=aver-review -f head_sha="${HEAD_SHA:?HEAD_SHA is not set}" \
  -f status=completed -f conclusion=neutral \
  -f "output[title]=$title" -f "output[summary]=$why" >/dev/null; then
  echo "::warning title=aver review::could not post the neutral check run (the job needs permissions: checks: write)"
fi
