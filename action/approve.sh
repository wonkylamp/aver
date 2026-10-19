#!/usr/bin/env bash
# mode: approve. Runs aver ci approve on a pull_request_review event: no
# check runs, so this step may hold the token aver reads the reviews with.
# An approval that binds to the pull request's current head commit is
# written to approval.json and reported in the head output for the check
# run step; none binding keeps the job green with a notice saying why.
set -euo pipefail

output=${GITHUB_OUTPUT:-/dev/null}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
out_dir=${OUT_DIR:?OUT_DIR is not set}

case "${GITHUB_EVENT_NAME:-}" in
  pull_request_review) ;;
  *)
    echo "::error title=aver::mode: approve runs on pull_request_review events, not ${GITHUB_EVENT_NAME:-an unnamed event}"
    exit 2 ;;
esac

rm -rf -- "$out_dir"
set +e
result=$(aver ci approve --github-event "${GITHUB_EVENT_PATH:?GITHUB_EVENT_PATH is not set}" \
  --repo "${WORKING_DIRECTORY:-.}" --out-dir "$out_dir")
code=$?
set -e
printf '%s\n' "$result"

case "$code" in
  0)
    head=$(jq -r '.head_commit' "$out_dir/approval.json")
    reviewer=$(jq -r '.reviewer' "$out_dir/approval.json")
    printf '## Aver: approved\n\n%s approved %s; the Aver check run on that commit is now successful.\n' "$reviewer" "$head" >>"$summary"
    printf 'approved=true\nhead=%s\nreviewer=%s\n' "$head" "$reviewer" >>"$output"
    ;;
  3)
    why=${result#NOT APPROVED: }
    echo "::notice title=Aver: not approved::$why"
    printf '## Aver: not approved\n\n%s\n' "$why" >>"$summary"
    printf 'approved=false\nhead=\nreviewer=\n' >>"$output"
    ;;
  *)
    echo "::error title=aver ci approve failed::aver ci approve exited with code $code"
    printf 'approved=false\nhead=\nreviewer=\n' >>"$output"
    exit "$code"
    ;;
esac
