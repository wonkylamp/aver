# `aver demo` as a GIF for the README. Run from the repository root: vhs bench/aver-demo.tape
# Uses the installed aver on PATH; the demo makes its own AVER_HOME under /tmp and removes it.

Output assets/demo.gif

Set Shell bash
Set FontSize 14
Set Width 1100
Set Height 520
Set Padding 18
Set Theme "Builtin Dark"
Set TypingSpeed 40ms
Set Framerate 15
Set CursorBlink false

Hide
Type `export PATH=$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin TMPDIR=/tmp; unset AVER_HOME CLAUDECODE AVER_RUN AVER_WORKSPACE; PS1='$ '; clear` Enter
Sleep 500ms
Show

Type "aver demo"
Sleep 400ms
Enter
Sleep 6s
