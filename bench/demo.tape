# Aver in about 12 seconds. Run from the repository root: vhs bench/demo.tape
# Uses the installed aver on PATH and its own AVER_HOME under /tmp; never ~/.aver.

Output assets/hero-comparison.gif

Set Shell bash
Set FontSize 17
Set Width 1300
Set Height 600
Set Padding 18
Set Theme "Builtin Dark"
Set TypingSpeed 10ms
Set Framerate 20
Set CursorBlink false

Hide
Type `export PATH=$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin HERO=$PWD/bench/hero AVER_HOME=/tmp/aver-demo/home; unset CLAUDECODE AVER_RUN AVER_WORKSPACE; set -o pipefail` Enter
Type `rm -rf /tmp/aver-demo && $HERO/make-repo.sh /tmp/aver-demo/repo && cd /tmp/aver-demo/repo && aver workspace new --label warmup >/dev/null 2>&1` Enter
Type `agent-skips-the-test-it-broke() { $HERO/sabotage.sh; git diff --stat | tail -1; }; agent-fixes-divide() { $HERO/fix.sh; git status --short; }` Enter
Type `PS1='$(e=$?; [ $e = 0 ] && printf "\[\e[32m\]ok" || printf "\[\e[31m\]exit $e"; printf "\[\e[0m\]") \[\e[36m\]\W\[\e[0m\] $ '; clear` Enter
Sleep 1.5s
Show

Type `cd "$(aver workspace new --count 3 -c 'div: divide returns the quotient and throws on zero' | sed -n 1p)"` Enter
Sleep 600ms
Type `aver verify 2>/dev/null | head -1` Enter
Sleep 600ms
Type `agent-skips-the-test-it-broke` Enter
Sleep 300ms
Type `aver verify 2>/dev/null | head -4` Enter
Sleep 2.8s
Type `aver restore --last-green` Enter
Sleep 500ms
Type `agent-fixes-divide && aver verify 2>/dev/null | head -2` Enter
Sleep 1.7s
Type `aver accept | head -1` Enter
Sleep 1.6s
