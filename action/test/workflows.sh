#!/usr/bin/env bash
# Runs this repository's ci.yml, release.yml and aver.yml as GitHub would, with no
# network: run-workflow.py runs each job's steps against a bare repository
# holding this tree (the checkouts' origin), a mock GitHub API
# (mock-github.py: check runs, compare, refs, releases, pulls) and stub
# toolchains (cargo builds a fake aver that answers --version and demo).
# Each flow is one workflow run on a fake tag or event, with assertions on
# the jobs, the release, the tags, the tap and every request.
#
# release.yml: a release end to end (draft, provenance, install, publish,
# tap, series tag, plugin pull request, each write from a job allowed to),
# a re-run after a failed install, a prerelease, the gate's refusals (tag
# and version differ, ci-ok missing, failed or from another app, a commit
# off main, a run on a branch), and a series tag that starts no release.
# ci.yml's tag job, with CI's own jobs stubbed and release-plz's action
# replaced by a stand-in with its inputs and outputs: a merge that changes
# no version, a version bump that tags and dispatches a release (then run),
# the same push again, a bump whose CI failed, and a version that went
# down. aver.yml: a pull request, a review, a push to main, and a push
# elsewhere.
set -euo pipefail

if [ "$(uname)" = Darwin ] && [ -d /opt/homebrew/bin ]; then
  PATH=/opt/homebrew/bin:$PATH
fi

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/aver-workflows.XXXXXX")
T=$(cd "$T" && pwd -P)
ORIGIN=$T/origin.git
SRC=$T/src
STARTED=$SECONDS

cleanup() {
  local code=$?
  stop_mock
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
	name = Aver Workflows
	email = workflows@example.invalid
[init]
	defaultBranch = main
[advice]
	detachedHead = false
[protocol "file"]
	allow = always
EOF

# Stubs: gh is the mock's shim; the toolchains log what they were asked.
mkdir -p "$T/bin"
STUB_LOG=$T/stub.log
export STUB_LOG
printf '#!/usr/bin/env bash\nexec python3 %q gh "$@"\n' "$HERE/mock-github.py" >"$T/bin/gh"
for tool in rustup sudo npm; do
  printf '#!/bin/sh\necho "%s $* (in $PWD)" >>"$STUB_LOG"\n' "$tool" >"$T/bin/$tool"
done
# What `file` says of a static musl binary; the fakes are shell scripts.
printf '#!/bin/sh\necho "ELF 64-bit LSB executable, x86-64, version 1 (SYSV), statically linked, stripped"\n' >"$T/bin/file"
cat >"$T/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "cargo $* (in $PWD)" >>"$STUB_LOG"
[ "${1:-}" = build ] || exit 0
target="" prev=""
for a in "$@"; do
  [ "$prev" = --target ] && target=$a
  prev=$a
done
version=$(awk '/^\[workspace\.package\]/ { p = 1; next } /^\[/ { p = 0 } p && $1 == "version" { gsub(/"/, "", $3); print $3; exit }' Cargo.toml)
out=${CARGO_TARGET_DIR:-target}/$target/release
mkdir -p "$out"
cat >"$out/aver" <<FAKE
#!/bin/sh
# fake aver $version for $target, action $AVER_ACTION_SHA
case "\$1" in
  --version) echo "aver $version" ;;
  demo) echo "   NOT DONE: FAIL_OWN: fix the NEW failures above"; echo "   DONE: PASS" ;;
  *) exit 2 ;;
esac
FAKE
chmod +x "$out/aver"
EOF
chmod +x "$T/bin/"*
export PATH="$T/bin:$PATH"
unset CARGO_TARGET_DIR AVER_HOME AVER_URL CI GITHUB_ACTIONS GH_TOKEN GITHUB_TOKEN

fail() {
  echo "FAIL [$FLOW]: $*" >&2
  [ -z "${J:-}" ] || { echo "--- log $J/log" >&2; tail -n 80 "$J/log" >&2; }
  exit 1
}
contains() { grep -qF -- "$2" "$1" || fail "$(basename "$1") does not contain '$2'"; }
lacks() { ! grep -qF -- "$2" "$1" || fail "$(basename "$1") contains '$2'"; }
equals() { [ "$1" = "$2" ] || fail "${3:-value}: expected '$2', got '$1'"; }
result() { jq -r --arg j "$1" '.jobs[$j].result' "$J/result.json"; }
results() { jq -r '[.jobs | to_entries[] | "\(.key)=\(.value.result)"] | join(" ")' "$J/result.json"; }
runs() { jq -r --arg j "$1" '.jobs[$j].runs | length' "$J/result.json"; }
step() { jq -r --arg j "$1" --arg n "$2" '[.jobs[$j].runs[].steps[] | select(.name == $n) | .outcome] | unique | join(",")' "$J/result.json"; }
count() { jq -c --arg m "$1" --arg p "$2" 'select(.method == $m and (.path | test($p)))' "$J/requests.jsonl" | wc -l | tr -d ' '; }
writes() { jq -c 'select(.method != "GET")' "$J/requests.jsonl" | wc -l | tr -d ' '; }
release_view() { GH_REPO=o/r GH_TOKEN=reader gh release view "$1" --json isDraft,isPrerelease,assets --jq "$2"; }

state() { # <jq filter> [args] edits the mock's state.json
  jq "$@" "$MOCK/state.json" >"$MOCK/state.tmp" && mv "$MOCK/state.tmp" "$MOCK/state.json"
}

# A fresh mock per flow: no releases, pulls or published files.
start_mock() {
  stop_mock
  MOCK=$T/mock.$1
  mkdir -p "$MOCK/www"
  jq -n --arg git "$ORIGIN" '{pulls: {}, reviews: {}, check_runs: {}, git: $git}' >"$MOCK/state.json"
  : >"$MOCK/requests.jsonl"
  python3 "$HERE/mock-github.py" serve "$MOCK" 2>"$MOCK/server.log" &
  MOCK_PID=$!
  for _ in $(seq 1 100); do
    [ -s "$MOCK/port" ] && break
    sleep 0.05
  done
  [ -s "$MOCK/port" ] || { echo "the mock GitHub did not start: $(cat "$MOCK/server.log")" >&2; exit 2; }
  export MOCK_GITHUB_URL="http://127.0.0.1:$(cat "$MOCK/port")"
}
stop_mock() {
  if [ -n "${MOCK_PID:-}" ]; then
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
    MOCK_PID=""
  fi
}

# Origin's main: the part of this tree the workflows read, as committed or
# not, minus what Git ignores (the crates' sources stay out: cargo is a stub).
git init -q --bare "$ORIGIN"
mkdir -p "$SRC"
(cd "$ROOT" && git ls-files -z --cached --others --exclude-standard -- .github action packaging scripts site/install.sh \
  site/package.json site/package-lock.json Cargo.toml Cargo.lock 'crates/*/Cargo.toml' release-plz.toml .claude-plugin integrations/claude-code \
  aver.toml README.md LICENSE | while IFS= read -r -d '' f; do
  [ -e "$f" ] && printf '%s\0' "$f"
done | xargs -0 tar -cf -) | tar -xf - -C "$SRC"
git -C "$SRC" init -q
git -C "$SRC" add -A
git -C "$SRC" commit -qm "aver"
git -C "$SRC" remote add origin "file://$ORIGIN"
git -C "$SRC" push -q origin HEAD:refs/heads/main

# Commits Cargo.toml's version <v> on <branch> (default main) and tags it
# <tag> (default v<v>; - for none). Sets SHA.
release_commit() { # <version> [tag] [branch]
  local version=$1 tag=${2:-v$1} branch=${3:-main}
  git -C "$SRC" fetch -q origin main
  git -C "$SRC" checkout -q --detach origin/main
  sed -i.bak "/^\[workspace.package\]/,/^\[/ s/^version = \".*\"/version = \"$version\"/" "$SRC/Cargo.toml" && rm "$SRC/Cargo.toml.bak"
  git -C "$SRC" commit -qam "aver $version"
  SHA=$(git -C "$SRC" rev-parse HEAD)
  if [ "$tag" = - ]; then
    git -C "$SRC" push -q origin "HEAD:refs/heads/$branch"
  else
    git -C "$SRC" push -q origin "HEAD:refs/heads/$branch" "HEAD:refs/tags/$tag"
  fi
}

ci_ok() { # <sha> <status> <conclusion> [app slug]
  state --arg sha "$1" --arg s "$2" --arg c "$3" --arg app "${4:-github-actions}" \
    '.check_runs[$sha] += [{name: "ci-ok", head_sha: $sha, status: $s, conclusion: $c, app: {slug: $app}}]'
}

# Runs <workflow> for one event. Sets J to the flow's directory: log,
# result.json, requests.jsonl, artifacts/, attestations.jsonl, exit.
run_workflow() { # <workflow> <event> <ref> <sha> [event payload file]
  J=$T/flows/$FLOW
  rm -rf "$J"
  mkdir -p "$J/artifacts" "$J/cache"
  local mark
  mark=$(wc -l <"$MOCK/requests.jsonl")
  set +e
  GITHUB_EVENT_NAME=$2 GITHUB_REF=$3 GITHUB_SHA=$4 GITHUB_EVENT_PATH=${5:-} GITHUB_REPOSITORY=o/r \
    WORKFLOW_REPO=$ORIGIN RUNNER_TEMP=$J E2E_ARTIFACTS_DIR=$J/artifacts E2E_CACHE_DIR=$J/cache \
    python3 "$HERE/run-workflow.py" "$ROOT/.github/workflows/$1" "$J/result.json" >"$J/log" 2>&1
  echo $? >"$J/exit"
  set -e
  tail -n +"$((mark + 1))" "$MOCK/requests.jsonl" >"$J/requests.jsonl"
  [ -f "$J/result.json" ] || fail "run-workflow.py wrote no result"
}

release() { # <tag> [secrets JSON]
  WORKFLOW_SECRETS=${2:-'{}'} AVER_RELEASE_BASE_URL="file://$MOCK/www/o/r/releases" \
    TAP_URL="file://$T/tap.git" REPO_URL="file://$ORIGIN" \
    run_workflow release.yml push "refs/tags/$1" "$(git -C "$ORIGIN" rev-parse "$1^{commit}")"
}

# Every write a job's own token sent came from a job granted that scope
# (releases and refs: contents; check runs: checks; comments:
# pull-requests), and every request carried a token.
least_privilege() {
  local token path job scope
  equals "$(jq -c 'select(.token == "" and (.path | startswith("/repos/")))' "$J/requests.jsonl" | wc -l | tr -d ' ')" 0 "API requests without a token"
  while read -r token path; do
    job=${token#token-}
    case "$path" in
      */releases* | */git/refs* | */git/tags*) scope=contents ;;
      */actions/workflows/*/dispatches) scope=actions ;;
      */check-runs*) scope=checks ;;
      */issues/*) scope=pull-requests ;;
      *) fail "a write to $path the check does not know" ;;
    esac
    equals "$(jq -r --arg j "$job" --arg s "$scope" '.jobs[$j].permissions[$s] // "none"' "$J/result.json")" write "$scope permission of $job, which wrote $path"
  done < <(jq -r 'select(.method != "GET" and (.token | startswith("token-"))) | "\(.token) \(.path)"' "$J/requests.jsonl" | sort -u)
}

# The gate refused: every later job skipped, nothing written.
refused() { # <message>
  equals "$(cat "$J/exit")" 1 "exit"
  equals "$(result gate)" failure "gate"
  equals "$(results)" "gate=failure build=skipped draft=skipped install=skipped publish=skipped tokens=skipped homebrew=skipped action-tag=skipped plugin=skipped" "jobs"
  contains "$J/log" "::error title=Release refused::$1"
  equals "$(writes)" 0 "writes to the API"
}

passed() { echo "ok $FLOW ($((SECONDS - FLOW_START))s)"; FLOW_START=$SECONDS; }
FLOW_START=$SECONDS

git init -q --bare "$T/tap.git"
git init -q "$T/tap-src"
echo "wonkylamp/homebrew-tap" >"$T/tap-src/README.md"
git -C "$T/tap-src" add README.md
git -C "$T/tap-src" commit -qm "tap"
git -C "$T/tap-src" push -q "file://$T/tap.git" HEAD:refs/heads/main

SECRETS='{"HOMEBREW_TAP_TOKEN": "tap-secret", "RELEASE_PR_TOKEN": "pr-secret"}'

FLOW=release
start_mock release
release_commit 0.99.0
ci_ok "$SHA" completed success
release v0.99.0 "$SECRETS"
equals "$(cat "$J/exit")" 0 "exit"
equals "$(results)" "gate=success build=success draft=success install=success publish=success tokens=success homebrew=success action-tag=success plugin=success" "jobs"
equals "$(runs build)" 4 "build runs"
equals "$(runs install)" 3 "install runs"
equals "$(jq -r '.jobs.gate.outputs | "\(.version) \(.prerelease) \(.series)"' "$J/result.json")" "0.99.0 false v0.99" "gate outputs"
equals "$(release_view v0.99.0 '"\(.isDraft) \(.isPrerelease) \(.assets | length)"')" "false false 9" "the release"
equals "$(count POST '^/repos/o/r/releases$')" 1 "releases created"
equals "$(jq -c 'select(.method == "POST" and .path == "/repos/o/r/releases") | .body.draft' "$J/requests.jsonl")" true "created as a draft"
equals "$(count GET '^/repos/o/r/releases/assets/')" 27 "draft assets downloaded by the three install runners"
www=$MOCK/www/o/r/releases
(cd "$www/download/v0.99.0" && shasum -a 256 -c SHA256SUMS >/dev/null) || fail "the published SHA256SUMS does not match its assets"
cmp -s "$www/download/v0.99.0/SHA256SUMS" "$www/latest/download/SHA256SUMS" || fail "latest is not v0.99.0"
contains "$www/download/v0.99.0/aver-linux-aarch64" "fake aver 0.99.0 for aarch64-unknown-linux-musl, action $SHA"
contains "$www/download/v0.99.0/aver-darwin-x86_64" "for x86_64-apple-darwin"
equals "$(jq -r '.subjects | map("\(.sha256)  \(.name)") | .[]' "$J/attestations.jsonl")" "$(cat "$www/download/v0.99.0/SHA256SUMS")" "attested subjects"
equals "$(jq -r .job "$J/attestations.jsonl")" draft "the attesting job"
for s in "Install the published release" "Install the latest release"; do
  equals "$(step publish "$s")" success "$s"
done
equals "$(git -C "$ORIGIN" rev-parse v0.99)" "$SHA" "the series tag"
equals "$(count POST '^/repos/o/r/git/refs$')" 1 "series tag created"
git -C "$T/tap.git" show main:Formula/aver.rb >"$J/formula.rb"
contains "$J/formula.rb" 'version "0.99.0"'
contains "$J/formula.rb" "sha256 \"$(awk '$2 == "aver-linux-x86_64" { print $1 }' "$www/download/v0.99.0/SHA256SUMS")\""
equals "$(git -C "$T/tap.git" log -1 --format=%s main)" "aver 0.99.0" "tap commit"
equals "$(jq -r 'select(.method == "POST" and .path == "/repos/o/r/pulls") | "\(.token) \(.body.head) \(.body.base)"' "$J/requests.jsonl")" "pr-secret release/plugin-0.99.0 main" "plugin pull request"
for f in integrations/claude-code/.claude-plugin/plugin.json .claude-plugin/marketplace.json; do
  git -C "$ORIGIN" show "release/plugin-0.99.0:$f" | grep -qF '"version": "0.99.0"' || fail "$f is not at 0.99.0 on the plugin branch"
done
equals "$(git -C "$ORIGIN" rev-parse release/plugin-0.99.0^)" "$SHA" "the plugin branch's parent"
contains "$STUB_LOG" "sudo apt-get install -y --no-install-recommends musl-tools"
contains "$STUB_LOG" "cargo build --release --locked -p aver --target x86_64-unknown-linux-musl"
least_privilege
passed

# The install runners cannot download the draft: nothing is published, and
# re-running the workflow replaces the draft's assets and publishes it.
# Without the optional tokens the tap and plugin jobs are skipped.
FLOW=rerun-after-a-failed-install
start_mock rerun
release_commit 0.99.1
ci_ok "$SHA" completed success
state '.fail_asset_downloads = true'
release v0.99.1
equals "$(cat "$J/exit")" 1 "exit"
equals "$(results)" "gate=success build=success draft=success install=failure publish=skipped tokens=skipped homebrew=skipped action-tag=skipped plugin=skipped" "jobs"
equals "$(release_view v0.99.1 .isDraft)" true "still a draft"
[ ! -e "$MOCK/www/o/r/releases/download/v0.99.1" ] || fail "a draft's assets were published"
state '.fail_asset_downloads = false'
release v0.99.1
equals "$(cat "$J/exit")" 0 "exit"
equals "$(results)" "gate=success build=success draft=success install=success publish=success tokens=success homebrew=skipped action-tag=success plugin=skipped" "jobs"
contains "$J/log" "Replaced the assets of the draft release v0.99.1"
equals "$(count POST '^/repos/o/r/releases$')" 0 "releases created by the re-run"
equals "$(count DELETE '^/repos/o/r/releases/assets/')" 9 "assets replaced"
equals "$(release_view v0.99.1 '"\(.isDraft) \(.assets | length)"')" "false 9" "the release"
contains "$J/log" "::notice title=Homebrew tap not updated::HOMEBREW_TAP_TOKEN is not set"
contains "$J/log" "::notice title=Plugin version not bumped::RELEASE_PR_TOKEN is not set"
equals "$(git -C "$ORIGIN" rev-parse v0.99)" "$SHA" "the series tag moved"
equals "$(count PATCH '^/repos/o/r/git/refs/tags/v0.99$')" 1 "series tag moved"
least_privilege
passed

FLOW=prerelease
start_mock prerelease
release_commit 0.100.0-rc.1
ci_ok "$SHA" completed success
release v0.100.0-rc.1 "$SECRETS"
equals "$(cat "$J/exit")" 0 "exit"
equals "$(results)" "gate=success build=success draft=success install=success publish=success tokens=success homebrew=skipped action-tag=skipped plugin=skipped" "jobs"
equals "$(release_view v0.100.0-rc.1 '"\(.isDraft) \(.isPrerelease)"')" "false true" "the release"
equals "$(step publish 'Install the latest release')" skipped "the latest install"
[ ! -e "$MOCK/www/o/r/releases/latest" ] || fail "a prerelease became the latest release"
git -C "$ORIGIN" rev-parse -q --verify refs/tags/v0.100 >/dev/null && fail "a prerelease moved the series tag"
passed

FLOW=refuse-tag-not-version
start_mock refusals
release_commit 0.99.2 v0.99.3
ci_ok "$SHA" completed success
release v0.99.3 "$SECRETS"
refused "the tag v0.99.3 does not name the version in Cargo.toml (v0.99.2)"
passed

FLOW=refuse-no-ci-ok
release_commit 0.99.4
release v0.99.4 "$SECRETS"
refused "$SHA has no ci-ok check run. Run CI on the tag (gh workflow run ci.yml --ref v0.99.4)"
passed

FLOW=refuse-ci-ok-failed
ci_ok "$SHA" completed failure
release v0.99.4 "$SECRETS"
refused "ci-ok on $SHA is completed/failure, not completed/success"
passed

FLOW=refuse-ci-ok-from-another-app
release_commit 0.99.5
ci_ok "$SHA" completed success not-actions
release v0.99.5 "$SECRETS"
refused "$SHA has no ci-ok check run"
passed

FLOW=refuse-off-main
release_commit 0.99.6 v0.99.6 side
ci_ok "$SHA" completed success
release v0.99.6 "$SECRETS"
refused "v0.99.6 ($SHA) is not on main (compared with main: ahead)"
passed

FLOW=series-tag-starts-nothing
release v0.99
equals "$(jq -r .triggered "$J/result.json")" false "triggered"
equals "$(wc -l <"$J/requests.jsonl" | tr -d ' ')" 0 "requests"
passed

FLOW=refuse-dispatch-on-a-branch
run_workflow release.yml workflow_dispatch refs/heads/main "$(git -C "$ORIGIN" rev-parse main)"
refused "refs/heads/main is not a tag. Run the release on its tag: gh workflow run release.yml --ref v0.99.5"
passed

# ci.yml on a push to main. CI's own jobs are stubbed (STUBS); ci-ok and
# tag run, and every job leaves its check run in the mock as GitHub would.
# release-plz's action is a stand-in with the real one's inputs and
# outputs, whose release-plz follows the documented git_only rules: the
# packages release-plz.toml does not exclude, tagged git_tag_name through
# the API (an annotated tag, then the ref) unless that tag exists.
mkdir -p "$T/release-plz-action"
cat >"$T/release-plz-action/action.yml" <<'YAML'
name: release-plz stand-in
inputs:
  command:
    required: false
  config:
    required: false
outputs:
  releases:
    value: ${{ steps.release-plz.outputs.releases }}
  releases_created:
    value: ${{ steps.release-plz.outputs.releases_created }}
runs:
  using: composite
  steps:
    - id: release-plz
      shell: bash
      run: |
        [ "${{ inputs.command }}" = release ] || { echo "the stand-in runs only release, not '${{ inputs.command }}'"; exit 1; }
        out=$(release-plz release --git-token "${GITHUB_TOKEN}" --forge github -o json)
        releases=$(jq -c .releases <<<"$out")
        echo "releases=$releases" >>"$GITHUB_OUTPUT"
        if [ "$(jq length <<<"$releases")" != 0 ]; then
          echo "releases_created=true" >>"$GITHUB_OUTPUT"
        else
          echo "releases_created=false" >>"$GITHUB_OUTPUT"
        fi
YAML
cp "$HERE/release-plz-stand-in.py" "$T/bin/release-plz"
chmod +x "$T/bin/release-plz"
STUBS='{"changes": "success", "docs": "success", "lint": "success", "test": "success", "eval": "success", "test-native": "success"}'
ci_push() { # <sha> [stubbed job results]
  jq -n --arg before "$(git -C "$ORIGIN" rev-parse "$1^")" --arg after "$1" \
    '{ref: "refs/heads/main", before: $before, after: $after, repository: {full_name: "o/r", default_branch: "main"}}' >"$T/push-ci.json"
  WORKFLOW_STUB_JOBS=${2:-$STUBS} WORKFLOW_CHECK_RUNS=$MOCK/state.json \
    WORKFLOW_ACTIONS=$(jq -cn --arg dir "$T/release-plz-action" '{"release-plz/action": $dir}') \
    run_workflow ci.yml push refs/heads/main "$1" "$T/push-ci.json"
}
no_tag() { # <tag>
  ! git -C "$ORIGIN" rev-parse -q --verify "refs/tags/$1" >/dev/null || fail "$1 was tagged"
  equals "$(count POST '/git/(tags|refs)$')" 0 "tags created"
  equals "$(count POST '/dispatches$')" 0 "releases dispatched"
}

FLOW=merge-without-a-bump
start_mock merge
git -C "$SRC" fetch -q origin main
git -C "$SRC" checkout -q --detach origin/main
echo "docs" >>"$SRC/README.md"
git -C "$SRC" commit -qam "docs"
SHA=$(git -C "$SRC" rev-parse HEAD)
git -C "$SRC" push -q origin HEAD:refs/heads/main
ci_push "$SHA"
equals "$(cat "$J/exit")" 0 "exit"
equals "$(result ci-ok) $(result tag)" "success success" "ci-ok and tag"
equals "$(step tag 'Start the release')" skipped "the release start"
contains "$J/log" "release-plz: aver 0.99.5 is already tagged v0.99.5"
no_tag v0.99.5-none
passed

FLOW=merge-a-bump
release_commit 0.101.0 -
ci_push "$SHA"
equals "$(cat "$J/exit")" 0 "exit"
equals "$(result tag)" success "tag"
equals "$(git -C "$ORIGIN" cat-file -t v0.101.0)" tag "an annotated tag"
equals "$(git -C "$ORIGIN" rev-parse 'v0.101.0^{commit}')" "$SHA" "the tagged commit"
equals "$(count POST '^/repos/o/r/git/refs$')" 1 "tags created"
equals "$(jq -c 'select(.path | endswith("/dispatches")) | [.path, .body.ref]' "$J/requests.jsonl")" '["/repos/o/r/actions/workflows/release.yml/dispatches","v0.101.0"]' "the dispatched release"
contains "$J/log" "::notice title=Release started::v0.101.0"
equals "$(jq -r --arg s "$SHA" '.check_runs[$s][] | select(.name == "ci-ok") | .conclusion' "$MOCK/state.json")" success "ci-ok's check run"
least_privilege
WORKFLOW_SECRETS=$SECRETS AVER_RELEASE_BASE_URL="file://$MOCK/www/o/r/releases" TAP_URL="file://$T/tap.git" REPO_URL="file://$ORIGIN" \
  run_workflow release.yml workflow_dispatch refs/tags/v0.101.0 "$SHA"
equals "$(cat "$J/exit")" 0 "the dispatched release's exit"
equals "$(results)" "gate=success build=success draft=success install=success publish=success tokens=success homebrew=success action-tag=success plugin=success" "jobs"
equals "$(release_view v0.101.0 '"\(.isDraft) \(.assets | length)"')" "false 9" "the release"
equals "$(git -C "$ORIGIN" rev-parse 'v0.101^{commit}')" "$SHA" "the series tag"
least_privilege
passed

FLOW=merge-a-bump-again
ci_push "$SHA"
equals "$(cat "$J/exit")" 0 "exit"
equals "$(step tag 'Start the release')" skipped "the release start"
equals "$(writes)" 0 "writes to the API"
passed

FLOW=merge-a-bump-with-red-ci
release_commit 0.102.0 -
ci_push "$SHA" "$(jq -c '.lint = "failure"' <<<"$STUBS")"
equals "$(cat "$J/exit")" 1 "exit"
equals "$(result ci-ok) $(result tag)" "failure skipped" "ci-ok and tag"
no_tag v0.102.0
passed

FLOW=merge-a-lower-version
release_commit 0.98.0 -
ci_push "$SHA"
equals "$(cat "$J/exit")" 1 "exit"
equals "$(result tag)" failure "tag"
contains "$J/log" "::error title=Version went down::Cargo.toml says 0.98.0, lower than the released v0.101.0"
no_tag v0.98.0
passed

# aver.yml with the action from action/ and a stub aver served as the
# release asset of the action's pinned commit.
action_sha=$(grep -oE 'wonkylamp/aver/action@[0-9a-f]{40}' "$ROOT/.github/workflows/aver.yml" | head -1 | cut -d@ -f2)
cat >"$T/stub-aver" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "aver $*" >>"$STUB_LOG"
if [ "${1:-} ${2:-}" = "ci keys" ]; then
  printf 'base=aver-base-b\nhead=aver-base-h\n'
  exit 0
fi
out="" prev=""
for a in "$@"; do
  [ "$prev" = --out-dir ] && out=$a
  [ "$prev" = --record ] && mkdir -p "$a" && echo '{}' >"$a/record.json"
  prev=$a
done
[ -n "$out" ] || exit 0
if [ "${1:-} ${2:-}" = "ci approve" ]; then
  echo "NOT APPROVED: no approving review"
  exit 3
fi
mkdir -p "$out"
printf '{"receipt_id":"rcpt_stub","receipt":{"verdict":"PASS"}}\n' >"$out/receipt.json"
printf '## PASS\n\nstub summary\n' >"$out/summary.md"
printf 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n' >"$out/public-key.txt"
EOF
aver_workflow() { # <event> <ref> <sha> [payload]
  : >"$STUB_LOG"
  WORKFLOW_ACTIONS=$(jq -cn --arg dir "$ROOT/action" '{"wonkylamp/aver/action": $dir}') AVER_RELEASE_BASE_URL=$MOCK_GITHUB_URL \
    run_workflow aver.yml "$@"
}
start_mock aver
asset_dir=$MOCK/www/wonkylamp/aver/releases/download/$action_sha
mkdir -p "$asset_dir"
cp "$T/stub-aver" "$asset_dir/aver-linux-x86_64"
(cd "$asset_dir" && shasum -a 256 aver-linux-x86_64 >aver-linux-x86_64.sha256)
MAIN=$(git -C "$ORIGIN" rev-parse main)
git -C "$SRC" checkout -q --detach "$MAIN"
echo "a change" >>"$SRC/README.md"
git -C "$SRC" commit -qam "pr 7"
PR_HEAD=$(git -C "$SRC" rev-parse HEAD)
git -C "$SRC" push -q origin "HEAD:refs/pull/7/head" "HEAD:refs/heads/pr-7"
pr_payload() { # <action>
  jq -n --arg a "$1" --arg base "$MAIN" --arg head "$PR_HEAD" '{
    action: $a, number: 7, review: {id: 50, state: "approved", user: {login: "alice"}},
    pull_request: {number: 7, body: null, user: {login: "author", type: "User"},
      base: {sha: $base, ref: "main", repo: {full_name: "o/r"}},
      head: {sha: $head, ref: "pr-7", repo: {full_name: "o/r"}}},
    repository: {full_name: "o/r", default_branch: "main"}}' >"$T/event.json"
}
state --arg head "$PR_HEAD" '.pulls["7"] = {number: 7, head: {sha: $head}}'

FLOW=aver-pull-request
pr_payload opened
aver_workflow pull_request refs/pull/7/merge "$PR_HEAD" "$T/event.json"
equals "$(cat "$J/exit")" 0 "exit"
equals "$(results)" "verify=success approve=skipped" "jobs"
contains "$J/log" "Installed aver from wonkylamp/aver@$action_sha"
grep -qE '^npm ci \(in .*/verify\.[^/]*/workspace/site\)$' "$STUB_LOG" || fail "npm ci did not run in site/: $(cat "$STUB_LOG")"
contains "$STUB_LOG" "cargo fetch --locked"
grep -E "^aver ci .*--base $MAIN .*--head $PR_HEAD" "$STUB_LOG" >/dev/null || fail "aver ci did not compare $MAIN with $PR_HEAD: $(cat "$STUB_LOG")"
equals "$(count POST '/check-runs$')" 1 "check runs"
equals "$(count POST '^/repos/o/r/issues/7/comments$')" 1 "comments"
least_privilege
passed

FLOW=aver-review
pr_payload submitted
aver_workflow pull_request_review refs/pull/7/merge "$PR_HEAD" "$T/event.json"
equals "$(cat "$J/exit")" 0 "exit"
equals "$(results)" "verify=skipped approve=success" "jobs"
contains "$STUB_LOG" "aver ci approve"
passed

FLOW=aver-push-main
jq -n --arg before "$(git -C "$ORIGIN" rev-parse "$MAIN^")" --arg after "$MAIN" \
  '{ref: "refs/heads/main", before: $before, after: $after, repository: {full_name: "o/r", default_branch: "main"}}' >"$T/push.json"
aver_workflow push refs/heads/main "$MAIN" "$T/push.json"
equals "$(cat "$J/exit")" 0 "exit"
equals "$(results)" "verify=success approve=skipped" "jobs"
contains "$STUB_LOG" "--record"
equals "$(count POST 'comments')" 0 "comments on a push"
passed

FLOW=aver-push-elsewhere
aver_workflow push refs/heads/pr-7 "$PR_HEAD"
equals "$(jq -r .triggered "$J/result.json")" false "triggered"
passed

echo "workflows: all flows passed in $((SECONDS - STARTED))s"
