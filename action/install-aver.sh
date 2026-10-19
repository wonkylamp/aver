#!/usr/bin/env bash
# Installs the aver binary from the action repository's GitHub release for
# AVER_VERSION, or for the action's own ref when that is empty. Outputs
# need-build=true when no release asset exists (a branch or SHA ref, a private
# action repository, a local action, or a non-Linux runner), so the action
# builds from source-dir: the repository the runner downloaded for the action.
set -euo pipefail

output=${GITHUB_OUTPUT:-/dev/null}
base_url=${AVER_RELEASE_BASE_URL:-https://github.com}
action_path=${GITHUB_ACTION_PATH:?GITHUB_ACTION_PATH is not set}
source_dir=$(cd "$action_path/.." && pwd)

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

repo=${ACTION_REPOSITORY:-}
ref=${ACTION_REF:-}
# github.action_repository and github.action_ref can be empty in composite
# steps; the runner checks the action out at _actions/<owner>/<repo>/<ref>.
case "$action_path" in
  */_actions/*/*/*/action)
    rest=${action_path#*/_actions/}
    rest=${rest%/action}
    owner=${rest%%/*}
    rest=${rest#*/}
    [ -n "$repo" ] || repo="$owner/${rest%%/*}"
    [ -n "$ref" ] || ref=${rest#*/}
    ;;
esac
tag=${AVER_VERSION:-$ref}
# A series ref such as v0.4 is the commit release.yml last moved it to, and
# that commit's Cargo.toml names its release.
if [ -z "${AVER_VERSION:-}" ] && [[ "$tag" =~ ^v[0-9]+\.[0-9]+$ ]] && [ -f "$source_dir/Cargo.toml" ]; then
  version=$(awk '
    /^\[workspace\.package\]/ { p = 1; next }
    /^\[/ { p = 0 }
    p && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
  ' "$source_dir/Cargo.toml")
  case "$version" in
    "${tag#v}".*) tag=v$version ;;
  esac
fi

lock_hash=""
if [ -f "$source_dir/Cargo.lock" ]; then
  lock_hash=$(sha256_of "$source_dir/Cargo.lock")
fi

build_from_source() {
  echo "::notice title=Building aver from source::$1"
  {
    echo "need-build=true"
    echo "source-dir=$source_dir"
    echo "cargo-lock-hash=$lock_hash"
    echo "ref=${tag:-local}"
  } >>"$output"
  exit 0
}

[ "${RUNNER_OS:-$(uname -s)}" = Linux ] || build_from_source "the action installs release binaries on Linux runners only"
[ -n "$repo" ] && [ -n "$tag" ] || build_from_source "the action repository or ref is unknown (a local action)"

case "${RUNNER_ARCH:-$(uname -m)}" in
  X64 | x86_64 | amd64) asset=aver-linux-x86_64 ;;
  ARM64 | aarch64 | arm64) asset=aver-linux-aarch64 ;;
  *) build_from_source "no release binary for architecture ${RUNNER_ARCH:-$(uname -m)}" ;;
esac

bin_dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/aver-bin.XXXXXX")
url="$base_url/$repo/releases/download/$tag/$asset"

fetch() {
  curl -sSL --retry 3 -o "$2" -w '%{http_code}' "$1" || true
}

status=$(fetch "$url" "$bin_dir/$asset")
case "$status" in
  200) ;;
  404) build_from_source "no release asset $asset for $repo@$tag" ;;
  *)
    echo "::error::downloading $url failed (HTTP ${status:-none})"
    exit 1
    ;;
esac

status=$(fetch "$url.sha256" "$bin_dir/$asset.sha256")
if [ "$status" != 200 ]; then
  echo "::error::downloading $url.sha256 failed (HTTP ${status:-none}); refusing to use an unverified binary"
  exit 1
fi

read -r expected name <"$bin_dir/$asset.sha256" || true
actual=$(sha256_of "$bin_dir/$asset")
if [ "$name" != "$asset" ] || [ "$expected" != "$actual" ]; then
  echo "::error::checksum mismatch for $asset from $repo@$tag: the release lists '${expected:-nothing}  ${name:-}', the download is $actual"
  exit 1
fi

mv "$bin_dir/$asset" "$bin_dir/aver"
rm -f "$bin_dir/$asset.sha256"
chmod 0755 "$bin_dir/aver"
echo "$bin_dir" >>"${GITHUB_PATH:-/dev/null}"
echo "Installed aver from $repo@$tag ($asset, sha256 $actual)"
{
  echo "need-build=false"
  echo "source-dir=$source_dir"
  echo "cargo-lock-hash=$lock_hash"
  echo "ref=$tag"
} >>"$output"
