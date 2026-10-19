#!/usr/bin/env bash
# Builds a release's assets on this Mac, as .github/workflows/release.yml
# would upload them: aver-<os>-<arch> for macOS arm64 and x86_64 and static
# musl Linux aarch64 and x86_64, each with its .sha256, and SHA256SUMS over
# all four, in dist/. Builds from `git archive <commit>` (default HEAD), so
# uncommitted changes never reach a binary, with AVER_ACTION_SHA set to that
# commit as the workflow sets it to the tagged one.
#
#   packaging/release-local.sh [commit]
#
# Needs rustup targets x86_64-apple-darwin, aarch64-unknown-linux-musl and
# x86_64-unknown-linux-musl, zig and cargo-zigbuild. CARGO_TARGET_DIR
# (default ~/.cache/aver-release-target) and AVER_RELEASE_SRC (default a
# temp directory) say where it builds.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
commit=$(git -C "$ROOT" rev-parse --verify "${1:-HEAD}^{commit}")
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/.cache/aver-release-target}
export AVER_ACTION_SHA=$commit
src=${AVER_RELEASE_SRC:-$(mktemp -d "${TMPDIR:-/tmp}/aver-release-src.XXXXXX")}
dist=$ROOT/dist

[ "$(uname -s)" = Darwin ] || { echo "error: the macOS binaries need a Mac" >&2; exit 1; }
command -v cargo-zigbuild >/dev/null || { echo "error: cargo-zigbuild is not installed (cargo install --locked cargo-zigbuild; brew install zig)" >&2; exit 1; }

# git archive stamps every file with the commit time, so a rerun on the same
# commit and directory reuses the build.
rm -rf "$src"
mkdir -p "$src"
git -C "$ROOT" archive "$commit" | tar -x -C "$src"
version=$(cd "$src" && cargo metadata --format-version 1 --no-deps --locked |
  python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "aver"))')

rm -rf "$dist"
mkdir -p "$dist"
for pair in aarch64-apple-darwin:darwin-aarch64 x86_64-apple-darwin:darwin-x86_64 \
  aarch64-unknown-linux-musl:linux-aarch64 x86_64-unknown-linux-musl:linux-x86_64; do
  triple=${pair%%:*}
  asset=aver-${pair#*:}
  echo "== $asset ($triple)" >&2
  case "$triple" in
    *-linux-*) (cd "$src" && cargo zigbuild --release --locked -p aver --target "$triple") ;;
    *) (cd "$src" && cargo build --release --locked -p aver --target "$triple") ;;
  esac
  cp "$CARGO_TARGET_DIR/$triple/release/aver" "$dist/$asset"
  (cd "$dist" && shasum -a 256 "$asset" >"$asset.sha256")
done

cd "$dist"
cat ./*.sha256 | sort -k 2 >SHA256SUMS
shasum -a 256 -c SHA256SUMS
[ "$(wc -l <SHA256SUMS)" -eq 4 ] || { echo "error: expected 4 binaries:" >&2; cat SHA256SUMS >&2; exit 1; }
# The x86_64 binary runs only under Rosetta; Linux ones are checked elsewhere.
for asset in aver-darwin-aarch64 aver-darwin-x86_64; do
  if [ "$asset" = aver-darwin-x86_64 ] && ! arch -x86_64 /usr/bin/true 2>/dev/null; then
    echo "note: Rosetta is not installed; $asset was not run" >&2
    continue
  fi
  "$ROOT/packaging/release/smoke.sh" "./$asset" "$version"
done
echo "built aver $version from $commit into $dist" >&2
