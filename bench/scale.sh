#!/usr/bin/env bash
# Scale benchmark: what N Aver workspaces cost on a real Node repository with
# real dependencies, against `git worktree add` plus `npm ci`, and what
# verifying in a few of them adds. Writes raw numbers to $OUT (TSV lines on
# stdout too); docs/BENCHMARKS.md summarizes them.
#
#   bench/scale.sh                       # N = 20 50 100, worktrees 5, verify 3
#   NS="20 50" VERIFY=0 bench/scale.sh
#   AVER=/path/to/aver MIN_FREE_GIB=12 KEEP=1 bench/scale.sh
#
# date-fns at REV is a pnpm monorepo, and Aver's dependency seeding reads npm
# lockfiles, so the script adds one local commit on top of REV: a root
# package-lock.json for the root devDependencies plus @date-fns/dev linked
# with `file:`, a root vitest config that runs pkgs/core's tests with only
# those dependencies (tz and utc aliased to their sources), and an aver.toml
# with that one check. The lockfile is resolved when the script runs.
#
# Disk is measured two ways: the free-space delta of the benchmark's own
# sparse APFS image (df; includes file metadata and Aver's store) and the
# APFS private size of each directory (bench/private-bytes.py, the measure of
# disk::usage::real_bytes; blocks clones share count in neither, inode and
# directory metadata not at all). `du` is not used: it counts every clone in full.
# A watchdog stops the run and deletes what it made when the host disk falls
# below MIN_FREE_GIB.
set -euo pipefail
cd "$(dirname "$0")/.."
BENCH="$PWD/bench"

REV="717ce0a807ea4c6b540d015b5c408723175b2838"
AVER="${AVER:-$(command -v aver)}"
NS="${NS:-20 50 100}"
WORKTREES="${WORKTREES:-5}"
VERIFY="${VERIFY:-3}"
MIN_FREE_GIB="${MIN_FREE_GIB:-12}"
HOST="${HOST:-$(mktemp -d "${TMPDIR:-/tmp}/aver-scale.XXXXXX")}"
OUT="${OUT:-$HOST/scale.tsv}"

# Everything runs on a sparse APFS disk image of its own (ISOLATE=0: in
# $HOST), so the volume's free-space deltas count this benchmark's writes
# only, not other processes'. Clones stay clones: the repository, the home
# and the worktrees share the volume.
if [ "${ISOLATE:-1}" = 1 ]; then
  hdiutil create -quiet -size "${IMAGE_GIB:-6}g" -type SPARSE -fs APFS -volname aver-scale "$HOST/scale"
  mkdir -p "$HOST/mnt"
  hdiutil attach -quiet -nobrowse -mountpoint "$HOST/mnt" "$HOST/scale.sparseimage"
  SCRATCH="$HOST/mnt"
else
  SCRATCH="$HOST"
fi
REPO="$SCRATCH/date-fns"

cleanup() {
  [ -n "${watchdog:-}" ] && kill "$watchdog" 2>/dev/null || true
  [ "${ISOLATE:-1}" = 1 ] && hdiutil detach -quiet -force "$HOST/mnt" || true
  [ "${KEEP:-0}" = 1 ] || rm -rf "$HOST"
}
trap cleanup EXIT
trap 'exit 3' TERM INT

# Free space on the host can also fall from other processes' writes mid-step:
# below the floor, the run stops and the EXIT trap deletes what it made.
(
  while sleep 2; do
    free=$(df -k "$HOST" | awk 'NR==2 {print $4}')
    if [ "$free" -lt $((MIN_FREE_GIB * 1024 * 1024)) ]; then
      echo "# aborted $(date -u +%FT%TZ): host free $((free / 1024)) MiB below ${MIN_FREE_GIB} GiB" | tee -a "$OUT" >&2
      # The children first: bash runs the trap only once its foreground child ends.
      kill -TERM $$
      pkill -TERM -P $$ 2>/dev/null
      exit
    fi
  done
) &
watchdog=$!

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
# Free space on the benchmark's volume (the measure) and on the host disk
# under it (the MIN_FREE_GIB floor).
free_kib() { df -k "$SCRATCH" | awk 'NR==2 {print $4}'; }
host_free_kib() { df -k "$HOST" | awk 'NR==2 {print $4}'; }
priv() { python3 -I "$BENCH/private-bytes.py" "$@" | awk '{s+=$1} END {print s+0}'; }
emit() { printf '%s\n' "$*" | tee -a "$OUT"; }
git_bench() { git -C "$REPO" -c user.name=bench -c user.email=bench@example.invalid "$@"; }
# A warm-up `aver workspace new` started in the background holds its log
# open until it finishes; wait so its base-slot writes land in one step only.
wait_warm() {
  local logs="$1/logs"
  while [ -d "$logs" ] && lsof -t "$logs"/warm-*.log >/dev/null 2>&1; do sleep 2; done
}
enough_room() {
  local need_kib="$1" free
  free=$(host_free_kib)
  [ $((free - need_kib)) -ge $((MIN_FREE_GIB * 1024 * 1024)) ]
}

emit "# aver $("$AVER" --version) ($AVER), $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m), macOS $(sw_vers -productVersion 2>/dev/null || uname -r), node $(node --version), npm $(npm --version)"
emit "# started $(date -u +%FT%TZ), host free $(($(host_free_kib) / 1024)) MiB, volume $(df -k "$SCRATCH" | awk 'NR==2 {print $1}') ($( [ "${ISOLATE:-1}" = 1 ] && echo "sparse APFS image" || echo "host disk"))"

# --- the repository -----------------------------------------------------
git init -q "$REPO"
git -C "$REPO" fetch -q --depth 1 https://github.com/date-fns/date-fns "$REV"
git -C "$REPO" checkout -q FETCH_HEAD
(
  cd "$REPO"
  npm pkg set "devDependencies.@date-fns/dev=file:pkgs/dev"
  npm install --package-lock-only --ignore-scripts --no-audit --no-fund >/dev/null 2>&1
  cat >vitest.scale.config.ts <<'EOF'
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const at = (p: string) => fileURLToPath(new URL(p, import.meta.url));

export default defineConfig({
  resolve: {
    alias: {
      "@date-fns/tz": at("./pkgs/tz/src/index.ts"),
      "@date-fns/utc": at("./pkgs/utc/src/index.ts"),
    },
  },
  test: { dir: "pkgs/core/src", include: ["**/test.ts"], exclude: ["fp/_lib/convertToFP/**"], pool: "threads" },
});
EOF
  cat >aver.toml <<'EOF'
# Scale bench: pkgs/core tests through the root vitest config (root npm deps only).
[[check]]
id = "core"
run = ["npx", "vitest", "run", "--config", "vitest.scale.config.ts", "--reporter=default", "--reporter=junit", "--outputFile.junit=/aver/out/core.xml"]
report = { format = "junit", path = "/aver/out/core.xml" }
EOF
)
git_bench add -A
git_bench commit -qm "scale bench: npm lockfile, root vitest config, aver.toml"

f0=$(free_kib); t0=$(now)
(cd "$REPO" && npm ci --ignore-scripts --no-audit --no-fund >/dev/null 2>&1)
t1=$(now); f1=$(free_kib)
npm_kib=$((f0 - f1)); [ "$npm_kib" -ge 163840 ] || npm_kib=163840
emit "repo	files=$(git -C "$REPO" ls-files | wc -l | tr -d ' ')	node_modules_files=$(find "$REPO/node_modules" -type f | wc -l | tr -d ' ')	npm_ci_s=$(echo "$t1 - $t0" | bc)	npm_ci_df_kib=$((f0 - f1))	node_modules_private=$(priv "$REPO/node_modules")"

# --- git worktree add + npm ci ---------------------------------------
f0=$(free_kib); t0=$(now)
made=0
i=0
while [ "$i" -lt "$WORKTREES" ]; do
  i=$((i + 1))
  # A worktree's node_modules is a real copy: room for one more, with margin.
  if ! enough_room $((npm_kib * 2)); then
    emit "worktree	i=$i	skipped: host free $(($(host_free_kib) / 1024)) MiB would drop below ${MIN_FREE_GIB} GiB"
    break
  fi
  made=$i
  ts=$(now)
  git -C "$REPO" worktree add -q --detach "$SCRATCH/wt-$i" HEAD
  (cd "$SCRATCH/wt-$i" && npm ci --ignore-scripts --no-audit --no-fund >/dev/null 2>&1)
  te=$(now)
  emit "worktree	i=$i	s=$(echo "$te - $ts" | bc)"
done
t1=$(now); f1=$(free_kib)
wts=$(i=1; while [ "$i" -le "$made" ]; do echo "$SCRATCH/wt-$i"; i=$((i + 1)); done)
# shellcheck disable=SC2086
emit "worktrees	n=$made	total_s=$(echo "$t1 - $t0" | bc)	df_kib=$((f0 - f1))	private=$(priv $wts)"
i=1; while [ "$i" -le "$made" ]; do git -C "$REPO" worktree remove --force "$SCRATCH/wt-$i"; i=$((i + 1)); done

# --- Aver workspaces at N ---------------------------------------------
# Per N, a fresh home: one first directory (import, checkout, dependency
# store), its background base warm-up waited out, then N more timed warm.
for n in $NS; do
  # About 6 MB per directory measured at N=20, with a 4x margin.
  if ! enough_room $((n * 24 * 1024)); then
    emit "aver	n=$n	skipped: host free $(($(host_free_kib) / 1024)) MiB would drop below ${MIN_FREE_GIB} GiB"
    continue
  fi
  home="$SCRATCH/home-$n"
  mkdir -p "$home"
  (cd "$SCRATCH" && AVER_HOME="$home" "$AVER" init --yes >/dev/null 2>&1)
  f0=$(free_kib); t0=$(now)
  AVER_HOME="$home" "$AVER" workspace new "$REPO" --json >"$SCRATCH/first-$n.json"
  t1=$(now)
  wait_warm "$home"
  tw=$(now)
  f1=$(free_kib)
  sides1=$(priv "$home/sides")
  AVER_HOME="$home" "$AVER" workspace new "$REPO" --count "$n" --json >"$SCRATCH/warm-$n.json"
  t2=$(now)
  wait_warm "$home"
  f2=$(free_kib)
  per=$(python3 -I -c '
import json, sys
ms = sorted(w["ms"] for w in json.load(open(sys.argv[1]))["workspaces"])
p = lambda q: ms[min(len(ms) - 1, int(q * len(ms)))]
print(f"ws_ms_p50={p(0.5):.0f}\tws_ms_p95={p(0.95):.0f}\tws_ms_max={ms[-1]:.0f}")
' "$SCRATCH/warm-$n.json")
  work=$(ls -d "$home"/work/*/ | head -1)
  emit "aver	n=$n	first_s=$(echo "$t1 - $t0" | bc)	warmup_s=$(echo "$tw - $t1" | bc)	first_plus_warmup_df_kib=$((f0 - f1))	base_slot_private=$sides1	warm_s=$(echo "$t2 - $tw" | bc)	$per	warm_df_kib=$((f1 - f2))	work_private=$(priv "$work")	home_private=$(priv "$home")	sides_private=$(priv "$home/sides")"
  if [ "$n" != "${NS%% *}" ] || [ "$VERIFY" = 0 ]; then
    rm -rf "$home"
  fi
done

# --- verify in VERIFY workspaces of the first N's home ----------------
if [ "$VERIFY" != 0 ]; then
  n="${NS%% *}"
  home="$SCRATCH/home-$n"
  work=$(ls -d "$home"/work/*/ | head -1)
  i=0
  for ws in $(ls -d "$work"ws-* | head -"$VERIFY"); do
    i=$((i + 1))
    # One line, behaviour unchanged, and a different line per workspace: the
    # same tree twice would reuse the first verify's runs.
    case $i in
      1) new='amount + _date.getDate()' ;;
      2) new='_date.getDate() - -amount' ;;
      *) new="_date.getDate() + amount * 1 + $((i - 3))" ;;
    esac
    sed -i '' "s/_date.setDate(_date.getDate() + amount);/_date.setDate($new);/" \
      "$ws/pkgs/core/src/addDays/index.ts"
    f0=$(free_kib); t0=$(now)
    set +e
    verdict=$(cd "$ws" && AVER_HOME="$home" "$AVER" verify 2>"$OUT.verify-$i.err" | tee "$OUT.verify-$i.out" | tail -1)
    set -e
    t1=$(now); f1=$(free_kib)
    slots=$(for s in "$home"/sides/*/*/; do printf '%s=%s,' "$(basename "$s")" "$(priv "$s")"; done)
    emit "verify	i=$i	s=$(echo "$t1 - $t0" | bc)	df_kib=$((f0 - f1))	sides_private=$(priv "$home/sides")	slots=$slots	verdict=$verdict"
  done
  rm -rf "$home"
fi
emit "# finished $(date -u +%FT%TZ), host free $(($(host_free_kib) / 1024)) MiB"
