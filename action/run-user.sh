#!/usr/bin/env bash
# Creates the user native checks run as (uid separation, so the code under
# test cannot read the verifier's key), when this runner has passwordless
# sudo; outputs user=<name>. Without it, outputs nothing and warns: checks
# then run as the runner's user and the receipt records uid "same".
#
# The checks run in the checkout (WORKING_DIRECTORY) with the toolchains the
# job installed, and on GitHub's images both sit in the runner's home, which
# other users cannot enter (0750). Below such a closed directory the user
# gets search access (ACLs) to each directory on the way to the checkout,
# rustup's and cargo's homes, uv's Python installs and the tool cache, and is
# denied every other entry there, so what the closed directory kept from
# other users stays out of its reach: the runner's credentials
# (~/actions-runner) and $RUNNER_TEMP (the steps' scripts, secrets they name
# included, and earlier steps' outputs). A home other users can already
# enter is left as it is. The denies go first, and when one fails nothing is
# opened.
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

home=$(cd "${HOME:-/nonexistent}" 2>/dev/null && pwd -P) || home=""
wanted=()
if [ -n "$home" ]; then
  for dir in "${WORKING_DIRECTORY:-.}" "${RUSTUP_HOME:-$home/.rustup}" "${CARGO_HOME:-$home/.cargo}" \
    "${UV_PYTHON_INSTALL_DIR:-${XDG_DATA_HOME:-$home/.local/share}/uv/python}" \
    "${RUNNER_TOOL_CACHE:-}" "${AGENT_TOOLSDIRECTORY:-}"; do
    [ -n "$dir" ] || continue
    dir=$(cd "$dir" 2>/dev/null && pwd -P) || continue
    case $dir in "$home"/*) wanted+=("$dir") ;; esac
  done
fi
# One inside another (a CARGO_HOME in the checkout) is reached through it:
# walking to it would deny the rest of the outer one, .git and
# node_modules included.
opened=()
for dir in ${wanted[@]+"${wanted[@]}"}; do
  for outer in "${wanted[@]}"; do
    case $dir in "$outer"/*) continue 2 ;; esac
  done
  opened+=("$dir")
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

# Whether other users cannot search $1 or a directory between it and the
# home.
closed() {
  local dir=$1
  while [ -n "$(find "$dir" -maxdepth 0 -perm -0001)" ]; do
    [ "$dir" != "$home" ] || return 1
    dir=$(dirname "$dir")
  done
}

grants=()
denies=()
shopt -s dotglob nullglob
while IFS= read -r dir; do
  [ -n "$dir" ] && closed "$dir" || continue
  grants+=("$dir")
  for entry in "$dir"/*; do
    [ -L "$entry" ] || leads_to_opened "$entry" || denies+=("$entry")
  done
done <<<"$above"

if [ ${#grants[@]} -gt 0 ] && ! command -v setfacl >/dev/null 2>&1; then
  echo "::warning title=aver::setfacl is not installed (the acl package), so $user cannot reach the checkout or the toolchains under $home, and checks would fail to start"
elif [ ${#grants[@]} -gt 0 ]; then
  # A deny an earlier job on this runner left on what this one opens.
  for dir in "${opened[@]}"; do
    privileged sudo -n setfacl -x "u:$user" "$dir" 2>/dev/null || true
  done
  failed=""
  for entry in ${denies[@]+"${denies[@]}"}; do
    privileged sudo -n setfacl -m "u:$user:---" "$entry" || [ ! -e "$entry" ] || failed=$entry
  done
  if [ -n "$failed" ]; then
    echo "::warning title=aver::could not deny $user $failed, so it gets no way into $home: checks there would fail to start"
  else
    for dir in "${grants[@]}"; do
      privileged sudo -n setfacl -m "u:$user:--x" "$dir" ||
        echo "::warning title=aver::could not let $user through $dir: checks there may fail to start"
    done
  fi
fi
echo "user=$user" >>"$output"
