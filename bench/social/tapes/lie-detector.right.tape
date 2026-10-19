Hide
Type `source bench/social/prompt.sh; export AVER_HOME=/tmp/aver-social/lie-detector/home; cd "$(cat /tmp/aver-social/lie-detector/ws)"; clear` Enter
Sleep 1s
Show
Sleep 400ms
Type `agent-breaks-sub-and-skips-its-test` Enter
Sleep 300ms
Type `git diff -U0 test/ | tail -2` Enter
Sleep 900ms
Type `aver verify 2>/dev/null | sed -n 1,3p` Enter
Sleep 6s
