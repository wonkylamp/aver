Hide
Type `source bench/social/prompt.sh; export AVER_HOME=/tmp/aver-social/empty-test/home; cd /tmp/aver-social/empty-test/right/calc; clear` Enter
Sleep 1s
Show
Sleep 400ms
Type `cd "$(aver workspace new -c 'div: divide returns the quotient' 2>/dev/null | tail -1)"` Enter
Sleep 600ms
Type `agent-adds-a-test-that-cannot-fail` Enter
Sleep 300ms
Type `cat test/divide.test.js` Enter
Sleep 1.5s
Type `aver verify 2>/dev/null | sed -n '1p;/NOT PROVEN/p'` Enter
Sleep 6s
