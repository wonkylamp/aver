#!/bin/sh
# Runs one of Aver's Claude Code hooks (`aver hook <name>`) for the plugin.
# Without an installed aver (found as scripts/aver.sh finds it) a starting
# session is told how to get one and every other hook does nothing. It
# always exits 0: a Stop block is the JSON aver prints, never an exit code.
if aver=$(sh "$(dirname "$0")/../scripts/aver.sh"); then
  "$aver" hook "$1" --plugin
elif [ "$1" = session-start ]; then
  printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Aver plugin is installed, but the `aver` CLI is missing. Run `/aver` to install the binary and initialize this repository."}}'
fi
exit 0
