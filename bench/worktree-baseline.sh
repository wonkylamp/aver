#!/usr/bin/env bash
# The pnpm and bun baselines for bench/scale.sh: `git worktree add` plus a
# frozen-lockfile install on date-fns at the same REV, from a store or cache
# warmed by one install first (not counted). date-fns is a pnpm monorepo, so
# this installs every workspace package, where scale.sh's npm rows install
# only the root devDependencies.
#
# PM=pnpm (default): `pnpm install --frozen-lockfile --offline`, with the
# pnpm version package.json pins (pnpm 9 when none).
# PM=bun: the warm-up `bun install` resolves afresh (bun 1.3 hangs migrating
# date-fns's pnpm-lock.yaml), and its bun.lock is committed so each worktree
# runs `bun install --frozen-lockfile`.
#
#   bench/worktree-baseline.sh               # pnpm, 5 worktrees
#   PM=bun WORKTREES=3 KEEP=1 bench/worktree-baseline.sh
#
# Like scale.sh it runs on a sparse APFS image of its own and measures disk
# as the image's free-space delta, which counts what pnpm cloned or
# hard-linked from the store (on the same volume) once.
set -euo pipefail
cd "$(dirname "$0")/.."

REV="717ce0a807ea4c6b540d015b5c408723175b2838"
PM="${PM:-pnpm}"
WORKTREES="${WORKTREES:-5}"
MIN_FREE_GIB="${MIN_FREE_GIB:-8}"
HOST="${HOST:-$(mktemp -d "${TMPDIR:-/tmp}/aver-$PM.XXXXXX")}"
OUT="${OUT:-$HOST/$PM.tsv}"

hdiutil create -quiet -size "${IMAGE_GIB:-6}g" -type SPARSE -fs APFS -volname "aver-$PM" "$HOST/image"
mkdir -p "$HOST/mnt"
hdiutil attach -quiet -nobrowse -mountpoint "$HOST/mnt" "$HOST/image.sparseimage"
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
case "$PM" in
  pnpm)
    pin=$(node -p 'require(process.argv[1]).packageManager || ""' "$REPO/package.json")
    case "$pin" in pnpm@*) TOOL="${pin%%+*}" ;; *) TOOL="pnpm@9" ;; esac # date-fns at REV pins none
    export npm_config_store_dir="$SCRATCH/pnpm-store"
    warm() { npx -y "$TOOL" install --frozen-lockfile --ignore-scripts >/dev/null; }
    install() { npx -y "$TOOL" install --frozen-lockfile --offline --ignore-scripts >/dev/null; }
    ;;
  bun)
    TOOL="bun@$(bun --version)"
    export BUN_INSTALL_CACHE_DIR="$SCRATCH/bun-cache"
    # bun 1.3 reads date-fns's `jsr:` specifier as a git URL, so the docs
    # package takes @deno/doc through JSR's npm registry instead.
    printf '[install.scopes]\n"@jsr" = "https://npm.jsr.io/"\n' > "$REPO/bunfig.toml"
    sed -i '' 's|"@deno/doc": "jsr:|"@deno/doc": "npm:@jsr/deno__doc@|' "$REPO/pkgs/docs/package.json"
    git -C "$REPO" rm -q pnpm-lock.yaml
    node -e 'const f=process.argv[1],p=require(f);p.workspaces=["pkgs/*","pkgs/tz/test/engines"];require("fs").writeFileSync(f,JSON.stringify(p,null,2)+"\n")' "$REPO/package.json" # pnpm-workspace.yaml's packages
    warm() { bun install --ignore-scripts >/dev/null; }
    install() { bun install --frozen-lockfile --ignore-scripts >/dev/null; }
    ;;
  *) echo "PM is pnpm or bun" >&2; exit 2 ;;
esac
emit "# $TOOL, node $(node --version), $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m), macOS $(sw_vers -productVersion), sparse APFS image"

f0=$(free_kib); t0=$(now)
(cd "$REPO" && warm)
t1=$(now); f1=$(free_kib)
if [ "$PM" = bun ]; then
  git -C "$REPO" add bun.lock bunfig.toml package.json pkgs/docs/package.json
  git -C "$REPO" -c user.name=bench -c user.email=bench@example.invalid commit -qm "bun.lock"
fi
emit "warm	install_s=$(echo "$t1 - $t0" | bc)	df_kib=$((f0 - f1))	node_modules_files=$(find "$REPO/node_modules" "$REPO"/pkgs/*/node_modules -type f 2>/dev/null | wc -l | tr -d ' ')"

f0=$(free_kib); t0=$(now)
i=0
while [ "$i" -lt "$WORKTREES" ]; do
  i=$((i + 1))
  ts=$(now)
  git -C "$REPO" worktree add -q --detach "$SCRATCH/wt-$i" HEAD
  (cd "$SCRATCH/wt-$i" && install)
  te=$(now)
  emit "worktree	i=$i	s=$(echo "$te - $ts" | bc)"
done
t1=$(now); f1=$(free_kib)
emit "worktrees	n=$WORKTREES	total_s=$(echo "$t1 - $t0" | bc)	df_kib=$((f0 - f1))	per_worktree_kib=$(((f0 - f1) / WORKTREES))"
