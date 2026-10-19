# Sourced by every tape's hidden first line: a user's terminal (no agent
# markers), the installed aver (or the one in $AVER_BIN_DIR), a prompt that
# names the last exit code, and the scripted agent edits each clip runs on
# screen.
export PATH=${AVER_BIN_DIR:+$AVER_BIN_DIR:}$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin
unset CLAUDECODE CLAUDE_CODE_ENTRYPOINT CODEX_SANDBOX AVER_RUN AVER_WORKSPACE AVER_URL
set -o pipefail
SOCIAL=${SOCIAL:-$PWD/bench/social}
TIMEFORMAT=$'\e[1;33mtotal %Rs\e[0m'
PS1='$(e=$?; [ $e = 0 ] && printf "\[\e[1;97;42m\] ok " || printf "\[\e[1;97;41m\] exit $e "; printf "\[\e[0m\]") \[\e[36m\]\W\[\e[0m\] $ '

agent-breaks-sub-and-skips-its-test() { "$SOCIAL/../hero/sabotage.sh"; }
agent-breaks-sub-and-edits-its-assertion() { "$SOCIAL/agents/rewrite.sh"; }
agent-adds-a-test-that-cannot-fail() { "$SOCIAL/agents/empty-test.sh"; }
