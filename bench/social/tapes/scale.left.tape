Set TypingSpeed 5ms
Hide
Type `source bench/social/prompt.sh; cd /tmp/aver-social/scale/left/calc; clear` Enter
Sleep 1s
Show
Sleep 400ms
Type `time for i in $(seq 20); do git worktree add ../wt$i -b agent$i && (cd ../wt$i && npm ci --offline --no-audit --no-fund); done` Enter
Sleep 14s
