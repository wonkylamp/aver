#!/bin/sh
# Installs the aver binary from a GitHub release of wonkylamp/aver.
#
#   curl -fsSL https://aver.build/install.sh | sh
#
# Environment:
#   AVER_VERSION           release tag to install, e.g. v0.3.0 (default: the latest release)
#   AVER_INSTALL_DIR       where to put the binary (default: ~/.local/bin)
#   AVER_RELEASE_BASE_URL  releases URL (default: https://github.com/wonkylamp/aver/releases);
#                          tests point it at a file:// directory with the same layout
#
# Supported platforms: Linux x86_64 and aarch64, macOS arm64 and x86_64.
# Windows works through WSL2: run this script in the WSL2 Linux shell
# (docs/WINDOWS.md). The download is checked against the release's SHA256SUMS
# before it is installed. Nothing runs as root. After installing, the script
# warns, and never fails, when git or a running Docker is missing.
set -eu

repo=wonkylamp/aver
base_url=${AVER_RELEASE_BASE_URL:-https://github.com/$repo/releases}
install_dir=${AVER_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}
version=${AVER_VERSION:-latest}

say() { printf '%s\n' "$*"; }
fail() {
  printf 'aver install: %s\n' "$*" >&2
  exit 1
}
warn() { printf 'aver install: warning: %s\n' "$*" >&2; }

detect_platform() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os" in
    Linux) os=linux ;;
    Darwin) os=darwin ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT) fail "native Windows ('$os') is not supported. Install WSL2 and run this script in its Linux shell: https://github.com/$repo/blob/main/docs/WINDOWS.md" ;;
    *) fail "unsupported operating system '$os'. Release binaries exist for Linux and macOS; on other systems build from source: cargo install --locked --git https://github.com/$repo aver" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "unsupported architecture '$arch' on $os. Release binaries exist for x86_64 and aarch64 (arm64); build from source instead: cargo install --locked --git https://github.com/$repo aver" ;;
  esac
  # A shell under Rosetta reports x86_64 on Apple silicon; install the native binary.
  if [ "$os" = darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
    arch=aarch64
  fi
}

download() { # <url> <file>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --proto '=https,file' --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    fail "neither curl nor wget is installed"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 -r "$1" | cut -d' ' -f1
  else
    fail "no SHA-256 tool found (sha256sum, shasum or openssl); refusing to install an unverified binary"
  fi
}

detect_platform
asset="aver-$os-$arch"
case "$version" in
  latest) release_url="$base_url/latest/download" ;;
  v*) release_url="$base_url/download/$version" ;;
  *) release_url="$base_url/download/v$version" ;;
esac

tmp=$(mktemp -d "${TMPDIR:-/tmp}/aver-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT TERM

say "Downloading $asset ($version) from $release_url"
download "$release_url/$asset" "$tmp/$asset" ||
  fail "could not download $release_url/$asset. Check that release $version exists and has a $asset asset (the repository must be public for anonymous downloads)."
download "$release_url/SHA256SUMS" "$tmp/SHA256SUMS" ||
  fail "could not download $release_url/SHA256SUMS; refusing to install an unverified binary"

# Lines are "<sha256>  <name>", or "<sha256> *<name>" from tools in binary mode.
expected=$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || fail "SHA256SUMS of release $version has no entry for $asset; refusing to install an unverified binary"
actual=$(sha256_of "$tmp/$asset")
[ "$expected" = "$actual" ] ||
  fail "checksum mismatch for $asset: SHA256SUMS lists $expected, the download is $actual. Nothing was installed."

mkdir -p "$install_dir" || fail "cannot create $install_dir; set AVER_INSTALL_DIR to a directory you can write"
chmod 0755 "$tmp/$asset"
# Copy next to the target first so the final rename is atomic and a running
# aver is never left half-written.
cp "$tmp/$asset" "$install_dir/.aver.$$" || fail "cannot write to $install_dir; set AVER_INSTALL_DIR to a directory you can write"
mv -f "$install_dir/.aver.$$" "$install_dir/aver"

say "Installed aver to $install_dir/aver (sha256 $actual)"
case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *)
    say ""
    say "$install_dir is not on your PATH. Add it, then open a new terminal:"
    # The file the user's login shell reads: zsh (macOS's default) and fish
    # never read ~/.profile, and bash skips it when ~/.bash_profile exists.
    case "${SHELL:-}" in
      */fish)
        say "  fish_add_path $install_dir"
        ;;
      *)
        case "${SHELL:-}" in
          */zsh) rc='~/.zshrc' ;;
          */bash)
            if [ -f "$HOME/.bash_profile" ]; then rc='~/.bash_profile'; else rc='~/.bashrc'; fi
            ;;
          *) rc='~/.profile' ;;
        esac
        say "  echo 'export PATH=\"$install_dir:\$PATH\"' >> $rc"
        ;;
    esac
    ;;
esac

# Missing prerequisites are warnings: the binary is installed either way, and
# `aver doctor` checks them in full.
if ! command -v git >/dev/null 2>&1; then
  warn "git is not installed. Aver reads your repositories with it: https://git-scm.com/downloads"
fi

say ""
say "Next steps:"
say "  aver setup     # in your repository: connects your agent tools"
say "  aver doctor    # checks everything Aver needs"
say "Docs: https://github.com/$repo#readme"
