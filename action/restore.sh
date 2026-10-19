#!/usr/bin/env bash
# Puts the checkout back at the job's commit after an aver ci that was
# killed while its checks ran in it. A no-op when nothing was left swapped.
set -euo pipefail

repo=${WORKING_DIRECTORY:-.}
command -v aver >/dev/null || exit 0
# Files a killed check wrote as the run user stay that user's.
if [ -n "${RUN_AS:-}" ]; then
  sudo -n chown -R "$(id -u):$(id -g)" "$repo" 2>/dev/null || true
fi
aver ci restore --repo "$repo"
