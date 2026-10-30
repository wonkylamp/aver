#!/usr/bin/env bash
# Runs aver ci and records its result. The step itself always succeeds; the
# action's last step exits with aver's code, after the comment and the
# artifact upload have had their chance.
set -euo pipefail

output=${GITHUB_OUTPUT:-/dev/null}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
out_dir=${OUT_DIR:?OUT_DIR is not set}

args=(ci
  --repo "${WORKING_DIRECTORY:-.}"
  --base "${BASE_REV:?BASE_REV is not set}"
  --head "${HEAD_REV:?HEAD_REV is not set}"
  --out-dir "$out_dir")
# A merge queue holds a change only for what it broke: the pull request's
# own check already asked for its criteria and any review.
if [ "${GITHUB_EVENT_NAME:-}" = merge_group ]; then
  args+=(--regression-only)
  FAIL_ON=regressions
else
  args+=(--fail-on "${FAIL_ON:-unexplained}")
fi
if [ -n "${GITHUB_EVENT_PATH:-}" ] && [ -f "$GITHUB_EVENT_PATH" ]; then
  args+=(--github-event "$GITHUB_EVENT_PATH")
fi

# Native checks run the pull request's code on this runner, so nothing it
# can read may be a credential: no token persisted by actions/checkout, no
# OIDC token, and the verifier key either out of reach of a separate user
# (--run-as) or in a throwaway home.
refuse() {
  echo "::error title=aver ci refused::$1"
  printf '## Aver error\n\n%s\n' "$1" >>"$summary"
  printf 'exit-code=2\nverdict=\nreceipt-id=\nconclusion=failure\n' >>"$output"
  exit 0
}
# The commits are fetched by now (resolve-revs.sh), so the token
# actions/checkout persisted for that has done its job.
repo_dir=${WORKING_DIRECTORY:-.}
for key in $(git -C "$repo_dir" config --local --no-includes --name-only --get-regexp '^http\..*\.extraheader$' 2>/dev/null || true); do
  git -C "$repo_dir" config --local --unset-all "$key"
  echo "aver: removed $key from .git/config before native checks run (persist-credentials: false avoids writing it)"
done
# With submodules, actions/checkout writes the header into every
# submodule's own configuration under the common Git directory too
# (.git/modules/<name>/config), which the repository's configuration
# never reads. Since v6 it writes it into a file under $RUNNER_TEMP that
# the configurations include (includeIf.gitdir:…), outside the checkout.
common=$(cd "$repo_dir" 2>/dev/null && cd "$(git rev-parse --git-common-dir 2>/dev/null)" && pwd) || common=""
configs=()
if [ -n "$common" ]; then
  while IFS= read -r -d '' config; do configs+=("$config"); done \
    < <(find "$common" -type f \( -name config -o -name config.worktree \) -print0)
fi
for config in ${configs[@]+"${configs[@]}"}; do
  for key in $(git config --file "$config" --no-includes --name-only --get-regexp '^http\..*\.extraheader$' 2>/dev/null || true); do
    git config --file "$config" --unset-all "$key"
    echo "aver: removed $key from ${config#"$common"/} before native checks run (persist-credentials: false avoids writing it)"
  done
  for included in $(git config --file "$config" --no-includes --get-regexp '^include(if\..*)?\.path$' 2>/dev/null | awk '{print $2}' || true); do
    case $included in /*) ;; *) included=$(dirname "$config")/$included ;; esac
    [ -f "$included" ] || continue
    for key in $(git config --file "$included" --no-includes --name-only --get-regexp '^http\..*\.extraheader$' 2>/dev/null || true); do
      git config --file "$included" --unset-all "$key"
      echo "aver: removed $key from $included, which ${config#"$common"/} includes, before native checks run (persist-credentials: false avoids writing it)"
    done
  done
done
if git -C "$repo_dir" config --get-regexp '^http\..*\.extraheader$' >/dev/null 2>&1; then
  refuse "native checks would run next to a Git credential header in a Git configuration other than the repository's own; remove it."
fi
for config in ${configs[@]+"${configs[@]}"}; do
  if grep -qi 'extraheader' "$config"; then
    refuse "native checks would run next to a Git credential header in ${config#"$common"/}; set persist-credentials: false on actions/checkout."
  fi
  for included in $(git config --file "$config" --no-includes --get-regexp '^include(if\..*)?\.path$' 2>/dev/null | awk '{print $2}' || true); do
    case $included in /*) ;; *) included=$(dirname "$config")/$included ;; esac
    if [ -f "$included" ] && grep -qi 'extraheader' "$included"; then
      refuse "native checks would run next to a Git credential header in $included, which ${config#"$common"/} includes; set persist-credentials: false on actions/checkout."
    fi
  done
done
if [ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ]; then
  refuse "native checks would run with an OIDC token request URL (the job has id-token: write); drop that permission from this job."
fi
if [ -n "${RUN_AS:-}" ]; then
  args+=(--run-as "$RUN_AS")
  # Under --run-as a check gets a HOME of its own, where rustup would find
  # no toolchain and cargo none of the crates the job fetched; run-user.sh
  # let the run user reach these.
  [ -n "${RUSTUP_HOME:-}" ] || [ ! -d "$HOME/.rustup" ] || export RUSTUP_HOME=$HOME/.rustup
  [ -n "${CARGO_HOME:-}" ] || [ ! -d "$HOME/.cargo" ] || export CARGO_HOME=$HOME/.cargo
  # The rest of the runner's home stays closed to the run user, so XDG
  # directories in it (GitHub's images set XDG_CONFIG_HOME=~/.config, where
  # pnpm and Yarn stop at EACCES) follow the check's own HOME instead.
  for var in XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME XDG_STATE_HOME; do
    case ${!var:-} in "$HOME" | "$HOME"/*) unset "$var" ;; esac
  done
fi
if [ -n "${CI_HOME:-}" ] && [ -n "${RUN_AS:-}" ]; then
  args+=(--home "$CI_HOME")
fi
if [ -n "${VIEWER_URL:-}" ]; then
  args+=(--viewer-url "$VIEWER_URL")
fi
if [ -n "${CHECK:-}" ]; then
  args+=(--check "$CHECK")
fi
# Base records: BASE_RECORD only on an exact cache hit; RECORD on a push to
# the default branch or a merge queue run.
if [ -n "${BASE_RECORD:-}" ]; then
  args+=(--base-record "$BASE_RECORD")
fi
if [ -n "${RECORD:-}" ]; then
  rm -rf -- "$RECORD"
  args+=(--record "$RECORD")
fi

# A stale summary.md from an earlier run would be reported as this run's.
rm -rf -- "$out_dir"

set +e
aver "${args[@]}"
code=$?
set -e

if [ -f "$out_dir/summary.md" ]; then
  cat "$out_dir/summary.md" >>"$summary"
  echo >>"$summary"
else
  printf '## Aver error\n\n`aver ci` exited with code %s before writing a summary. The step log has the error.\n' "$code" >>"$summary"
fi

verdict=""
receipt_id=""
if [ -f "$out_dir/receipt.json" ]; then
  verdict=$(jq -r '.receipt.verdict // empty' "$out_dir/receipt.json" 2>/dev/null || true)
  receipt_id=$(jq -r '.receipt_id // empty' "$out_dir/receipt.json" 2>/dev/null || true)
fi

# Exit 3: REVIEW or UNPROVEN under --fail-on unexplained. Nothing broke, but
# nothing proved the change safe either, so by default the required check
# fails until a maintainer looks; review: neutral keeps the job green and a
# later step posts a neutral check run instead.
needs_review=false
case "$code" in
  0) echo "aver ci: ${verdict:-no verdict}" ;;
  1) echo "::error title=aver verdict ${verdict:-unknown}::the verdict fails --fail-on ${FAIL_ON:-unexplained}" ;;
  3)
    needs_review=true
    title="Aver: needs maintainer review (${verdict:-REVIEW})"
    case "$verdict" in
      UNPROVEN) why="Nothing broke, but nothing proved all of the change: a criterion is unproven, or a changed source file is covered by no check." ;;
      *) why="Nothing broke, but the change rewrites test expectations or test harness, which a person has to approve." ;;
    esac
    printf '\n> **%s.** %s\n' "$title" "$why" >>"$summary"
    if [ "${REVIEW:-fail}" = neutral ]; then
      echo "::notice title=$title::$why (review: neutral keeps the job green)"
      code=0
    elif [ "${REVIEW:-fail}" = approve ]; then
      echo "::notice title=$title::$why (review: approve keeps the job green; the Aver check run waits for a maintainer's approval of this commit)"
      code=0
    else
      echo "::error title=$title::$why"
    fi
    ;;
  *) echo "::error title=aver ci failed::aver ci exited with code $code" ;;
esac

# The Aver check run's conclusion, for review: approve.
case "$code:$needs_review" in
  *:true) conclusion=action_required ;;
  0:*) conclusion=success ;;
  *) conclusion=failure ;;
esac

printf 'exit-code=%s\nverdict=%s\nreceipt-id=%s\nneeds-review=%s\nconclusion=%s\n' "$code" "$verdict" "$receipt_id" "$needs_review" "$conclusion" >>"$output"

recorded=false
if [ -n "${RECORD:-}" ] && [ -f "$RECORD/record.json" ]; then
  recorded=true
fi
printf 'recorded=%s\n' "$recorded" >>"$output"
