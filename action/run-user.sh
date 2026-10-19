#!/usr/bin/env bash
# Creates the user native checks run as (uid separation, so the code under
# test cannot read the verifier's key), when this runner has passwordless
# sudo; outputs user=<name>. Without it, outputs nothing and warns: checks
# then run as the runner's user and the receipt records uid "same".
#
# AVER_DRY_RUN=1 prints each privileged command instead of running it.
set -euo pipefail

user=${AVER_RUN_USER:-aver-run}
dry=${AVER_DRY_RUN:-0}
output=${GITHUB_OUTPUT:-/dev/null}

privileged() {
  if [ "$dry" = 1 ]; then
    echo "+ $*"
  else
    "$@"
  fi
}

if [ "$(uname)" != Linux ] || ! sudo -n true 2>/dev/null; then
  echo "::warning title=aver::this runner has no passwordless sudo, so native checks run as the runner's own user (the receipt records uid \"same\")"
  exit 0
fi
if ! id "$user" >/dev/null 2>&1; then
  privileged sudo -n useradd --system --no-create-home --shell /usr/sbin/nologin "$user"
fi
echo "user=$user" >>"$output"
