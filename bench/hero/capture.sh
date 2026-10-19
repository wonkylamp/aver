#!/bin/bash
# Reproduces bench/hero/capture.json: a plain `git worktree add` + `npm ci` per
# agent against `aver workspace new --count 4`, the .skip sabotage on both sides,
# Aver's restore and fix, and (when Docker has node:22-bookworm-slim) a
# container per agent. REPS=5 by default; medians are taken in aggregate.py.
# Uses the installed aver (AVER=…), a fresh AVER_HOME per rep, never ~/.aver.
set -euo pipefail
export PATH=/opt/homebrew/bin:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
# A user's terminal, not an agent's: Aver changes some behavior under these.
unset CLAUDECODE CLAUDE_CODE_ENTRYPOINT AVER_RUN AVER_WORKSPACE CODEX_SANDBOX AVER_URL || true
here=$(cd "$(dirname "$0")" && pwd)
AVER=${AVER:-$HOME/.local/bin/aver}
REPS=${REPS:-5}
ROOT=${ROOT:-/tmp/aver-hero/runs}
IMAGE=${IMAGE:-node:22-bookworm-slim}
CRIT="div: divide returns the quotient and throws on division by zero"
MIN_FREE=$((15 * 1024 * 1024 * 1024))

rm -rf "$ROOT"; mkdir -p "$ROOT"
J=$ROOT/steps.jsonl; D=$ROOT/disk.jsonl; : > "$J"; : > "$D"
step() { python3 "$here/step.py" "$J" "$@"; }
free() { python3 "$here/step.py" free; }
disk() { echo "{\"rep\":$1,\"name\":\"$2\",\"before\":$3,\"after\":$4}" >> "$D"; }
watchdog() {
  local f; f=$(free)
  if [ "$f" -lt "$MIN_FREE" ]; then echo "free space under 15 GB ($f bytes): stopping" >&2; exit 3; fi
}

docker_ok=0
if docker info >/dev/null 2>&1 && docker image inspect "$IMAGE" >/dev/null 2>&1; then docker_ok=1; fi

for i in $(seq 1 "$REPS"); do
  watchdog
  R=$ROOT/rep$i; mkdir -p "$R"
  export AVER_HOME=$R/home
  PATH="$(dirname "$AVER"):$PATH" "$here/make-repo.sh" "$R/repo" >/dev/null

  # Left: plain git worktrees, each with its own npm ci (warm npm cache, offline).
  f0=$(free)
  for k in 1 2 3 4; do step "$i" git "worktree-add-$k" "$R/repo" "git worktree add ../wt$k -b agent$k"; done
  f1=$(free)
  for k in 1 2 3 4; do step "$i" git "npm-ci-$k" "$R/wt$k" "npm ci --offline --no-audit --no-fund"; done
  f2=$(free)
  disk "$i" git-worktrees "$f0" "$f1"; disk "$i" git-npm-ci "$f1" "$f2"; disk "$i" git-total "$f0" "$f2"
  step "$i" git npm-test-before "$R/wt1" "npm test" >/dev/null
  (cd "$R/wt1" && "$here/sabotage.sh")
  step "$i" git git-diff-sabotage "$R/wt1" "git --no-pager diff" >/dev/null
  step "$i" git npm-test-sabotaged "$R/wt1" "npm test" >/dev/null

  # Right: Aver. The first workspace new imports the commit (cold); the timed one is the second.
  step "$i" aver workspace-new-cold "$R/repo" "$AVER workspace new --count 1 --label warmup" >/dev/null
  f3=$(free)
  step "$i" aver workspace-new-4 "$R/repo" "$AVER workspace new --count 4 -c '$CRIT'" >/dev/null
  f4=$(free)
  disk "$i" aver-workspaces "$f3" "$f4"
  # Noise control: the same free-space reading around an idle interval as long as the step above.
  f7=$(free); sleep 0.3; f8=$(free); disk "$i" idle-control "$f7" "$f8"
  ws=$(tail -1 "$J" | python3 -c 'import json,sys; print([l for l in json.load(sys.stdin)["output"].splitlines() if l.startswith("/")][0])')
  step "$i" aver verify-baseline "$ws" "$AVER verify" >/dev/null
  (cd "$ws" && "$here/sabotage.sh")
  step "$i" aver npm-test-sabotaged "$ws" "npm test" >/dev/null
  step "$i" aver verify-sabotaged "$ws" "$AVER verify" >/dev/null
  step "$i" aver restore-last-green "$ws" "$AVER restore --last-green" >/dev/null
  (cd "$ws" && "$here/fix.sh")
  step "$i" aver verify-fixed "$ws" "$AVER verify" >/dev/null
  step "$i" aver accept "$ws" "$AVER accept" >/dev/null

  # Docker: a container per agent, the repo copied in, npm ci from the registry inside.
  if [ "$docker_ok" = 1 ]; then
    watchdog
    f5=$(free)
    for k in 1 2 3 4; do
      c=aver-hero-$i-$k
      step "$i" docker "container-$k" "$R/repo" "docker run -d --name $c $IMAGE sleep infinity >/dev/null && docker exec $c mkdir -p /app && git archive --format=tar HEAD | docker cp - $c:/app && docker exec -w /app $c npm ci --no-audit --no-fund" >/dev/null
    done
    f6=$(free)
    disk "$i" docker-containers "$f5" "$f6"
    step "$i" docker container-sizes "$R/repo" "docker ps -a -s --filter name=aver-hero-$i- --format '{{.Names}}\t{{.Size}}'" >/dev/null
    docker rm -f $(docker ps -aq --filter name=aver-hero-$i-) >/dev/null
  fi
done

python3 "$here/aggregate.py" "$ROOT" "$AVER" "$IMAGE" "$docker_ok" > "$here/capture.json"
echo "wrote $here/capture.json" >&2
