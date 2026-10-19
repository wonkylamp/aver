#!/bin/sh
# Prints the path of the installed aver for the Claude Code plugin's hooks,
# or exits 1 when there is none.
#
# Apps started from the Dock on macOS, the Claude desktop app among them,
# get launchd's PATH (/usr/bin:/bin:/usr/sbin:/sbin), which holds none of
# the places aver is installed. So this looks for aver as $AVER_BIN, on
# PATH, then in ~/.local/bin (install.sh), /opt/homebrew/bin and
# /usr/local/bin (Homebrew) and ~/.cargo/bin (cargo install).
for dir in "${HOME:-}/.local/bin" /opt/homebrew/bin /usr/local/bin "${HOME:-}/.cargo/bin"; do
  case ":$PATH:" in
    *":$dir:"*) ;;
    *) PATH="$PATH:$dir" ;;
  esac
done

aver=${AVER_BIN:-}
if [ -z "$aver" ]; then
  aver=$(command -v aver 2>/dev/null) || aver=
fi
if [ -z "$aver" ] || [ ! -x "$aver" ]; then
  exit 1
fi
printf '%s\n' "$aver"
