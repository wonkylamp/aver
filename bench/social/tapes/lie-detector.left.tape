Hide
Type `source bench/social/prompt.sh; cd /tmp/aver-social/lie-detector/left/calc; clear` Enter
Sleep 1s
Show
Sleep 400ms
Type `agent-breaks-sub-and-skips-its-test` Enter
Sleep 300ms
Type `git diff -U0 test/ | tail -2` Enter
Sleep 900ms
Type `npm test --silent 2>&1 | tail -8` Enter
Sleep 6s
