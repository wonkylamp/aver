#!/usr/bin/env bash
# Checks the four binaries in <dir> against their .sha256 files and writes
# <dir>/SHA256SUMS over them, sorted by name: what site/install.sh, the
# Homebrew formula and the provenance attestation read.
#
#   packaging/release/sums.sh <dir>
set -euo pipefail

[ $# -eq 1 ] || { echo "usage: $0 <dir>" >&2; exit 2; }
cd "$1"
sha256_check() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum -c "$@"; else shasum -a 256 -c "$@"; fi
}
ls -l
sha256_check ./*.sha256
cat ./*.sha256 | sort -k 2 >SHA256SUMS
sha256_check SHA256SUMS
[ "$(wc -l <SHA256SUMS | tr -d ' ')" -eq 4 ] || { echo "::error::expected 4 binaries:"; cat SHA256SUMS; exit 1; }
for target in darwin-aarch64 darwin-x86_64 linux-aarch64 linux-x86_64; do
  grep -qE "  aver-$target\$" SHA256SUMS || { echo "::error::SHA256SUMS has no aver-$target"; exit 1; }
done
cat SHA256SUMS
