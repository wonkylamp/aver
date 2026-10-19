#!/usr/bin/env bash
# Runs site/install.sh and packaging/homebrew/update-formula.sh against fake
# GitHub releases: file:// directories with the releases/ URL layout, passed
# through AVER_RELEASE_BASE_URL, and a stub `uname` (and `sysctl`) on PATH to
# play each platform, with git and docker stubs on PATH or left off. install.sh
# runs under every POSIX shell found (sh, dash). Prints one line per test;
# exits 1 if any fails.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
INSTALL="$ROOT/site/install.sh"
FORMULA="$ROOT/packaging/homebrew/update-formula.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/aver-install-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

die() {
  echo "assertion failed: $*" >&2
  exit 1
}
contains() { grep -qF -- "$2" "$1" || die "$1 does not contain '$2'"; }
lacks() { ! grep -qF -- "$2" "$1" || die "$1 contains '$2'"; }

# A fake release: one binary per platform (a shell script printing its name)
# and SHA256SUMS, at <base>/download/<tag>/ and <base>/latest/download/.
REL="$T/releases"
make_release() { # <dir>
  mkdir -p "$1"
  local target
  for target in linux-x86_64 linux-aarch64 darwin-x86_64 darwin-aarch64; do
    printf '#!/bin/sh\necho "aver 0.2.0 (%s)"\n' "$target" >"$1/aver-$target"
  done
  (cd "$1" && for f in aver-*; do printf '%s  %s\n' "$(sha256_of "$f")" "$f"; done >SHA256SUMS)
}
make_release "$REL/download/v0.2.0"
make_release "$REL/latest/download"

mkdir -p "$T/stubs"
cat >"$T/stubs/uname" <<'EOF'
#!/bin/sh
case "$1" in
  -s) echo "$FAKE_OS" ;;
  -m) echo "$FAKE_ARCH" ;;
  *) echo "$FAKE_OS" ;;
esac
EOF
cat >"$T/stubs/sysctl" <<'EOF'
#!/bin/sh
[ "$*" = "-n sysctl.proc_translated" ] && echo "${FAKE_TRANSLATED:-0}"
EOF
chmod +x "$T/stubs/uname" "$T/stubs/sysctl"

# git and docker stubs live in their own directories so a test can leave
# either off PATH. `docker info` exits with $FAKE_DOCKER_INFO.
mkdir -p "$T/have-git" "$T/have-docker"
printf '#!/bin/sh\necho "git version 2.50.0"\n' >"$T/have-git/git"
cat >"$T/have-docker/docker" <<'EOF'
#!/bin/sh
[ "$1" = info ] && exit "${FAKE_DOCKER_INFO:-0}"
exit 0
EOF
chmod +x "$T/have-git/git" "$T/have-docker/docker"

# install.sh runs with a PATH of only the stubs and links to the tools it
# needs, so the host's own git and docker never leak into a test.
mkdir -p "$T/sys"
for tool in sh dash curl wget sha256sum shasum openssl perl cut awk mktemp rm mkdir chmod cp mv cat; do
  found=$(command -v "$tool" 2>/dev/null) || continue
  ln -s "$found" "$T/sys/$tool"
done

# Gives the current test a fresh install dir and output files.
fresh() {
  W=$(mktemp -d "$T/w.XXXXXX")
  export AVER_INSTALL_DIR="$W/bin" AVER_RELEASE_BASE_URL="file://$REL" HOME="$W/home"
  export FAKE_OS=Linux FAKE_ARCH=x86_64 FAKE_TRANSLATED=0 FAKE_DOCKER_INFO=0
  HAVE_GIT=1 HAVE_DOCKER=1 PATH_PREFIX=
  unset AVER_VERSION
}

# Runs install.sh under $SHELL_UNDER_TEST with the stubs first on PATH;
# stdout and stderr go to $W/out, the exit code to $W/code.
run_install() {
  local path="$T/stubs:$PATH_PREFIX"
  [ "$HAVE_GIT" = 1 ] && path="$path$T/have-git:"
  [ "$HAVE_DOCKER" = 1 ] && path="$path$T/have-docker:"
  set +e
  PATH="$path$T/sys" "$T/sys/$SHELL_UNDER_TEST" "$INSTALL" >"$W/out" 2>&1
  echo $? >"$W/code"
  set -e
}
code() { cat "$W/code"; }
installed() { [ -x "$W/bin/aver" ] || die "no executable $W/bin/aver"; "$W/bin/aver"; }

test_latest_linux_x86_64() {
  fresh
  run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
  [ "$(installed)" = "aver 0.2.0 (linux-x86_64)" ] || die "wrong binary: $(installed)"
  contains "$W/out" "$REL/latest/download"
  contains "$W/out" "Installed aver to $W/bin/aver"
  contains "$W/out" "is not on your PATH"
  contains "$W/out" "aver setup     # in your repository: connects your agent tools"
  contains "$W/out" "aver doctor    # checks everything Aver needs"
  lacks "$W/out" "warning"
}

test_path_hint_names_the_login_shells_file() {
  fresh
  SHELL=/bin/zsh run_install
  contains "$W/out" ">> ~/.zshrc"
  lacks "$W/out" "~/.profile"
  mkdir -p "$HOME" && : >"$HOME/.bash_profile"
  SHELL=/bin/bash run_install
  contains "$W/out" ">> ~/.bash_profile"
  rm "$HOME/.bash_profile"
  SHELL=/usr/bin/bash run_install
  contains "$W/out" ">> ~/.bashrc"
  SHELL=/usr/local/bin/fish run_install
  contains "$W/out" "fish_add_path $W/bin"
  SHELL=/bin/ksh run_install
  contains "$W/out" ">> ~/.profile"
}

test_native_windows_refused_with_wsl_link() {
  local os
  for os in MINGW64_NT-10.0-22631 MSYS_NT-10.0-22631 CYGWIN_NT-10.0-22631 Windows_NT; do
    fresh
    export FAKE_OS=$os
    run_install
    [ "$(code)" = 1 ] || die "$os: expected exit 1, got $(code)"
    contains "$W/out" "native Windows ('$os') is not supported"
    contains "$W/out" "https://github.com/wonkylamp/aver/blob/main/docs/WINDOWS.md"
    [ ! -e "$W/bin/aver" ] || die "installed on native Windows ($os)"
  done
}

test_missing_git_warns_but_installs() {
  fresh
  HAVE_GIT=0 HAVE_DOCKER=0
  run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
  [ "$(installed)" = "aver 0.2.0 (linux-x86_64)" ] || die "wrong binary: $(installed)"
  contains "$W/out" "warning: git is not installed"
  contains "$W/out" "https://git-scm.com/downloads"
  lacks "$W/out" "Docker"
  contains "$W/out" "aver doctor"
}

test_pinned_version_without_v() {
  fresh
  export AVER_VERSION=0.2.0 FAKE_ARCH=aarch64
  run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
  [ "$(installed)" = "aver 0.2.0 (linux-aarch64)" ] || die "wrong binary: $(installed)"
  contains "$W/out" "$REL/download/v0.2.0"
}

test_macos_arm64() {
  fresh
  export AVER_VERSION=v0.2.0 FAKE_OS=Darwin FAKE_ARCH=arm64
  run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
  [ "$(installed)" = "aver 0.2.0 (darwin-aarch64)" ] || die "wrong binary: $(installed)"
}

test_macos_x86_64() {
  fresh
  export FAKE_OS=Darwin FAKE_ARCH=x86_64
  run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
  [ "$(installed)" = "aver 0.2.0 (darwin-x86_64)" ] || die "wrong binary: $(installed)"
}

test_rosetta_gets_native_binary() {
  fresh
  export FAKE_OS=Darwin FAKE_ARCH=x86_64 FAKE_TRANSLATED=1
  run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
  [ "$(installed)" = "aver 0.2.0 (darwin-aarch64)" ] || die "wrong binary: $(installed)"
}

test_dir_on_path_gets_no_hint() {
  fresh
  mkdir -p "$W/bin"
  PATH_PREFIX="$W/bin:" run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
  lacks "$W/out" "is not on your PATH"
}

test_upgrade_replaces_binary() {
  fresh
  mkdir -p "$W/bin"
  printf '#!/bin/sh\necho old\n' >"$W/bin/aver"
  chmod +x "$W/bin/aver"
  run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
  [ "$(installed)" = "aver 0.2.0 (linux-x86_64)" ] || die "not replaced: $(installed)"
  [ -z "$(find "$W/bin" -name '.aver.*')" ] || die "temporary file left in $W/bin"
}

test_checksum_mismatch_refuses() {
  fresh
  local rel="$T/bad-sum"
  make_release "$rel/latest/download"
  echo tampered >>"$rel/latest/download/aver-linux-x86_64"
  export AVER_RELEASE_BASE_URL="file://$rel"
  mkdir -p "$W/bin"
  printf '#!/bin/sh\necho old\n' >"$W/bin/aver"
  chmod +x "$W/bin/aver"
  run_install
  [ "$(code)" = 1 ] || die "expected exit 1, got $(code)"
  contains "$W/out" "checksum mismatch for aver-linux-x86_64"
  contains "$W/out" "Nothing was installed"
  [ "$(installed)" = old ] || die "the existing binary was replaced"
}

test_missing_sum_entry_refuses() {
  fresh
  local rel="$T/no-entry"
  make_release "$rel/latest/download"
  grep -v 'aver-linux-x86_64$' "$rel/latest/download/SHA256SUMS" >"$rel/sums" && mv "$rel/sums" "$rel/latest/download/SHA256SUMS"
  export AVER_RELEASE_BASE_URL="file://$rel"
  run_install
  [ "$(code)" = 1 ] || die "expected exit 1, got $(code)"
  contains "$W/out" "has no entry for aver-linux-x86_64"
  [ ! -e "$W/bin/aver" ] || die "installed without a checksum"
}

test_missing_sums_file_refuses() {
  fresh
  local rel="$T/no-sums"
  make_release "$rel/latest/download"
  rm "$rel/latest/download/SHA256SUMS"
  export AVER_RELEASE_BASE_URL="file://$rel"
  run_install
  [ "$(code)" = 1 ] || die "expected exit 1, got $(code)"
  contains "$W/out" "refusing to install an unverified binary"
  [ ! -e "$W/bin/aver" ] || die "installed without a checksum"
}

test_binary_mode_sums_line() {
  fresh
  local rel="$T/star"
  make_release "$rel/latest/download"
  sed 's/  aver-/ *aver-/' "$rel/latest/download/SHA256SUMS" >"$rel/sums" && mv "$rel/sums" "$rel/latest/download/SHA256SUMS"
  export AVER_RELEASE_BASE_URL="file://$rel"
  run_install
  [ "$(code)" = 0 ] || die "exit $(code): $(cat "$W/out")"
}

test_missing_release_errors() {
  fresh
  export AVER_VERSION=v9.9.9
  run_install
  [ "$(code)" = 1 ] || die "expected exit 1, got $(code)"
  contains "$W/out" "could not download file://$REL/download/v9.9.9/aver-linux-x86_64"
}

test_unsupported_os() {
  fresh
  export FAKE_OS=FreeBSD
  run_install
  [ "$(code)" = 1 ] || die "expected exit 1, got $(code)"
  contains "$W/out" "unsupported operating system 'FreeBSD'"
  contains "$W/out" "cargo install"
  [ ! -e "$W/bin/aver" ] || die "installed on an unsupported OS"
}

test_unsupported_arch() {
  fresh
  export FAKE_ARCH=riscv64
  run_install
  [ "$(code)" = 1 ] || die "expected exit 1, got $(code)"
  contains "$W/out" "unsupported architecture 'riscv64' on linux"
}

test_formula_fills_every_platform() {
  fresh
  local out="$W/aver.rb"
  "$FORMULA" v0.2.0 "$out" 2>"$W/err"
  contains "$out" 'version "0.2.0"'
  contains "$out" "Generated from packaging/homebrew/aver.rb"
  lacks "$out" "@"
  local target sha
  for target in linux-x86_64 linux-aarch64 darwin-x86_64 darwin-aarch64; do
    sha=$(sha256_of "$REL/download/v0.2.0/aver-$target")
    contains "$out" "sha256 \"$sha\""
  done
  if command -v ruby >/dev/null 2>&1; then
    ruby -c "$out" >/dev/null || die "the formula is not valid Ruby"
  fi
}

test_formula_refuses_missing_platform() {
  fresh
  local rel="$T/formula-missing"
  make_release "$rel/download/v0.3.0"
  grep -v 'darwin-x86_64$' "$rel/download/v0.3.0/SHA256SUMS" >"$rel/sums" && mv "$rel/sums" "$rel/download/v0.3.0/SHA256SUMS"
  set +e
  AVER_RELEASE_BASE_URL="file://$rel" "$FORMULA" 0.3.0 >"$W/out" 2>&1
  local status=$?
  set -e
  [ "$status" = 1 ] || die "expected exit 1, got $status"
  contains "$W/out" "no valid entry for aver-darwin-x86_64"
}

shells=(sh)
command -v dash >/dev/null 2>&1 && shells+=(dash)
failed=0
count=0
run() { # <test> [shell]
  count=$((count + 1))
  local label=$1${2:+ ($2)}
  if (SHELL_UNDER_TEST=${2:-sh} "$1") 2>"$T/err"; then
    echo "ok   $label"
  else
    echo "FAIL $label"
    sed 's/^/     /' "$T/err"
    failed=1
  fi
}
for t in $(declare -F | awk '{print $3}' | grep '^test_'); do
  case "$t" in
    test_formula_*) run "$t" ;;
    *) for s in "${shells[@]}"; do run "$t" "$s"; done ;;
  esac
done
[ "$failed" = 0 ] || exit 1
echo "all $count install tests passed"
