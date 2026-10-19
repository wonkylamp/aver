#!/usr/bin/env bash
# Builds one release binary as release.yml uploads it: dist/<asset> and
# dist/<asset>.sha256. Linux targets are static musl, as
# packaging/release-local.sh builds them; they need musl-gcc (musl-tools).
#
#   packaging/release/build.sh <rust target triple> <asset>
#
# AVER_ACTION_SHA, when set, is the commit aver init --ci pins the action to.
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <triple> <asset>" >&2; exit 2; }
triple=$1
asset=$2
root=$(cd "$(dirname "$0")/../.." && pwd)
target_dir=${CARGO_TARGET_DIR:-$root/target}

rustup target add "$triple"
case "$triple" in
  *-linux-musl)
    # The cc crate looks for <arch>-linux-musl-gcc; Ubuntu's musl-tools has musl-gcc.
    cc_var=CC_$(tr - _ <<<"$triple")
    export "$cc_var=${!cc_var:-musl-gcc}"
    ;;
esac
(cd "$root" && cargo build --release --locked -p aver --target "$triple")

mkdir -p "$root/dist"
cp "$target_dir/$triple/release/aver" "$root/dist/$asset"
cd "$root/dist"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$asset" >"$asset.sha256"
else
  shasum -a 256 "$asset" >"$asset.sha256"
fi
cat "$asset.sha256"
