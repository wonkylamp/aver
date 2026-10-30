#!/usr/bin/env bash
# Creates the user native checks run as (uid separation, so the code under
# test cannot read the verifier's key), when this runner has passwordless
# sudo; outputs user=<name>. Without it, outputs nothing and warns: checks
# then run as the runner's user and the receipt records uid "same".
#
# The checks run in the checkout (WORKING_DIRECTORY) with the toolchains the
# job installed, and on GitHub's images both sit in the runner's home, which
# other users cannot enter (0750). The user gets through each directory
# above the checkout, rustup's and cargo's homes and uv's Python installs
# (ACLs, search only), and is denied every other entry of those
# directories: the runner's own credentials (~/actions-runner) and
# $RUNNER_TEMP (the steps' scripts, secrets they name included, and earlier
# steps' outputs) stay out of its reach.
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

home=$(cd "$HOME" && pwd -P)
opened=()
for dir in "${WORKING_DIRECTORY:-.}" "${RUSTUP_HOME:-$HOME/.rustup}" "${CARGO_HOME:-$HOME/.cargo}" \
  "${UV_PYTHON_INSTALL_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/uv/python}"; do
  dir=$(cd "$dir" 2>/dev/null && pwd -P) || continue
  case $dir in "$home"/*) opened+=("$dir") ;; esac
done
above=$(for dir in ${opened[@]+"${opened[@]}"}; do
  while [ "$dir" != "$home" ]; do
    dir=$(dirname "$dir")
    echo "$dir"
  done
done | sort -u)

leads_to_opened() {
  local dir
  for dir in "${opened[@]}"; do
    case $dir in "$1" | "$1"/*) return 0 ;; esac
  done
  return 1
}

if [ -n "$above" ] && ! command -v setfacl >/dev/null 2>&1; then
  echo "::warning title=aver::setfacl is not installed (the acl package), so $user may not reach the checkout or the toolchains under $home, and checks would fail to start"
elif [ -n "$above" ]; then
  shopt -s dotglob nullglob
  while IFS= read -r dir; do
    privileged sudo -n setfacl -m "u:$user:--x" "$dir"
    for entry in "$dir"/*; do
      [ -L "$entry" ] || leads_to_opened "$entry" || privileged sudo -n setfacl -m "u:$user:---" "$entry"
    done
  done <<<"$above"
fi
echo "user=$user" >>"$output"
