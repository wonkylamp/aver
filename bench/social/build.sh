#!/bin/bash
# build.sh [clip…]: set up, record both panes with vhs, and compose each clip
# into assets/social/<clip>.gif and .mp4. Run from anywhere; no clip means all five.
set -euo pipefail
export PATH=/opt/homebrew/bin:$PATH
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/../.."
free_gb() { df -g / | awk 'NR==2 {print $4}'; }

caption() {
  case $1 in
    lie-detector)  echo "Your CI says green. Your agent cheated.|Standard CI|Aver" ;;
    quiet-rewrite) echo "Agents can't approve their own homework.|Standard CI|Aver" ;;
    empty-test)    echo "A test that can't fail proves nothing.|Standard CI|Aver" ;;
    merge-day)     echo "Three agents. One file. No conflicts.|git|Aver" ;;
    scale)         echo "20 agent workspaces before your coffee.|git|Aver" ;;
  esac
}

for clip in "${@:-lie-detector quiet-rewrite empty-test merge-day scale}"; do
  for c in $clip; do
    if [ "$(free_gb)" -lt 15 ]; then echo "under 15 GB free: stopping" >&2; exit 3; fi
    "$here/setup.sh" "$c"
    for side in left right; do
      tape=/tmp/aver-social/$c/$side.tape
      { echo "Output \"/tmp/aver-social/$c/$side.gif\""; cat "$here/tapes/_settings.tape" "$here/tapes/$c.$side.tape"; } >"$tape"
      vhs -q "$tape"
    done
    IFS='|' read -r cap l r <<<"$(caption "$c")"
    "$here/compose.sh" "$c" "$cap" "$l" "$r"
  done
done
