#!/usr/bin/env bash
# Frees about 20 GB on a hosted Ubuntu runner by deleting SDKs this
# repository never uses. `start` runs the deletion in the background and
# returns; `wait` blocks until it has finished, then prints the free space.
# The two calls may be in different steps: the runner keeps a step's
# background processes alive until the job ends.
set -euo pipefail

done_marker="${RUNNER_TEMP:?}/free-disk.done"

case "${1:-}" in
  start)
    rm -f "$done_marker"
    nohup bash -c 'sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /opt/hostedtoolcache/CodeQL; touch "$1"' \
      _ "$done_marker" >/dev/null 2>&1 &
    df -h /
    ;;
  wait)
    start=$SECONDS
    while [ ! -e "$done_marker" ]; do
      if [ $((SECONDS - start)) -ge 600 ]; then
        echo "::warning::freeing disk space took over 10 minutes; continuing without it"
        break
      fi
      sleep 2
    done
    echo "waited $((SECONDS - start))s for the deletion"
    df -h /
    ;;
  *)
    echo "usage: $0 start|wait" >&2
    exit 2
    ;;
esac
