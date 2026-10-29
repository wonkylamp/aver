#!/usr/bin/env bash
# Runs the action end to end on this machine with no network: the steps of
# action/action.yml (run-action.py), the real action scripts and the real
# aver binary, against a bare Git repository as origin and a mock GitHub API
# (mock-github.py) that records every request. Each flow is one job: a fresh
# RUNNER_TEMP, a checkout as actions/checkout makes it, the event payload,
# and assertions on the job's conclusion, outputs, summary and requests.
#
# AVER=<binary> picks the binary (default target/release/aver). The mock
# serves it as the Linux release asset install-aver.sh downloads, so the
# install step runs as on a Linux runner, also on macOS. Checks run as the
# runner's own user: a sudo that always fails is put on PATH, so run-user.sh
# never creates a user here; AVER_E2E_SUDO=1 leaves the real sudo, and a
# Linux runner with passwordless sudo then runs the checks as aver-run
# (--run-as), which the receipts must record.
set -euo pipefail

if [ "$(uname)" = Darwin ] && [ -d /opt/homebrew/bin ]; then
  PATH=/opt/homebrew/bin:$PATH
fi

HERE=$(cd "$(dirname "$0")" && pwd)
ACTION=$(cd "$HERE/.." && pwd)
ROOT=$(cd "$ACTION/.." && pwd)
AVER=${AVER:-${CARGO_TARGET_DIR:-$ROOT/target}/release/aver}
[ -x "$AVER" ] || { echo "no aver binary at $AVER: run cargo build --release -p aver, or set AVER" >&2; exit 2; }
AVER=$(cd "$(dirname "$AVER")" && pwd)/$(basename "$AVER")

T=$(mktemp -d "${TMPDIR:-/tmp}/aver-e2e.XXXXXX")
T=$(cd "$T" && pwd -P)
MOCK=$T/mock
ORIGIN=$T/origin.git
DEV=$T/dev
TAG=v0.0.0-e2e
STARTED=$SECONDS

cleanup() {
  local code=$?
  if [ -n "${MOCK_PID:-}" ]; then
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
  if [ "$code" = 0 ] && [ -z "${E2E_KEEP:-}" ]; then
    rm -rf "$T"
  else
    echo "kept $T" >&2
  fi
}
trap cleanup EXIT

export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=$T/gitconfig
cat >"$GIT_CONFIG_GLOBAL" <<'EOF'
[user]
	name = Aver E2E
	email = e2e@example.invalid
[init]
	defaultBranch = main
[advice]
	detachedHead = false
EOF

mkdir -p "$T/bin" "$MOCK/www" "$T/cache"
printf '#!/usr/bin/env bash\nexec python3 %q gh "$@"\n' "$HERE/mock-github.py" >"$T/bin/gh"
chmod +x "$T/bin/gh"
CHECK_UID=separate
if [ -z "${AVER_E2E_SUDO:-}" ]; then
  printf '#!/bin/sh\nexit 1\n' >"$T/bin/sudo"
  chmod +x "$T/bin/sudo"
  CHECK_UID=same
fi
export PATH="$T/bin:$PATH"

echo '{"pulls":{},"reviews":{}}' >"$MOCK/state.json"
: >"$MOCK/requests.jsonl"
python3 "$HERE/mock-github.py" serve "$MOCK" 2>"$MOCK/server.log" &
MOCK_PID=$!
for _ in $(seq 1 100); do
  [ -s "$MOCK/port" ] && break
  sleep 0.05
done
[ -s "$MOCK/port" ] || { echo "the mock GitHub did not start: $(cat "$MOCK/server.log")" >&2; exit 2; }
export MOCK_GITHUB_URL="http://127.0.0.1:$(cat "$MOCK/port")"

release=$MOCK/www/wonkylamp/aver/releases/download/$TAG
mkdir -p "$release"
cp "$AVER" "$release/aver-linux-x86_64"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$release" && sha256sum aver-linux-x86_64 >aver-linux-x86_64.sha256)
else
  (cd "$release" && shasum -a 256 aver-linux-x86_64 >aver-linux-x86_64.sha256)
fi

# What GitHub's runner sets for every job here. RUNNER_OS and RUNNER_ARCH
# name the release asset install-aver.sh fetches (the local binary).
export AVER_RELEASE_BASE_URL=$MOCK_GITHUB_URL ACTION_REPOSITORY=wonkylamp/aver ACTION_REF=$TAG
export RUNNER_OS=Linux RUNNER_ARCH=X64 GITHUB_REPOSITORY=o/r GITHUB_SERVER_URL=https://github.com GITHUB_RUN_ID=1
export GH_TOKEN=e2e-token E2E_CACHE_DIR=$T/cache
unset CI GITHUB_ACTIONS AVER_HOME AVER_URL ACTIONS_ID_TOKEN_REQUEST_URL

fail() {
  echo "FAIL [$FLOW]: $*" >&2
  [ -z "${J:-}" ] || { echo "--- job log $J/log" >&2; tail -n 60 "$J/log" >&2; }
  exit 1
}
contains() { grep -qF -- "$2" "$1" || fail "$(basename "$1") does not contain '$2'"; }
lacks() { ! grep -qF -- "$2" "$1" || fail "$(basename "$1") contains '$2'"; }
equals() { [ "$1" = "$2" ] || fail "${3:-value}: expected '$2', got '$1'"; }
out() { jq -r --arg k "$1" '.outputs[$k]' "$J/result.json"; }
step() { jq -r --arg n "$1" '.steps[] | select(.name == $n) | .outcome' "$J/result.json"; }
step_out() { jq -r --arg n "$1" --arg k "$2" '.steps[] | select(.name == $n) | .outputs[$k] // ""' "$J/result.json"; }
# The job's requests to the mock matching a method and a path regex, one JSON per line.
requests() { jq -c --arg m "$1" --arg p "$2" 'select(.method == $m and (.path | test($p)))' "$J/requests.jsonl"; }
count() { requests "$1" "$2" | wc -l | tr -d ' '; }
receipt() { jq -r "$1" "$J/artifacts/${ARTIFACT:-aver-receipt}/receipt.json"; }

state() { # <jq filter> — edits the mock's state.json
  jq "$@" "$MOCK/state.json" >"$MOCK/state.tmp" && mv "$MOCK/state.tmp" "$MOCK/state.json"
}

# The repository: fixtures/sh-calc with a workflow file, as origin's main.
git init -q --bare "$ORIGIN"
git -C "$ORIGIN" config uploadpack.allowAnySHA1InWant true
git init -q "$DEV"
cp -R "$ROOT/fixtures/sh-calc/." "$DEV/"
mkdir -p "$DEV/.github/workflows"
cat >"$DEV/.github/workflows/aver.yml" <<'EOF'
on: [pull_request, push, merge_group]
jobs:
  aver:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: wonkylamp/aver/action@v0.0.0-e2e
EOF
git -C "$DEV" add -A
git -C "$DEV" commit -qm "sh-calc"
git -C "$DEV" remote add origin "file://$ORIGIN"
git -C "$DEV" push -q origin HEAD:refs/heads/main

# Opens pull request <n> with the change <edit> makes on main: pushes
# refs/pull/<n>/head and the test merge GitHub keeps at refs/pull/<n>/merge,
# writes <dir>/event.json, and tells the mock the head. Sets PR_BASE, PR_HEAD.
open_pr() { # <n> <edit function> <body file or ''>
  local n=$1
  git -C "$DEV" fetch -q origin main
  git -C "$DEV" checkout -q --detach origin/main
  (cd "$DEV" && "$2")
  git -C "$DEV" add -A
  git -C "$DEV" commit -qm "pr $n: $2"
  PR_BASE=$(git -C "$DEV" rev-parse origin/main)
  PR_HEAD=$(git -C "$DEV" rev-parse HEAD)
  git -C "$DEV" push -q origin "HEAD:refs/pull/$n/head"
  git -C "$DEV" checkout -q --detach origin/main
  git -C "$DEV" merge -q --no-ff --no-edit -m "Merge $PR_HEAD into $PR_BASE" "$PR_HEAD"
  git -C "$DEV" push -q origin "HEAD:refs/pull/$n/merge"
  mkdir -p "$T/pr$n"
  jq -n --argjson n "$n" --arg base "$PR_BASE" --arg head "$PR_HEAD" --rawfile body "${3:-/dev/null}" '{
    action: "synchronize", number: $n,
    pull_request: {number: $n, body: (if $body == "" then null else $body end),
      user: {login: "author", type: "User"},
      base: {sha: $base, ref: "main", repo: {full_name: "o/r"}},
      head: {sha: $head, ref: "pr-\($n)", repo: {full_name: "o/r"}}},
    repository: {full_name: "o/r", default_branch: "main"}}' >"$T/pr$n/event.json"
  state --arg n "$n" --arg head "$PR_HEAD" '.pulls[$n] = {number: ($n | tonumber), head: {sha: $head}}'
}

# Runs the action as one job: checks out <ref> as actions/checkout does
# (depth 1, a detached head, a v6 credentials include file under
# RUNNER_TEMP), then runs action.yml's steps with the given inputs. Sets J
# to the job's directory: log, result.json, summary, requests.jsonl,
# artifacts/, include (the credentials file), exit.
job() { # <name> <event name> <event file> <ref> [input=value ...]
  local name=$1 event=$2 payload=$3 ref=$4
  shift 4
  J=$T/jobs/$name
  mkdir -p "$J/temp" "$J/artifacts"
  cp "$payload" "$J/event.json"
  git init -q "$J/ws"
  git -C "$J/ws" remote add origin "file://$ORIGIN"
  # J_CHECKOUT (a commit) is what the workflow's checkout `ref:` names
  # instead of GITHUB_REF; J_FULL=1 is `fetch-depth: 0` with
  # `persist-credentials: false`; J_SETUP is a script of the workflow's
  # `run:` steps before the action.
  if [ -n "${J_FULL:-}" ]; then
    git -C "$J/ws" fetch -q --no-tags origin '+refs/heads/*:refs/remotes/origin/*'
    git -C "$J/ws" fetch -q --no-tags origin "+${J_CHECKOUT:-$ref}:refs/remotes/origin/checkout"
  else
    git -C "$J/ws" fetch -q --no-tags --depth=1 origin "+${J_CHECKOUT:-$ref}:refs/remotes/origin/checkout"
  fi
  git -C "$J/ws" checkout -q --detach refs/remotes/origin/checkout
  J_INCLUDE=$J/temp/git-credentials-e2e.config
  if [ -z "${J_FULL:-}" ]; then
    printf '[http "https://github.com/"]\n\textraheader = AUTHORIZATION: basic ZTJlOnNlY3JldA==\n' >"$J_INCLUDE"
    git -C "$J/ws" config --local "includeIf.gitdir:$J/ws/.git.path" "$J_INCLUDE"
  fi
  [ -z "${J_SETUP:-}" ] || (cd "$J/ws" && GITHUB_PATH=$J/temp/path bash -e "$J_SETUP") >"$J/setup.log" 2>&1 || fail "the setup steps failed: $(cat "$J/setup.log")"
  local mark
  mark=$(wc -l <"$MOCK/requests.jsonl")
  : >"$J/summary"
  set +e
  (
    cd "$J/ws"
    if [ -s "$J/temp/path" ]; then while IFS= read -r dir; do PATH=$dir:$PATH; done <"$J/temp/path"; fi
    GITHUB_EVENT_NAME=$event GITHUB_EVENT_PATH=$J/event.json GITHUB_REF=$ref \
      GITHUB_WORKFLOW_REF="o/r/.github/workflows/aver.yml@$ref" \
      GITHUB_WORKSPACE=$J/ws RUNNER_TEMP=$J/temp GITHUB_STEP_SUMMARY=$J/summary \
      E2E_ARTIFACTS_DIR=$J/artifacts \
      python3 "$HERE/run-action.py" "$ACTION" "$J/result.json" "$@"
  ) >"$J/log" 2>&1
  echo $? >"$J/exit"
  set -e
  tail -n +"$((mark + 1))" "$MOCK/requests.jsonl" >"$J/requests.jsonl"
  [ -f "$J/result.json" ] || fail "run-action.py wrote no result"
}

# Every job: the release binary installed, the credentials header gone
# when aver ci ran, and the checkout back at the job's commit.
common_job_checks() {
  contains "$J/log" "Installed aver from wonkylamp/aver@$TAG"
  if [ "$(step 'Run aver ci')" = success ] && [ -f "$J_INCLUDE" ]; then
    lacks "$J_INCLUDE" "extraheader"
    contains "$J/log" "removed http.https://github.com/.extraheader from $J_INCLUDE"
  fi
  equals "$(git -C "$J/ws" rev-parse HEAD)" "$(git -C "$J/ws" rev-parse refs/remotes/origin/checkout)" "the checkout's commit after the job"
  [ -z "$(git -C "$J/ws" status --porcelain --untracked-files=no)" ] || fail "the checkout has changes after the job"
  [ ! -e "$J/ws/.git/aver-swap.json" ] || fail "the checkout was left swapped"
}

passed() { echo "ok $FLOW ($((SECONDS - FLOW_START))s)"; FLOW_START=$SECONDS; }
FLOW_START=$SECONDS

edit_sub() {
  printf 'sub() { echo $(($1 - $2)); }\n' >>calc.sh
  printf 'subtracts [crit:sub] :: [ "$(sub 5 3)" = 2 ]\n' >>test/add.tests
}
edit_regression() {
  sed -i.bak 's/\$1 + \$2/$1 + $2 + 1/' calc.sh && rm calc.sh.bak
}
# Changes what add does on purpose and rewrites both tests to match: the
# base's copies fail on the head, the head's pass (CHANGED, REVIEW).
edit_expectation() {
  edit_regression
  sed -i.bak -e 's/= 5 \]/= 6 ]/' -e 's/= -1 \]/= 0 ]/' test/add.tests && rm test/add.tests.bak
}
edit_mul() {
  printf 'mul() { echo $(($1 * $2)); }\n' >>calc.sh
  printf 'multiplies [crit:mul] :: [ "$(mul 4 3)" = 12 ]\n' >>test/add.tests
}

cat >"$T/body-sub.md" <<'EOF'
Adds sub.

```aver
- sub: sub(a, b) returns a minus b
```
EOF
cat >"$T/body-mul.md" <<'EOF'
Adds mul.

```aver
- mul: mul(a, b) returns a times b
```
EOF

FLOW="1 pull_request, a good change"
open_pr 1 edit_sub "$T/body-sub.md"
PR1_HEAD=$PR_HEAD
job pr1 pull_request "$T/pr1/event.json" refs/pull/1/merge
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out verdict)" PASS verdict
equals "$(out exit-code)" 0 exit-code
[ -n "$(out receipt-id)" ] || fail "no receipt-id output"
contains "$J/summary" "sub"
equals "$(count POST '^/repos/o/r/issues/1/comments$')" 1 "comments posted"
requests POST '^/repos/o/r/issues/1/comments$' | jq -r .body.body | head -n 1 | grep -qF '<!-- aver-receipt -->' || fail "the comment lacks the marker"
equals "$(count POST 'check-runs')" 0 "check runs with review: fail"
[ "$(receipt '.receipt.criteria | length')" -ge 1 ] || fail "the receipt has no criteria"
equals "$(receipt '.receipt.provenance.uid')" "$CHECK_UID" "the checks' user"
passed

FLOW="2 pull_request, a regression"
open_pr 2 edit_regression ""
job pr2 pull_request "$T/pr2/event.json" refs/pull/2/merge
common_job_checks
equals "$(cat "$J/exit")" 1 "job exit"
equals "$(out verdict)" FAIL_OWN verdict
equals "$(out exit-code)" 1 exit-code
equals "$(step 'Exit with the aver ci code')" failure "last step"
equals "$(count POST '^/repos/o/r/issues/2/comments$')" 1 "comments posted"
passed

FLOW="3 pull_request, a change that needs review"
open_pr 3 edit_expectation ""
PR3_HEAD=$PR_HEAD
job pr3-fail pull_request "$T/pr3/event.json" refs/pull/3/merge
common_job_checks
equals "$(cat "$J/exit")" 1 "job exit"
equals "$(out exit-code)" 3 exit-code
REVIEW_VERDICT=$(out verdict)
case "$REVIEW_VERDICT" in REVIEW | UNPROVEN) ;; *) fail "verdict $REVIEW_VERDICT, not REVIEW or UNPROVEN" ;; esac
contains "$J/summary" "Aver: needs maintainer review"
equals "$(count POST 'check-runs')" 0 "check runs with review: fail"

job pr3-approve pull_request "$T/pr3/event.json" refs/pull/3/merge review=approve
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out exit-code)" 0 exit-code
equals "$(out verdict)" "$REVIEW_VERDICT" verdict
equals "$(count POST '/check-runs$')" 1 "check runs"
PR3_APPROVE_RUN=$(requests POST '/check-runs$')
equals "$(jq -r .body.name <<<"$PR3_APPROVE_RUN")" Aver "check run name"
equals "$(jq -r .body.conclusion <<<"$PR3_APPROVE_RUN")" action_required "check run conclusion"
equals "$(jq -r .body.head_sha <<<"$PR3_APPROVE_RUN")" "$PR3_HEAD" "check run head_sha"
equals "$(count PATCH '^/repos/o/r/issues/comments/')" 1 "the second run updates the comment"

job pr3-neutral pull_request "$T/pr3/event.json" refs/pull/3/merge review=neutral
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(count POST '/check-runs$')" 1 "check runs"
PR3_NEUTRAL_RUN=$(requests POST '/check-runs$')
equals "$(jq -r .body.name <<<"$PR3_NEUTRAL_RUN")" aver-review "check run name"
equals "$(jq -r .body.conclusion <<<"$PR3_NEUTRAL_RUN")" neutral "check run conclusion"
equals "$(jq -r .body.head_sha <<<"$PR3_NEUTRAL_RUN")" "$PR3_HEAD" "check run head_sha"
passed

FLOW="4 push to main records the base, a later pull request reuses it"
BEFORE=$(git -C "$DEV" rev-parse origin/main)
git -C "$DEV" fetch -q origin refs/pull/1/merge
AFTER=$(git -C "$DEV" rev-parse FETCH_HEAD)
git -C "$DEV" push -q origin "$AFTER:refs/heads/main"
jq -n --arg before "$BEFORE" --arg after "$AFTER" \
  '{ref: "refs/heads/main", before: $before, after: $after, repository: {full_name: "o/r", default_branch: "main"}}' >"$T/push.json"
job push push "$T/push.json" refs/heads/main
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out verdict)" PASS verdict
equals "$(step 'Save the record')" success "Save the record"
PUSH_KEY=$(step_out 'Compute the base record keys' head)
[ -n "$PUSH_KEY" ] && [ -f "$T/cache/$PUSH_KEY/0/record.json" ] || fail "no record saved under the head key '$PUSH_KEY'"
RECORDED=$(sed -n 's/^aver: recorded \([0-9]*\) runs of the head.*/\1/p' "$J/log")
[ -n "$RECORDED" ] && [ "$RECORDED" -gt 0 ] || fail "the push recorded '${RECORDED:-no}' runs of the head"
equals "$(count POST 'comments')" 0 "comments on a push"

open_pr 4 edit_mul "$T/body-mul.md"
job pr4 pull_request "$T/pr4/event.json" refs/pull/4/merge
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out verdict)" PASS verdict
equals "$(step_out 'Compute the base record keys' base)" "$PUSH_KEY" "the pull request's base key"
equals "$(step_out 'Restore the base record' cache-hit)" true "base record cache hit"
contains "$J/summary" "The base came from a recorded run: 1 base run was recorded by an earlier job"
contains "$J/summary" "0 base runs started on this runner"
passed

FLOW="5 merge_group, regression-only"
merge_group() { # <name> <edit function> -> event file in $T/<name>.json
  git -C "$DEV" fetch -q origin main
  git -C "$DEV" checkout -q --detach origin/main
  (cd "$DEV" && "$2")
  git -C "$DEV" add -A
  git -C "$DEV" commit -qm "merge group $1"
  local base head ref
  base=$(git -C "$DEV" rev-parse origin/main)
  head=$(git -C "$DEV" rev-parse HEAD)
  ref=refs/heads/gh-readonly-queue/main/pr-9-$base
  git -C "$DEV" push -q -f origin "HEAD:$ref"
  MG_REF=$ref
  jq -n --arg base "$base" --arg head "$head" --arg ref "$ref" \
    '{merge_group: {base_sha: $base, head_sha: $head, head_ref: $ref, base_ref: "refs/heads/main"},
      repository: {full_name: "o/r", default_branch: "main"}}' >"$T/$1.json"
}
merge_group mg-review edit_expectation
job mg-review merge_group "$T/mg-review.json" "$MG_REF"
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out exit-code)" 0 exit-code
equals "$(receipt '.receipt.criteria | length')" 0 "criteria read in a merge queue"
equals "$(count POST '')" 0 "requests from a merge_group job"

merge_group mg-regression edit_regression
job mg-regression merge_group "$T/mg-regression.json" "$MG_REF"
common_job_checks
equals "$(cat "$J/exit")" 1 "job exit"
equals "$(out verdict)" FAIL_OWN verdict
equals "$(out exit-code)" 1 exit-code
passed

FLOW="6 pull_request_review, mode: approve"
review_event() { # <review id> -> $T/review.json for pull request 3
  jq --argjson id "$1" '. + {action: "submitted", review: {id: $id, state: "approved"}}' "$T/pr3/event.json" >"$T/review.json"
}
reviews() { # <json array>
  state --argjson r "$1" '.reviews["3"] = $r'
}
review() { # <id> <login> <commit> [association]
  jq -n --argjson id "$1" --arg login "$2" --arg commit "$3" --arg assoc "${4:-MEMBER}" \
    '{id: $id, state: "APPROVED", commit_id: $commit, user: {login: $login, type: "User"}, author_association: $assoc}'
}
ARTIFACT=aver-approval

reviews "[$(review 50 alice "$PR3_HEAD")]"
review_event 50
job approve-ok pull_request_review "$T/review.json" refs/pull/3/merge mode=approve
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out approved)" true approved
equals "$(count GET '^/repos/o/r/pulls/3/reviews')" 1 "reviews read"
equals "$(count POST '/check-runs$')" 1 "check runs"
APPROVE_RUN=$(requests POST '/check-runs$')
equals "$(jq -r .body.name <<<"$APPROVE_RUN")" Aver "check run name"
equals "$(jq -r .body.conclusion <<<"$APPROVE_RUN")" success "check run conclusion"
equals "$(jq -r .body.head_sha <<<"$APPROVE_RUN")" "$PR3_HEAD" "check run head_sha"
equals "$(jq -r '.body.output.title' <<<"$APPROVE_RUN")" "Aver: approved by alice" "check run title"
equals "$(jq -r .head_commit "$J/artifacts/aver-approval/approval.json")" "$PR3_HEAD" "approval.json head"
equals "$(step 'Run aver ci')" skipped "Run aver ci in mode: approve"

reviews "[$(review 51 alice "$PR1_HEAD")]"
review_event 51
job approve-stale pull_request_review "$T/review.json" refs/pull/3/merge mode=approve
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out approved)" false approved
contains "$J/log" "::notice title=Aver: not approved::review 51 by alice approved $PR1_HEAD, not the current head $PR3_HEAD"
equals "$(count POST '/check-runs$')" 0 "check runs"

reviews "[$(review 52 author "$PR3_HEAD" OWNER)]"
review_event 52
job approve-author pull_request_review "$T/review.json" refs/pull/3/merge mode=approve
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out approved)" false approved
contains "$J/log" "the pull request's author"
equals "$(count POST '/check-runs$')" 0 "check runs"
passed

FLOW="7 the workflow aver init --ci writes, on a pull request"
# Its verify job, read from the generated file: the checkout's settings,
# its run: steps as a script, and the action's inputs. The checkout is the
# pull request's head, not refs/pull/<n>/merge, so aver ci runs the checks
# in the job's checkout with what the setup steps installed.
GEN=$T/generated
git clone -q "file://$ORIGIN" "$GEN"
rm "$GEN/.github/workflows/aver.yml"
(cd "$GEN" && AVER_HOME=$T/init-home "$AVER" init --ci --yes >"$T/init.log" 2>&1) || { cat "$T/init.log" >&2; fail "aver init --ci failed"; }
mkdir -p "$T/gen/.github/workflows" "$T/gen/action/test"
cp "$GEN/.github/workflows/aver.yml" "$T/gen/.github/workflows/aver.yml"
cp "$HERE/check-workflows.sh" "$T/gen/action/test/"
"$T/gen/action/test/check-workflows.sh" >"$T/check.log" 2>&1 || fail "check-workflows.sh rejects the generated workflow: $(cat "$T/check.log")"
python3 - "$GEN/.github/workflows/aver.yml" "$T/gen-setup.sh" "$T/gen-inputs" <<'EOF'
import sys, yaml
wf = yaml.safe_load(open(sys.argv[1]))
on = wf[True] if True in wf else wf["on"]
assert {"pull_request", "pull_request_review", "push", "merge_group"} <= set(on), on
steps = wf["jobs"]["verify"]["steps"]
checkout = steps[0]
assert checkout["uses"].startswith("actions/checkout@"), checkout
w = checkout["with"]
assert w["ref"] == "${{ github.event.pull_request.head.sha || github.sha }}", w
assert w["fetch-depth"] == 0 and w["persist-credentials"] is False, w
action = steps[-1]
assert action["uses"].startswith("wonkylamp/aver/action@"), action
with open(sys.argv[2], "w") as f:
    for s in steps[1:-1]:
        assert "run" in s, s
        f.write("(cd %s && %s)\n" % (s.get("working-directory", "."), s["run"]))
with open(sys.argv[3], "w") as f:
    for k, v in (action.get("with") or {}).items():
        f.write("%s=%s\n" % (k, v))
EOF
open_pr 7 edit_mul "$T/body-mul.md"
GEN_INPUTS=()
while IFS= read -r input; do GEN_INPUTS+=("$input"); done <"$T/gen-inputs"
J_CHECKOUT=$PR_HEAD J_FULL=1 J_SETUP=$T/gen-setup.sh job pr7 pull_request "$T/pr7/event.json" refs/pull/7/merge "${GEN_INPUTS[@]}"
common_job_checks
equals "$(cat "$J/exit")" 0 "job exit"
equals "$(out verdict)" PASS verdict
lacks "$J/log" "running the checks in slots"
equals "$(count POST '/check-runs$')" 1 "check runs"
equals "$(requests POST '/check-runs$' | jq -r .body.conclusion)" success "check run conclusion"
passed

echo "e2e-local: every flow passed in $((SECONDS - STARTED))s"
