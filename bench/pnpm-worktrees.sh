#!/usr/bin/env bash
# The pnpm baseline for bench/scale.sh: `git worktree add` plus
# `pnpm install --frozen-lockfile --offline` on date-fns at the same REV,
# with the pnpm version its package.json pins (pnpm 9 when none) and a store warmed by one
# install first (not counted). date-fns is a pnpm monorepo, so this installs
# its own lockfile, every workspace package included, where scale.sh's npm
# rows install only the root devDependencies.
#
#   bench/pnpm-worktrees.sh               # 5 worktrees
#   WORKTREES=3 KEEP=1 bench/pnpm-worktrees.sh
#
# Like scale.sh it runs on a sparse APFS image of its own and measures disk
# as the image's free-space delta, which counts what pnpm cloned or
# hard-linked from the store (on the same volume) once.
set -euo pipefail
cd "$(dirname "$0")/.."

REV="717ce0a807ea4c6b540d015b5c408723175b2838"
WORKTREES="${WORKTREES:-5}"
MIN_FREE_GIB="${MIN_FREE_GIB:-8}"
HOST="${HOST:-$(mktemp -d "${TMPDIR:-/tmp}/aver-pnpm.XXXXXX")}"
OUT="${OUT:-$HOST/pnpm.tsv}"

hdiutil create -quiet -size "${IMAGE_GIB:-6}g" -type SPARSE -fs APFS -volname aver-pnpm "$HOST/pnpm"
mkdir -p "$HOST/mnt"
hdiutil attach -quiet -nobrowse -mountpoint "$HOST/mnt" "$HOST/pnpm.sparseimage"
SCRATCH="$HOST/mnt"
REPO="$SCRATCH/date-fns"
cleanup() {
  [ -n "${watchdog:-}" ] && kill "$watchdog" 2>/dev/null || true
  hdiutil detach -quiet -force "$HOST/mnt" || true
  [ "${KEEP:-0}" = 1 ] || rm -rf "$HOST"
}
trap cleanup EXIT
trap 'exit 3' TERM INT

(
  while sleep 2; do
    free=$(df -k "$HOST" | awk 'NR==2 {print $4}')
    if [ "$free" -lt $((MIN_FREE_GIB * 1024 * 1024)) ]; then
      echo "# aborted: host free $((free / 1024)) MiB below ${MIN_FREE_GIB} GiB" | tee -a "$OUT" >&2
      kill -TERM $$
      pkill -TERM -P $$ 2>/dev/null
      exit
    fi
  done
) &
watchdog=$!

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
free_kib() { df -k "$SCRATCH" | awk 'NR==2 {print $4}'; }
emit() { printf '%s\n' "$*" | tee -a "$OUT"; }

git init -q "$REPO"
git -C "$REPO" fetch -q --depth 1 https://github.com/date-fns/date-fns "$REV"
git -C "$REPO" checkout -q FETCH_HEAD
pin=$(node -p 'require(process.argv[1]).packageManager || ""' "$REPO/package.json")
case "$pin" in pnpm@*) PNPM="${pin%%+*}" ;; *) PNPM="pnpm@9" ;; esac # date-fns at REV pins none
export npm_config_store_dir="$SCRATCH/pnpm-store"
pnpm() { npx -y "$PNPM" "$@"; }
emit "# $PNPM, node $(node --version), $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m), macOS $(sw_vers -productVersion), sparse APFS image"

f0=$(free_kib); t0=$(now)
(cd "$REPO" && pnpm install --frozen-lockfile --ignore-scripts >/dev/null)
t1=$(now); f1=$(free_kib)
emit "warm	install_s=$(echo "$t1 - $t0" | bc)	df_kib=$((f0 - f1))	node_modules_files=$(find "$REPO/node_modules" "$REPO"/pkgs/*/node_modules -type f 2>/dev/null | wc -l | tr -d ' ')"

f0=$(free_kib); t0=$(now)
i=0
while [ "$i" -lt "$WORKTREES" ]; do
  i=$((i + 1))
  ts=$(now)
  git -C "$REPO" worktree add -q --detach "$SCRATCH/wt-$i" HEAD
  (cd "$SCRATCH/wt-$i" && pnpm install --frozen-lockfile --offline --ignore-scripts >/dev/null)
  te=$(now)
  emit "worktree	i=$i	s=$(echo "$te - $ts" | bc)"
done
t1=$(now); f1=$(free_kib)
emit "worktrees	n=$WORKTREES	total_s=$(echo "$t1 - $t0" | bc)	df_kib=$((f0 - f1))	per_worktree_kib=$(((f0 - f1) / WORKTREES))"
