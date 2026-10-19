#!/usr/bin/env bash
# Build Aver and set up a home: the release binary, and the home's keys on
# first run. Nothing keeps running: every aver command works in process.
set -euo pipefail
cd "$(dirname "$0")/.."

export AVER_HOME="${AVER_HOME:-$HOME/.aver}"

echo "==> building aver (release)"
cargo build --release --quiet -p aver
AVER="$PWD/target/release/aver"

echo "==> keys in $AVER_HOME"
VERIFIER_KEY="$("$AVER" key)"

cat <<MSG

Aver is ready.
  binary        $AVER
  home          $AVER_HOME  (admin key: $AVER_HOME/keys/admin.json)
  verifier key  $VERIFIER_KEY

Try next, in a Git repository:
  $AVER doctor
  $AVER setup
  $AVER workspace new . -c "id: what done means"
MSG
