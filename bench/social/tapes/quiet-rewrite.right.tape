Hide
Type `source bench/social/prompt.sh; export AVER_HOME=/tmp/aver-social/quiet-rewrite/home; cd "$(cat /tmp/aver-social/quiet-rewrite/ws)"; clear` Enter
Sleep 1s
Show
Sleep 400ms
Type `agent-breaks-sub-and-edits-its-assertion` Enter
Sleep 300ms
Type `git diff -U0 | grep '^[-+] '` Enter
Sleep 1.2s
Type `aver verify 2>/dev/null | sed -n 1p` Enter
Sleep 2.5s
Type `CLAUDECODE=1 aver accept --approve-review` Enter
Sleep 6s
