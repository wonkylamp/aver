#!/usr/bin/env bash
# Runs the action's scripts against simulated GitHub environments: temporary
# Git repositories, fake event payloads and output files, and stub `gh` and
# `aver` commands on PATH. Prints one line per test; exits 1 if any fails.
set -euo pipefail

# Xcode's /usr/bin/python3 lacks PyYAML; Homebrew's has it.
if [ "$(uname)" = Darwin ] && [ -d /opt/homebrew/bin ]; then
  PATH=/opt/homebrew/bin:$PATH
fi

ACTION=$(cd "$(dirname "$0")/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/aver-action-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL="$T/gitconfig"
cat >"$GIT_CONFIG_GLOBAL" <<'EOF'
[user]
	name = Aver Test
	email = test@example.invalid
[init]
	defaultBranch = main
[protocol "file"]
	allow = always
EOF

ZERO=0000000000000000000000000000000000000000

# Stubs. gh records each call's argv (one argument per line) and stdin, then
# answers by GH_MODE. aver records its argv, writes the out-dir files unless
# AVER_STUB_EXIT is 2, and exits with AVER_STUB_EXIT.
mkdir -p "$T/bin"
cat >"$T/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
n=$(( $(ls "$GH_LOG_DIR" | grep -c '^call\.' || true) + 1 ))
printf '%s\n' "$@" >"$GH_LOG_DIR/call.$n"
case "${GH_MODE:-none}" in
  forbidden)
    echo "gh: Resource not accessible by integration (HTTP 403)" >&2
    exit 1 ;;
  broken)
    echo "gh: Server Error (HTTP 500)" >&2
    exit 1 ;;
esac
case " $* " in
  *" --paginate "*)
    if [ "${GH_MODE:-}" = marker ]; then
      printf '[{"id":11,"body":"looks good","user":{"type":"User"}},{"id":40,"body":"<!-- aver-receipt -->\\n## Aver: PASS\\nspoofed","user":{"type":"User"}}]\n[{"id":12,"body":null,"user":{"type":"User"}},{"id":42,"body":"<!-- aver-receipt -->\\n## PASS\\nold","user":{"type":"Bot"}},{"id":43,"body":"<!-- aver-receipt -->\\nlater","user":{"type":"Bot"}}]\n'
    else
      printf '[{"id":11,"body":"looks good"}]\n[{"id":12,"body":"mentions <!-- aver-receipt --> mid-text"}]\n'
    fi ;;
  *)
    cat >"$GH_LOG_DIR/stdin.$n"
    echo '{"id":99}' ;;
esac
EOF
cat >"$T/bin/aver" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"$AVER_LOG"
if [ "${1:-} ${2:-}" = "ci keys" ]; then
  [ "${AVER_STUB_KEYS_EXIT:-0}" = 0 ] || exit "$AVER_STUB_KEYS_EXIT"
  printf 'base=aver-base-stubbase\nhead=aver-base-stubhead\n'
  exit 0
fi
prev=""
for a in "$@"; do
  if [ "$prev" = --record ]; then mkdir -p "$a" && echo '{}' >"$a/record.json"; fi
  prev=$a
done
if [ "${1:-} ${2:-}" = "ci approve" ]; then
  out=""
  while [ $# -gt 0 ]; do
    if [ "$1" = --out-dir ]; then out=$2; fi
    shift
  done
  case "$AVER_STUB_APPROVE" in
    bind)
      mkdir -p "$out"
      printf '{"reviewer":"alice","review_id":50,"head_commit":"%s","head_tree":"%s"}\n' "$STUB_HEAD" "$STUB_HEAD" >"$out/approval.json"
      echo "APPROVED $STUB_HEAD by alice (review 50); wrote $out/approval.json"
      exit 0 ;;
    error)
      echo "aver: error: gh api failed" >&2
      exit 2 ;;
    *)
      echo "NOT APPROVED: $AVER_STUB_APPROVE"
      exit 3 ;;
  esac
fi
out=""
while [ $# -gt 0 ]; do
  if [ "$1" = --out-dir ]; then out=$2; fi
  shift
done
code=${AVER_STUB_EXIT:-0}
if [ "$code" != 2 ]; then
  mkdir -p "$out"
  printf '{"receipt_id":"rcpt_stub","receipt":{"verdict":"%s"}}\n' "$AVER_STUB_VERDICT" >"$out/receipt.json"
  printf '## %s\n\nstub summary\n' "$AVER_STUB_VERDICT" >"$out/summary.md"
  printf 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n' >"$out/public-key.txt"
else
  echo "aver: error: stub failure" >&2
fi
exit "$code"
EOF
chmod +x "$T/bin/gh" "$T/bin/aver"

die() {
  echo "assertion failed: $*" >&2
  exit 1
}
contains() { grep -qF -- "$2" "$1" || die "$1 does not contain '$2'"; }
lacks() { ! grep -qF -- "$2" "$1" || die "$1 contains '$2'"; }
equals() { [ "$1" = "$2" ] || die "expected '$2', got '$1'"; }
out() { grep "^$1=" "$W/output" | tail -n 1 | cut -d= -f2-; }
gh_calls() { ls "$W/gh" | grep -c '^call\.' || true; }

# Gives the current test a fresh work dir with GitHub's files and the stubs.
fresh() {
  W=$(mktemp -d "$T/w.XXXXXX")
  mkdir -p "$W/gh"
  : >"$W/output"
  : >"$W/summary"
  export GITHUB_OUTPUT="$W/output" GITHUB_STEP_SUMMARY="$W/summary"
  export GH_LOG_DIR="$W/gh" AVER_LOG="$W/aver.argv"
  export PATH="$T/bin:$PATH"
  export RUNNER_TEMP="$W"
  unset INPUT_BASE INPUT_HEAD GITHUB_EVENT_NAME GITHUB_EVENT_PATH GH_MODE AVER_STUB_APPROVE
}

make_repo() {
  git init -q "$1"
  local i
  for i in 1 2 3; do
    echo "$i" >"$1/file"
    git -C "$1" add file
    git -C "$1" commit -qm "commit $i"
  done
}

pr_event() { # <head repo> <base repo>
  cat >"$W/event.json" <<EOF
{"pull_request":{"number":7,"body":null,
  "base":{"sha":"${BASE_SHA:-1111111111111111111111111111111111111111}","repo":{"full_name":"$2"}},
  "head":{"sha":"${HEAD_SHA:-2222222222222222222222222222222222222222}","repo":{"full_name":"$1"}}}}
EOF
  export GITHUB_EVENT_NAME=pull_request GITHUB_EVENT_PATH="$W/event.json"
}

comment_setup() {
  fresh
  pr_event o/r o/r
  export GITHUB_REPOSITORY=o/r OUT_DIR="$W/out"
  mkdir -p "$OUT_DIR"
  printf '## PASS\n\nall good\n' >"$OUT_DIR/summary.md"
}

# Runs the action's last step, extracted from action.yml, with AVER_EXIT
# evaluated from its `steps.run.outputs.exit-code || '<default>'` mapping,
# given the run step's exit-code output (empty when the run step was skipped).
final_step() {
  python3 - "$ACTION/action.yml" "$1" "$W/final.sh" >"$W/final.env" <<'EOF'
import re, sys, yaml
step = yaml.safe_load(open(sys.argv[1]))["runs"]["steps"][-1]
expr = step["env"]["AVER_EXIT"]
m = re.fullmatch(r"\$\{\{\s*steps\.run\.outputs\.exit-code(?:\s*\|\|\s*'([^']*)')?\s*\}\}", expr)
assert m, "unexpected AVER_EXIT mapping: " + expr
print(sys.argv[2] or (m.group(1) or ""))
open(sys.argv[3], "w").write(step["run"])
EOF
  AVER_EXIT=$(cat "$W/final.env") bash "$W/final.sh"
}

test_comment_create() {
  comment_setup
  export GH_MODE=none
  bash "$ACTION/comment.sh"
  equals "$(gh_calls)" 2
  contains "$W/gh/call.1" "--paginate"
  contains "$W/gh/call.1" "repos/o/r/issues/7/comments?per_page=100"
  contains "$W/gh/call.2" "POST"
  contains "$W/gh/call.2" "repos/o/r/issues/7/comments"
  lacks "$W/gh/call.2" "PATCH"
  equals "$(jq -r '.body' "$W/gh/stdin.2" | head -n 1)" "<!-- aver-receipt -->"
  jq -r '.body' "$W/gh/stdin.2" >"$W/body"
  contains "$W/body" "all good"
}

test_comment_update() {
  comment_setup
  export GH_MODE=marker
  bash "$ACTION/comment.sh"
  equals "$(gh_calls)" 2
  contains "$W/gh/call.2" "PATCH"
  contains "$W/gh/call.2" "repos/o/r/issues/comments/42"
  lacks "$W/gh/call.2" "POST"
  jq -r '.body' "$W/gh/stdin.2" >"$W/body"
  contains "$W/body" "all good"
}

test_comment_fork_skip() {
  comment_setup
  pr_event someone/r o/r
  export GH_MODE=none
  bash "$ACTION/comment.sh" >"$W/log"
  equals "$(gh_calls)" 0
  contains "$W/log" "::notice"
  contains "$W/log" "fork"
}

test_comment_forbidden_skip() {
  comment_setup
  export GH_MODE=forbidden
  bash "$ACTION/comment.sh" >"$W/log"
  contains "$W/log" "::notice"
  contains "$W/log" "permission"
  lacks "$W/log" "Posted"
}

test_comment_other_error_warns() {
  comment_setup
  export GH_MODE=broken
  bash "$ACTION/comment.sh" >"$W/log"
  contains "$W/log" "::warning"
  contains "$W/log" "HTTP 500"
}

test_comment_no_summary() {
  comment_setup
  rm "$OUT_DIR/summary.md"
  export GH_MODE=none
  bash "$ACTION/comment.sh" >"$W/log"
  equals "$(gh_calls)" 0
  contains "$W/log" "no summary.md"
}

test_comment_truncates() {
  comment_setup
  export GH_MODE=none
  python3 -c 'print("## PASS\n" + "x" * 70000 + "\nTAIL")' >"$OUT_DIR/summary.md"
  bash "$ACTION/comment.sh"
  local length
  length=$(jq -r '.body | length' "$W/gh/stdin.2")
  [ "$length" -le 65536 ] || die "body has $length characters"
  jq -r '.body' "$W/gh/stdin.2" >"$W/body"
  contains "$W/body" "Summary truncated"
  lacks "$W/body" "TAIL"
}

test_revs_pull_request() {
  fresh
  make_repo "$W/repo"
  BASE_SHA=$(git -C "$W/repo" rev-parse HEAD~2) HEAD_SHA=$(git -C "$W/repo" rev-parse HEAD) pr_event o/r o/r
  WORKING_DIRECTORY="$W/repo" bash "$ACTION/resolve-revs.sh"
  equals "$(out base)" "$(git -C "$W/repo" rev-parse HEAD~2)"
  equals "$(out head)" "$(git -C "$W/repo" rev-parse HEAD)"
  equals "$(out skip)" false
}

test_revs_push() {
  fresh
  make_repo "$W/repo"
  local before after
  before=$(git -C "$W/repo" rev-parse HEAD~1)
  after=$(git -C "$W/repo" rev-parse HEAD)
  printf '{"before":"%s","after":"%s"}' "$before" "$after" >"$W/event.json"
  GITHUB_EVENT_NAME=push GITHUB_EVENT_PATH="$W/event.json" WORKING_DIRECTORY="$W/repo" bash "$ACTION/resolve-revs.sh"
  equals "$(out base)" "$before"
  equals "$(out head)" "$after"
  equals "$(out skip)" false
}

# A new branch's push compares with where it left the default branch.
test_revs_push_new_branch_compares_with_the_default_branch() {
  fresh
  make_repo "$W/origin"
  local fork head
  fork=$(git -C "$W/origin" rev-parse HEAD)
  git -C "$W/origin" checkout -q -b feature
  echo feature >"$W/origin/feature"
  git -C "$W/origin" add feature
  git -C "$W/origin" commit -qm "feature"
  head=$(git -C "$W/origin" rev-parse HEAD)
  git -C "$W/origin" checkout -q main
  echo 4 >"$W/origin/file"
  git -C "$W/origin" commit -qam "main moves on"
  git clone -q --depth=1 --branch feature "file://$W/origin" "$W/clone"
  printf '{"before":"%s","after":"%s","repository":{"default_branch":"main"}}' "$ZERO" "$head" >"$W/event.json"
  GITHUB_EVENT_NAME=push GITHUB_EVENT_PATH="$W/event.json" WORKING_DIRECTORY="$W/clone" bash "$ACTION/resolve-revs.sh" >"$W/log" 2>&1
  equals "$(out skip)" false
  equals "$(out base)" "$fork"
  equals "$(out head)" "$head"
  contains "$W/log" "comparing with its merge base with main"
}

test_revs_push_new_branch_skips_without_a_comparison() {
  fresh
  make_repo "$W/repo"
  local head
  head=$(git -C "$W/repo" rev-parse HEAD)
  printf '{"before":"%s","after":"%s"}' "$ZERO" "$head" >"$W/event.json"
  GITHUB_EVENT_NAME=push GITHUB_EVENT_PATH="$W/event.json" WORKING_DIRECTORY="$W/repo" bash "$ACTION/resolve-revs.sh" >"$W/log"
  equals "$(out skip)" true
  contains "$W/summary" "names no default branch"
  contains "$W/log" "::notice"

  fresh
  make_repo "$W/origin"
  git clone -q "file://$W/origin" "$W/clone"
  head=$(git -C "$W/clone" rev-parse HEAD)
  printf '{"before":"%s","after":"%s","repository":{"default_branch":"main"}}' "$ZERO" "$head" >"$W/event.json"
  GITHUB_EVENT_NAME=push GITHUB_EVENT_PATH="$W/event.json" WORKING_DIRECTORY="$W/clone" bash "$ACTION/resolve-revs.sh" >"$W/log"
  equals "$(out skip)" true
  contains "$W/summary" "no commits beyond main"
}

test_revs_merge_group() {
  fresh
  make_repo "$W/repo"
  local base head
  base=$(git -C "$W/repo" rev-parse HEAD~1)
  head=$(git -C "$W/repo" rev-parse HEAD)
  printf '{"merge_group":{"base_sha":"%s","head_sha":"%s","head_ref":"refs/heads/gh-readonly-queue/main/pr-7"}}' "$base" "$head" >"$W/event.json"
  GITHUB_EVENT_NAME=merge_group GITHUB_EVENT_PATH="$W/event.json" WORKING_DIRECTORY="$W/repo" bash "$ACTION/resolve-revs.sh"
  equals "$(out base)" "$base"
  equals "$(out head)" "$head"
  equals "$(out skip)" false
}

test_revs_shallow_fetch() {
  fresh
  make_repo "$W/origin"
  local base head
  base=$(git -C "$W/origin" rev-parse HEAD~2)
  head=$(git -C "$W/origin" rev-parse HEAD)
  git clone -q --depth=1 "file://$W/origin" "$W/clone"
  if git -C "$W/clone" cat-file -e "$base^{commit}" 2>/dev/null; then
    die "the shallow clone already has the base commit"
  fi
  printf '{"before":"%s","after":"%s"}' "$base" "$head" >"$W/event.json"
  GITHUB_EVENT_NAME=push GITHUB_EVENT_PATH="$W/event.json" WORKING_DIRECTORY="$W/clone" bash "$ACTION/resolve-revs.sh" >"$W/log" 2>&1
  contains "$W/log" "Fetching $base"
  git -C "$W/clone" cat-file -e "$base^{commit}" || die "the base commit was not fetched"
  equals "$(out base)" "$base"
}

test_revs_diverged_base_finds_the_merge_base() {
  fresh
  make_repo "$W/origin"
  local fork base head
  fork=$(git -C "$W/origin" rev-parse HEAD)
  git -C "$W/origin" checkout -q -b feature
  echo feature >"$W/origin/feature"
  git -C "$W/origin" add feature
  git -C "$W/origin" commit -qm "feature"
  head=$(git -C "$W/origin" rev-parse HEAD)
  git -C "$W/origin" checkout -q main
  echo 4 >"$W/origin/file"
  git -C "$W/origin" commit -qam "main moves on"
  base=$(git -C "$W/origin" rev-parse HEAD)
  git clone -q --depth=1 --branch feature "file://$W/origin" "$W/clone"
  BASE_SHA=$base HEAD_SHA=$head pr_event o/r o/r
  WORKING_DIRECTORY="$W/clone" bash "$ACTION/resolve-revs.sh" >"$W/log" 2>&1
  equals "$(out base)" "$base"
  equals "$(out head)" "$head"
  equals "$(out merge-base)" "$fork"
  contains "$W/log" "aver ci compares with their merge base $fork"
}

test_revs_missing_inputs() {
  fresh
  make_repo "$W/repo"
  echo '{}' >"$W/event.json"
  local rc=0
  GITHUB_EVENT_NAME=workflow_dispatch GITHUB_EVENT_PATH="$W/event.json" WORKING_DIRECTORY="$W/repo" \
    bash "$ACTION/resolve-revs.sh" >"$W/log" 2>&1 || rc=$?
  equals "$rc" 2
  contains "$W/log" "set the base and head inputs"
  rc=0
  INPUT_BASE=HEAD GITHUB_EVENT_NAME=workflow_dispatch GITHUB_EVENT_PATH="$W/event.json" WORKING_DIRECTORY="$W/repo" \
    bash "$ACTION/resolve-revs.sh" >"$W/log" 2>&1 || rc=$?
  equals "$rc" 2
  contains "$W/log" "set both the base and head inputs"
  INPUT_BASE=HEAD~1 INPUT_HEAD=HEAD GITHUB_EVENT_NAME=workflow_dispatch GITHUB_EVENT_PATH="$W/event.json" \
    WORKING_DIRECTORY="$W/repo" bash "$ACTION/resolve-revs.sh"
  equals "$(out base)" "HEAD~1"
}

test_revs_refuses_pull_request_target() {
  fresh
  make_repo "$W/repo"
  pr_event o/r o/r
  local rc=0
  GITHUB_EVENT_NAME=pull_request_target WORKING_DIRECTORY="$W/repo" bash "$ACTION/resolve-revs.sh" >"$W/log" 2>&1 || rc=$?
  equals "$rc" 2
  contains "$W/log" "pull_request_target"
}

run_with_exit() { # <aver exit code> <verdict> [fail-on]
  fresh
  pr_event o/r o/r
  export AVER_STUB_EXIT=$1 AVER_STUB_VERDICT=$2
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h FAIL_ON="${3:-fail-own}" bash "$ACTION/run.sh" >"$W/log"
  contains "$AVER_LOG" "${3:-fail-own}"
  equals "$(out exit-code)" "$1"
  local rc=0
  final_step "$(out exit-code)" || rc=$?
  equals "$rc" "$1"
}

test_run_exit_0() {
  run_with_exit 0 PASS
  equals "$(out verdict)" PASS
  equals "$(out receipt-id)" rcpt_stub
  contains "$W/summary" "## PASS"
}

test_run_exit_1() {
  run_with_exit 1 FAIL_OWN
  equals "$(out verdict)" FAIL_OWN
  contains "$W/summary" "## FAIL_OWN"
  contains "$W/log" "::error"
}

test_run_exit_2() {
  run_with_exit 2 ""
  equals "$(out verdict)" ""
  equals "$(out receipt-id)" ""
  contains "$W/summary" "exited with code 2 before writing a summary"
}

# aver ci exits 0 on REVIEW under fail-own and 1 under not-pass; the action
# passes the verdict through either way and fails only on the exit code.
test_run_review_under_fail_own_succeeds() {
  run_with_exit 0 REVIEW fail-own
  equals "$(out verdict)" REVIEW
  contains "$W/summary" "## REVIEW"
  lacks "$W/log" "::error"
}

test_run_review_under_not_pass_fails() {
  run_with_exit 1 REVIEW not-pass
  equals "$(out verdict)" REVIEW
  contains "$W/log" "::error title=aver verdict REVIEW::the verdict fails --fail-on not-pass"
}

# A merge queue fails only on what the change broke: aver ci runs with
# --regression-only (no criteria, --fail-on regressions), whatever fail-on
# says, and exits 0 on REVIEW.
test_run_merge_group_is_regression_only() {
  fresh
  printf '{"merge_group":{"base_sha":"b","head_sha":"h"}}' >"$W/event.json"
  export GITHUB_EVENT_NAME=merge_group GITHUB_EVENT_PATH="$W/event.json"
  export AVER_STUB_EXIT=0 AVER_STUB_VERDICT=REVIEW
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h FAIL_ON=unexplained bash "$ACTION/run.sh" >"$W/log"
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  contains "$W/argv" "--regression-only"
  lacks "$W/argv" "--fail-on"
  equals "$(out exit-code)" 0
  lacks "$W/log" "::error"
  export AVER_STUB_EXIT=1 AVER_STUB_VERDICT=FAIL_OWN
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h FAIL_ON=unexplained bash "$ACTION/run.sh" >"$W/log"
  equals "$(out exit-code)" 1
  contains "$W/log" "the verdict fails --fail-on regressions"
}

# Exit 3 (REVIEW or UNPROVEN under fail-on unexplained) fails the job by
# default, titled for the maintainer, with the reason in the summary.
test_run_exit_3_fails_by_default_crit_ci_trust_action() {
  for verdict in REVIEW UNPROVEN; do
    run_with_exit 3 "$verdict" unexplained
    equals "$(out verdict)" "$verdict"
    equals "$(out needs-review)" true
    contains "$W/log" "::error title=Aver: needs maintainer review ($verdict)::"
    contains "$W/summary" "**Aver: needs maintainer review ($verdict).** Nothing broke"
  done
}

# With review: neutral, exit 3 keeps the job green and the next step posts
# a neutral check run with the same title.
test_run_exit_3_neutral_crit_ci_trust_action() {
  fresh
  pr_event o/r o/r
  export AVER_STUB_EXIT=3 AVER_STUB_VERDICT=UNPROVEN
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h FAIL_ON=unexplained REVIEW=neutral \
    bash "$ACTION/run.sh" >"$W/log"
  equals "$(out exit-code)" 0
  equals "$(out needs-review)" true
  contains "$W/log" "::notice title=Aver: needs maintainer review (UNPROVEN)::"
  lacks "$W/log" "::error"
  final_step "$(out exit-code)"
  GITHUB_REPOSITORY=o/r HEAD_SHA=h VERDICT=UNPROVEN bash "$ACTION/review-neutral.sh" >"$W/log"
  equals "$(gh_calls)" 1
  contains "$W/gh/call.1" "repos/o/r/check-runs"
  contains "$W/gh/call.1" "conclusion=neutral"
  contains "$W/gh/call.1" "head_sha=h"
  contains "$W/gh/call.1" "output[title]=Aver: needs maintainer review (UNPROVEN)"
  GH_MODE=forbidden GITHUB_REPOSITORY=o/r HEAD_SHA=h VERDICT=REVIEW bash "$ACTION/review-neutral.sh" >"$W/log"
  contains "$W/log" "::warning title=aver review::could not post the neutral check run"
}

# review: approve keeps the job green on exit 3 and gives the Aver check
# run a conclusion for every verdict: success, failure, action_required.
test_review_approve_conclusions() {
  local case code verdict conclusion final
  for case in "0 PASS success 0" "1 FAIL_OWN failure 1" "3 REVIEW action_required 0" "3 UNPROVEN action_required 0" "2 - failure 2"; do
    read -r code verdict conclusion final <<<"$case"
    [ "$verdict" != - ] || verdict=""
    fresh
    pr_event o/r o/r
    export AVER_STUB_EXIT=$code AVER_STUB_VERDICT=$verdict
    OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h FAIL_ON=unexplained REVIEW=approve bash "$ACTION/run.sh" >"$W/log"
    equals "$(out conclusion)" "$conclusion"
    equals "$(out exit-code)" "$final"
    local rc=0
    final_step "$(out exit-code)" || rc=$?
    equals "$rc" "$final"
    GITHUB_REPOSITORY=o/r GITHUB_RUN_ID=5 HEAD_SHA=h CONCLUSION="$conclusion" VERDICT="$verdict" \
      bash "$ACTION/check-run.sh" >"$W/log"
    equals "$(gh_calls)" 1
    contains "$W/gh/call.1" "repos/o/r/check-runs"
    contains "$W/gh/call.1" "name=Aver"
    contains "$W/gh/call.1" "head_sha=h"
    contains "$W/gh/call.1" "conclusion=$conclusion"
    contains "$W/gh/call.1" "details_url=https://github.com/o/r/actions/runs/5"
  done
  fresh
  GH_MODE=forbidden GITHUB_REPOSITORY=o/r HEAD_SHA=h CONCLUSION=success CHECK=unit bash "$ACTION/check-run.sh" >"$W/log"
  contains "$W/gh/call.1" "name=Aver (unit)"
  contains "$W/log" "::warning title=aver check run::could not post the Aver (unit) check run"
}

approve_event() { # <event name>
  cat >"$W/event.json" <<'JSON'
{"review":{"id":50},"pull_request":{"number":7,"user":{"login":"author","type":"User"},"base":{"repo":{"full_name":"o/r"}}}}
JSON
  export GITHUB_EVENT_NAME=$1 GITHUB_EVENT_PATH="$W/event.json"
}

# mode: approve runs aver ci approve and nothing else: an approval that
# binds posts the Aver check run as success on the head it names.
test_approve_mode_binds_on_the_current_head() {
  fresh
  approve_event pull_request_review
  export AVER_STUB_APPROVE=bind STUB_HEAD=3333333333333333333333333333333333333333
  OUT_DIR="$W/out" WORKING_DIRECTORY=repo bash "$ACTION/approve.sh" >"$W/log"
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  contains "$W/argv" "ci approve --github-event $W/event.json --repo repo --out-dir $W/out"
  equals "$(out approved)" true
  equals "$(out head)" "$STUB_HEAD"
  equals "$(out reviewer)" alice
  contains "$W/summary" "## Aver: approved"
  GITHUB_REPOSITORY=o/r HEAD_SHA="$(out head)" CONCLUSION=success APPROVED=true REVIEWER=alice \
    bash "$ACTION/check-run.sh" >"$W/log"
  contains "$W/gh/call.1" "name=Aver"
  contains "$W/gh/call.1" "conclusion=success"
  contains "$W/gh/call.1" "head_sha=$STUB_HEAD"
  contains "$W/gh/call.1" "output[title]=Aver: approved by alice"
}

# An approval on an older commit (a push came after it), the author's own
# and a bot's bind nothing: the job stays green with a notice and the step
# outputs leave the check run step nothing to post.
test_approve_mode_refusals_post_nothing() {
  local why
  for why in "review 50 by alice approved 1111, not the current head 2222: a push after the review needs a new approval" \
    "review 50 is by author, the pull request's author, who cannot approve their own change" \
    "review 50 is by renovate[bot], which is not a person's account"; do
    fresh
    approve_event pull_request_review
    export AVER_STUB_APPROVE=$why
    OUT_DIR="$W/out" bash "$ACTION/approve.sh" >"$W/log"
    equals "$(out approved)" false
    equals "$(out head)" ""
    contains "$W/log" "::notice title=Aver: not approved::$why"
    contains "$W/summary" "$why"
    equals "$(gh_calls)" 0
  done
  fresh
  approve_event pull_request_review
  export AVER_STUB_APPROVE=error
  local rc=0
  OUT_DIR="$W/out" bash "$ACTION/approve.sh" >"$W/log" 2>&1 || rc=$?
  equals "$rc" 2
  contains "$W/log" "::error title=aver ci approve failed::"
  fresh
  approve_event pull_request
  rc=0
  OUT_DIR="$W/out" bash "$ACTION/approve.sh" >"$W/log" 2>&1 || rc=$?
  equals "$rc" 2
  contains "$W/log" "mode: approve runs on pull_request_review events"
  [ ! -f "$AVER_LOG" ] || die "aver ran"
}

# The steps that run the pull request's code are off in mode: approve, the
# approve step is the only one with the token besides the posting steps,
# and the check run step posts on the approved head or the verified one.
test_action_wires_approve_and_check() {
  python3 - "$ACTION/action.yml" <<'PY'
import sys, yaml
doc = yaml.safe_load(open(sys.argv[1]))
steps = {s.get("name"): s for s in doc["runs"]["steps"]}
for name in ["Run aver ci", "Create the run user"]:
    assert "inputs.mode != 'approve'" in steps[name]["if"], steps[name]["if"]
    assert "GH_TOKEN" not in steps[name].get("env", {}), name
assert steps["Run aver ci"]["env"]["CHECK"] == "${{ inputs.check }}"
approve = steps["Bind a review's approval to the head commit"]
assert approve["id"] == "approve" and "inputs.mode == 'approve'" in approve["if"], approve
post = steps["Post the Aver check run"]
assert "inputs.review == 'approve'" in post["if"] and "steps.approve.outputs.approved == 'true'" in post["if"], post["if"]
assert post["env"]["HEAD_SHA"] == "${{ steps.approve.outputs.head || steps.revs.outputs.head }}"
assert post["env"]["CHECK"] == "${{ inputs.check }}"
upload = steps["Upload the receipt"]["with"]["name"]
assert "format('aver-receipt-{0}', inputs.check)" in upload and "'aver-receipt')" in upload, upload
assert doc["inputs"]["mode"]["default"] == "verify"
assert doc["inputs"]["check"]["default"] == ""
PY
}

# The check input reaches aver ci as --check.
test_run_check_argument() {
  fresh
  pr_event o/r o/r
  export AVER_STUB_EXIT=0 AVER_STUB_VERDICT=PASS
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h CHECK=unit bash "$ACTION/run.sh" >/dev/null
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  contains "$W/argv" "--check unit"
  fresh
  pr_event o/r o/r
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h bash "$ACTION/run.sh" >/dev/null
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  lacks "$W/argv" "--check"
}

test_run_skipped_final_step_exits_0() {
  fresh
  final_step ""
}

test_run_arguments() {
  fresh
  pr_event o/r o/r
  export AVER_STUB_EXIT=0 AVER_STUB_VERDICT=PASS
  OUT_DIR="$W/out" CI_HOME="$W/home" WORKING_DIRECTORY=repo BASE_REV=b HEAD_REV=h FAIL_ON=not-pass \
    RUN_AS=aver-run VIEWER_URL=https://viewer.example/receipt.html bash "$ACTION/run.sh" >/dev/null
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  contains "$W/argv" "ci --repo repo --base b --head h --out-dir $W/out --fail-on not-pass --github-event"
  contains "$W/argv" "--github-event $W/event.json"
  contains "$W/argv" "--home $W/home"
  contains "$W/argv" "--viewer-url https://viewer.example/receipt.html"
}

# Native by default: --run-as when the run user exists, and the persistent
# home only then (otherwise native code could read its key).
test_run_native_arguments() {
  fresh
  make_repo "$W/repo"
  pr_event o/r o/r
  export AVER_STUB_EXIT=0 AVER_STUB_VERDICT=PASS
  OUT_DIR="$W/out" CI_HOME="$W/home" WORKING_DIRECTORY="$W/repo" BASE_REV=b HEAD_REV=h RUN_AS=aver-run \
    bash "$ACTION/run.sh" >/dev/null
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  contains "$W/argv" "--run-as aver-run"
  contains "$W/argv" "--home $W/home"

  fresh
  make_repo "$W/repo"
  pr_event o/r o/r
  OUT_DIR="$W/out" CI_HOME="$W/home" WORKING_DIRECTORY="$W/repo" BASE_REV=b HEAD_REV=h \
    bash "$ACTION/run.sh" >/dev/null
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  lacks "$W/argv" "--run-as"
  lacks "$W/argv" "--home"
}

# A token actions/checkout persisted, or an OIDC request URL, would be
# readable by the code native checks run: the persisted token is removed
# from .git/config first, and an OIDC request URL refuses the run (exit 2).
test_run_native_keeps_credentials_away() {
  fresh
  make_repo "$W/repo"
  git -C "$W/repo" config http.https://github.com/.extraheader "AUTHORIZATION: basic x"
  pr_event o/r o/r
  export AVER_STUB_EXIT=0 AVER_STUB_VERDICT=PASS
  OUT_DIR="$W/out" WORKING_DIRECTORY="$W/repo" BASE_REV=b HEAD_REV=h bash "$ACTION/run.sh" >"$W/log"
  equals "$(out exit-code)" 0
  contains "$W/log" "removed http.https://github.com/.extraheader"
  if git -C "$W/repo" config --get-regexp 'extraheader' >/dev/null; then die "the token is still in .git/config"; fi

  # A submodule's own configuration (checkout with submodules: true) holds
  # the header too, where the repository's configuration never reads it.
  fresh
  make_repo "$W/repo"
  mkdir -p "$W/repo/.git/modules/sub/modules/deeper"
  git config --file "$W/repo/.git/modules/sub/config" http.https://github.com/.extraheader "AUTHORIZATION: basic x"
  git config --file "$W/repo/.git/modules/sub/modules/deeper/config" http.https://github.com/.extraheader "AUTHORIZATION: basic x"
  pr_event o/r o/r
  OUT_DIR="$W/out" WORKING_DIRECTORY="$W/repo" BASE_REV=b HEAD_REV=h bash "$ACTION/run.sh" >"$W/log"
  equals "$(out exit-code)" 0
  contains "$W/log" "removed http.https://github.com/.extraheader from modules/sub/config"
  contains "$W/log" "from modules/sub/modules/deeper/config"
  if grep -rqi extraheader "$W/repo/.git"; then die "a token is still under .git"; fi

  # Checkout v6 and later keep the header in a file under $RUNNER_TEMP that
  # .git/config (and each submodule's configuration) includes through
  # includeIf.gitdir: the header is removed from that file.
  fresh
  make_repo "$W/repo"
  mkdir -p "$W/repo/.git/modules/sub"
  printf '[http "https://github.com/"]\n\textraheader = AUTHORIZATION: basic x\n' >"$W/credentials.config"
  git -C "$W/repo" config --local "includeIf.gitdir:$W/repo/.git.path" "$W/credentials.config"
  git config --file "$W/repo/.git/modules/sub/config" "includeIf.gitdir:$W/repo/.git/modules/sub.path" "$W/credentials.config"
  pr_event o/r o/r
  rm -f "$AVER_LOG"
  OUT_DIR="$W/out" WORKING_DIRECTORY="$W/repo" BASE_REV=b HEAD_REV=h bash "$ACTION/run.sh" >"$W/log"
  equals "$(out exit-code)" 0
  contains "$W/log" "from $W/credentials.config, which config includes"
  if grep -qi extraheader "$W/credentials.config"; then die "the token is still in the included file"; fi
  [ -f "$AVER_LOG" ] || die "aver did not run"

  # A header the action cannot remove (the global configuration) refuses
  # the run.
  fresh
  make_repo "$W/repo"
  printf '[http "https://github.com/"]\n\textraheader = AUTHORIZATION: basic x\n' >"$W/global.config"
  pr_event o/r o/r
  rm -f "$AVER_LOG"
  GIT_CONFIG_GLOBAL="$W/global.config" OUT_DIR="$W/out" WORKING_DIRECTORY="$W/repo" BASE_REV=b HEAD_REV=h \
    bash "$ACTION/run.sh" >"$W/log"
  equals "$(out exit-code)" 2
  contains "$W/log" "other than the repository's own"
  [ ! -f "$AVER_LOG" ] || die "aver ran"

  fresh
  make_repo "$W/repo"
  pr_event o/r o/r
  rm -f "$AVER_LOG"
  OUT_DIR="$W/out" WORKING_DIRECTORY="$W/repo" BASE_REV=b HEAD_REV=h ACTIONS_ID_TOKEN_REQUEST_URL=https://token.example \
    bash "$ACTION/run.sh" >"$W/log"
  equals "$(out exit-code)" 2
  contains "$W/log" "id-token"
  [ ! -f "$AVER_LOG" ] || die "aver ran"
}

test_run_user_without_sudo_warns() {
  fresh
  mkdir -p "$W/nosudo"
  printf '#!/bin/sh\nexit 1\n' >"$W/nosudo/sudo"
  chmod +x "$W/nosudo/sudo"
  PATH="$W/nosudo:$PATH" bash "$ACTION/run-user.sh" >"$W/log"
  contains "$W/log" "uid \"same\""
  equals "$(out user)" ""
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# Serves $W/www over HTTP on 127.0.0.1 and logs request paths to $W/http.log.
serve_releases() {
  python3 - "$W/www" "$W/port" >"$W/http.log" 2>&1 <<'EOF' &
import functools, http.server, sys
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[1])
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
open(sys.argv[2], "w").write(str(server.server_address[1]))
server.serve_forever()
EOF
  trap "kill $! 2>/dev/null || true" EXIT
  local i
  for i in $(seq 1 50); do
    [ -s "$W/port" ] && break
    sleep 0.1
  done
  [ -s "$W/port" ] || die "the release server did not start"
  export AVER_RELEASE_BASE_URL="http://127.0.0.1:$(cat "$W/port")"
}

install_setup() { # <ref>
  fresh
  ACTION_DIR="$W/runner/_actions/wonkylamp/aver/$1/action"
  mkdir -p "$ACTION_DIR"
  echo "lock" >"$ACTION_DIR/../Cargo.lock"
  : >"$W/path"
  mkdir -p "$W/www"
  export GITHUB_ACTION_PATH="$ACTION_DIR" GITHUB_PATH="$W/path" RUNNER_OS=Linux RUNNER_ARCH=X64
  unset ACTION_REPOSITORY ACTION_REF AVER_VERSION
}

publish_asset() { # <tag> <asset> [<checksum override>]
  local dir="$W/www/wonkylamp/aver/releases/download/$1"
  mkdir -p "$dir"
  printf '#!/bin/sh\necho aver-from-release\n' >"$dir/$2"
  printf '%s  %s\n' "${3:-$(sha256_of "$dir/$2")}" "$2" >"$dir/$2.sha256"
}

test_install_from_release() {
  install_setup v9.9.9
  publish_asset v9.9.9 aver-linux-x86_64
  serve_releases
  bash "$ACTION/install-aver.sh" >"$W/log" 2>&1 || { cat "$W/log"; die "install failed"; }
  equals "$(out need-build)" false
  equals "$(out ref)" v9.9.9
  contains "$W/http.log" "GET /wonkylamp/aver/releases/download/v9.9.9/aver-linux-x86_64 "
  equals "$("$(cat "$W/path")/aver")" aver-from-release
}

test_install_branch_ref_builds() {
  install_setup feature/x
  serve_releases
  bash "$ACTION/install-aver.sh" >"$W/log" 2>&1 || { cat "$W/log"; die "install failed"; }
  contains "$W/http.log" "GET /wonkylamp/aver/releases/download/feature/x/aver-linux-x86_64 "
  equals "$(out need-build)" true
  equals "$(out source-dir)" "$(cd "$ACTION_DIR/.." && pwd)"
  equals "$(out cargo-lock-hash)" "$(sha256_of "$ACTION_DIR/../Cargo.lock")"
  equals "$(cat "$W/path")" ""
}

# A private action repository answers 404 to an unauthenticated release
# download, exactly like a commit SHA with no release, so both build from the
# source the runner checked out for the action.
test_install_sha_ref_builds_from_action_source() {
  local sha=45d8d020c9d2a6c7e9f8b1a3d4e5f60718293a4b
  install_setup "$sha"
  echo '[workspace]' >"$ACTION_DIR/../Cargo.toml"
  serve_releases
  bash "$ACTION/install-aver.sh" >"$W/log" 2>&1 || { cat "$W/log"; die "install failed"; }
  contains "$W/http.log" "GET /wonkylamp/aver/releases/download/$sha/aver-linux-x86_64 "
  contains "$W/log" "::notice title=Building aver from source::no release asset aver-linux-x86_64 for wonkylamp/aver@$sha"
  equals "$(out need-build)" true
  equals "$(out ref)" "$sha"
  equals "$(out source-dir)" "$(cd "$ACTION_DIR/.." && pwd)"
  [ -f "$(out source-dir)/Cargo.toml" ] || die "source-dir has no Cargo.toml"
  equals "$(cat "$W/path")" ""
}

# The build step compiles the source-dir that install-aver.sh names, never a
# fresh clone, which a private repository's token could not read.
test_action_builds_the_install_source_dir() {
  python3 - "$ACTION/action.yml" <<'EOF'
import sys, yaml
steps = {s.get("name"): s for s in yaml.safe_load(open(sys.argv[1]))["runs"]["steps"]}
build = steps["Build aver from source"]
assert build["env"]["SOURCE_DIR"] == "${{ steps.install.outputs.source-dir }}", build["env"]
assert "steps.install.outputs.need-build == 'true'" in build["if"], build["if"]
assert '--manifest-path "$SOURCE_DIR/Cargo.toml"' in build["run"], build["run"]
assert "--git" not in build["run"], build["run"]
EOF
}

# actions/cache saves in a post step that runs only when the job succeeds, so a
# FAIL_OWN verdict (the job's last step exits 1) would never save the build.
# The action restores and saves explicitly, the save right after the build.
test_action_saves_the_build_cache_before_the_verdict() {
  python3 - "$ACTION/action.yml" <<'EOF'
import sys, yaml
steps = yaml.safe_load(open(sys.argv[1]))["runs"]["steps"]
names = [s.get("name") for s in steps]
for s in steps:
    assert not s.get("uses", "").startswith("actions/cache@"), "actions/cache saves only on success: " + s["uses"]
restore = steps[names.index("Restore the cargo cache")]
save = steps[names.index("Save the cargo cache")]
assert restore["uses"].startswith("actions/cache/restore@"), restore["uses"]
assert save["uses"].startswith("actions/cache/save@"), save["uses"]
assert names.index("Build aver from source") < names.index("Save the cargo cache") < names.index("Run aver ci"), names
assert save["with"]["key"] == restore["with"]["key"], (save["with"]["key"], restore["with"]["key"])
assert save["with"]["path"] == restore["with"]["path"]
assert "cache-hit != 'true'" in save["if"], save["if"]
assert "need-build == 'true'" in save["if"], save["if"]
EOF
}

test_install_version_input_and_arm() {
  install_setup main
  publish_asset v1.2.3 aver-linux-aarch64
  serve_releases
  AVER_VERSION=v1.2.3 RUNNER_ARCH=ARM64 bash "$ACTION/install-aver.sh" >"$W/log" 2>&1 || { cat "$W/log"; die "install failed"; }
  equals "$(out need-build)" false
  contains "$W/http.log" "GET /wonkylamp/aver/releases/download/v1.2.3/aver-linux-aarch64 "
}

test_install_checksum_mismatch_fails() {
  install_setup v9.9.9
  publish_asset v9.9.9 aver-linux-x86_64 0000000000000000000000000000000000000000000000000000000000000000
  serve_releases
  local rc=0
  bash "$ACTION/install-aver.sh" >"$W/log" 2>&1 || rc=$?
  equals "$rc" 1
  contains "$W/log" "checksum mismatch"
  equals "$(cat "$W/path")" ""
}

test_install_local_action_builds() {
  install_setup v9.9.9
  export GITHUB_ACTION_PATH="$W/workspace/action"
  mkdir -p "$GITHUB_ACTION_PATH"
  bash "$ACTION/install-aver.sh" >"$W/log"
  equals "$(out need-build)" true
  equals "$(out ref)" local
}

# A series ref (v0.4, which release.yml moves) installs the release its
# commit's Cargo.toml names; a ref the version does not belong to builds.
test_install_series_ref_uses_the_source_version() {
  install_setup v9.9
  printf '[workspace]\n\n[workspace.package]\nversion = "9.9.3"\n' >"$ACTION_DIR/../Cargo.toml"
  publish_asset v9.9.3 aver-linux-x86_64
  serve_releases
  bash "$ACTION/install-aver.sh" >"$W/log" 2>&1 || { cat "$W/log"; die "install failed"; }
  equals "$(out need-build)" false
  equals "$(out ref)" v9.9.3
  contains "$W/http.log" "GET /wonkylamp/aver/releases/download/v9.9.3/aver-linux-x86_64 "

  install_setup v9.8
  printf '[workspace.package]\nversion = "9.9.3"\n' >"$ACTION_DIR/../Cargo.toml"
  serve_releases
  bash "$ACTION/install-aver.sh" >"$W/log" 2>&1 || { cat "$W/log"; die "install failed"; }
  contains "$W/http.log" "GET /wonkylamp/aver/releases/download/v9.8/aver-linux-x86_64 "
  equals "$(out need-build)" true
}

# smoke.sh refuses a dynamically linked Linux binary and a wrong version
# before any demo runs.
test_release_smoke_refusals() {
  fresh
  mkdir -p "$W/fake" "$W/dist"
  printf '#!/bin/sh\necho "ELF 64-bit LSB pie executable, dynamically linked, interpreter /lib64/ld-linux-x86-64.so.2"\n' >"$W/fake/file"
  printf '#!/bin/sh\ncase "$1" in --version) echo "aver 1.2.3" ;; demo) echo "NOT DONE: FAIL_OWN"; echo "DONE: PASS" ;; esac\n' >"$W/dist/aver-linux-x86_64"
  cp "$W/dist/aver-linux-x86_64" "$W/dist/aver-darwin-aarch64"
  chmod +x "$W/fake/file" "$W/dist/"*
  local smoke="$ACTION/../packaging/release/smoke.sh" rc=0
  PATH="$W/fake:$PATH" bash "$smoke" "$W/dist/aver-linux-x86_64" 1.2.3 >"$W/log" 2>&1 || rc=$?
  equals "$rc" 1
  contains "$W/log" "not statically linked: ELF 64-bit LSB pie executable, dynamically linked"
  rc=0
  PATH="$W/fake:$PATH" bash "$smoke" "$W/dist/aver-darwin-aarch64" 1.2.4 >"$W/log" 2>&1 || rc=$?
  equals "$rc" 1
  contains "$W/log" "--version printed 'aver 1.2.3', not 'aver 1.2.4'"
  PATH="$W/fake:$PATH" bash "$smoke" "$W/dist/aver-darwin-aarch64" 1.2.3 >"$W/log" 2>&1 || { cat "$W/log"; die "smoke failed"; }
  contains "$W/log" "aver-darwin-aarch64: aver 1.2.3, demo passed"
}

# The restore step runs whatever happened before it, and runs aver ci
# restore on the working directory; without aver installed it does nothing.
test_restore_step() {
  fresh
  python3 - "$ACTION/action.yml" <<'PY'
import sys, yaml
steps = yaml.safe_load(open(sys.argv[1]))["runs"]["steps"]
names = [s.get("name") for s in steps]
i = names.index("Restore the checkout")
step = steps[i]
assert names.index("Run aver ci") < i, names
assert step["if"].startswith("always()"), step["if"]
assert "restore.sh" in step["run"], step["run"]
PY
  mkdir -p "$W/bin"
  printf '#!/bin/sh\nprintf "%%s\\n" "$@" >"$AVER_LOG"\n' >"$W/bin/aver"
  chmod +x "$W/bin/aver"
  PATH="$W/bin:$PATH" WORKING_DIRECTORY="$W/repo" bash "$ACTION/restore.sh"
  equals "$(tr '\n' ' ' <"$AVER_LOG")" "ci restore --repo $W/repo "
  rm -f "$AVER_LOG"
  PATH=/usr/bin:/bin WORKING_DIRECTORY="$W/repo" /bin/bash "$ACTION/restore.sh"
  [ ! -e "$AVER_LOG" ] || die "aver ran without being installed"
}

test_workflows_pinned() {
  bash "$ACTION/test/check-workflows.sh"
}

# Base records (5B.3): the keys step, the restore on pull_request with
# --base-record only on an exact hit, and the record saved on push and
# merge_group even when the verdict fails the job.
test_record_keys_written() {
  fresh
  WORKING_DIRECTORY=repo BASE_REV=b HEAD_REV=h bash "$ACTION/record-keys.sh" >"$W/log"
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  contains "$W/argv" "ci keys --repo repo --base b --head h"
  equals "$(out base)" aver-base-stubbase
  equals "$(out head)" aver-base-stubhead
}

test_record_keys_failure_only_warns() {
  fresh
  AVER_STUB_KEYS_EXIT=2 BASE_REV=b HEAD_REV=h bash "$ACTION/record-keys.sh" >"$W/log"
  contains "$W/log" "::warning title=aver ci keys failed::"
  equals "$(out base)" ""
  equals "$(out head)" ""
}

test_run_base_record_arguments() {
  fresh
  pr_event o/r o/r
  export AVER_STUB_EXIT=1 AVER_STUB_VERDICT=FAIL_OWN
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h bash "$ACTION/run.sh" >/dev/null
  lacks "$AVER_LOG" "--base-record"
  lacks "$AVER_LOG" "--record"
  equals "$(out recorded)" false

  fresh
  pr_event o/r o/r
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h BASE_RECORD="$W/base-record" bash "$ACTION/run.sh" >/dev/null
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  contains "$W/argv" "--base-record $W/base-record"
  lacks "$AVER_LOG" "--record"

  fresh
  mkdir -p "$W/record" && echo stale >"$W/record/stale"
  OUT_DIR="$W/out" BASE_REV=b HEAD_REV=h RECORD="$W/record" bash "$ACTION/run.sh" >/dev/null
  tr '\n' ' ' <"$AVER_LOG" >"$W/argv"
  contains "$W/argv" "--record $W/record"
  [ ! -e "$W/record/stale" ] || die "a stale record directory was kept"
  equals "$(out recorded)" true
  equals "$(out exit-code)" 1
}

test_action_base_record_wiring() {
  python3 - "$ACTION/action.yml" <<'EOF'
import re, sys, yaml
steps = yaml.safe_load(open(sys.argv[1]))["runs"]["steps"]
names = [s.get("name") for s in steps]
by = {s.get("name"): s for s in steps}
keys, restore, run, save = (by[n] for n in ("Compute the base record keys", "Restore the base record", "Run aver ci", "Save the record"))
order = [names.index(n) for n in ("Install aver", "Compute the base record keys", "Restore the base record", "Run aver ci", "Save the record")]
assert order == sorted(order), names
assert keys["id"] == "keys" and "record-keys.sh" in keys["run"], keys
pin = re.compile(r"^actions/cache/(restore|save)@[0-9a-f]{40}$")
for s in (restore, save):
    assert pin.match(s["uses"]), s["uses"]
assert restore["uses"].startswith("actions/cache/restore@") and save["uses"].startswith("actions/cache/save@")
assert "github.event_name == 'pull_request'" in restore["if"], restore["if"]
assert restore["with"]["key"] == "${{ steps.keys.outputs.base }}", restore["with"]
assert "restore-keys" not in restore["with"], "a prefix match would restore another base's record"
env = run["env"]
assert "steps.base-record.outputs.cache-hit == 'true'" in env["BASE_RECORD"], env["BASE_RECORD"]
assert restore["with"]["path"] == "${{ runner.temp }}/aver-base-record" and "aver-base-record" in env["BASE_RECORD"]
assert "github.event_name == 'merge_group'" in env["RECORD"], env["RECORD"]
assert "github.event_name == 'push'" in env["RECORD"] and "default_branch" in env["RECORD"], env["RECORD"]
assert save["with"]["path"] == "${{ runner.temp }}/aver-record" and "aver-record" in env["RECORD"]
assert save["with"]["key"] == "${{ steps.keys.outputs.head }}", save["with"]
assert save["if"].startswith("always()") and "steps.run.outputs.recorded == 'true'" in save["if"], save["if"]
EOF
}

failed=0
total=0
for t in $(declare -F | awk '{print $3}' | grep '^test_'); do
  total=$((total + 1))
  set +e
  (
    set -e
    "$t"
  ) >"$T/$t.log" 2>&1
  rc=$?
  set -e
  if [ "$rc" = 0 ]; then
    echo "ok   ${t#test_}"
  else
    echo "FAIL ${t#test_}"
    sed 's/^/     | /' "$T/$t.log"
    failed=$((failed + 1))
  fi
done

echo "$((total - failed))/$total passed"
[ "$failed" = 0 ]
