#!/usr/bin/env bash
# Writes the base record cache keys (`aver ci keys`) to the step's outputs.
# No keys means no record restored or saved, never a failed job.
set -euo pipefail

output=${GITHUB_OUTPUT:-/dev/null}
if keys=$(aver ci keys --repo "${WORKING_DIRECTORY:-.}" --base "${BASE_REV:?BASE_REV is not set}" --head "${HEAD_REV:?HEAD_REV is not set}"); then
  printf '%s\n' "$keys" >>"$output"
  printf '%s\n' "$keys"
else
  echo "::warning title=aver ci keys failed::the base record is neither restored nor saved in this run"
fi
