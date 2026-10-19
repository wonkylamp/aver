#!/bin/sh
# Runs each "name :: command" line of test/*.tests with calc.sh loaded and
# writes a JUnit report to $1.
out=${1:-unit.xml}
cases=""
n=0
failed=0
for f in test/*.tests; do
  while IFS= read -r line; do
    case $line in '' | '#'*) continue ;; esac
    name=${line%% :: *}
    cmd=${line#* :: }
    n=$((n + 1))
    if (. ./calc.sh && eval "$cmd") >/dev/null 2>&1; then
      cases="$cases<testcase classname=\"$f\" name=\"$name\"/>"
      echo "ok $name"
    else
      failed=$((failed + 1))
      cases="$cases<testcase classname=\"$f\" name=\"$name\"><failure message=\"failed\"/></testcase>"
      echo "FAIL $name"
    fi
  done <"$f"
done
printf '<?xml version="1.0"?>\n<testsuites><testsuite name="sh-calc" tests="%s" failures="%s">%s</testsuite></testsuites>\n' "$n" "$failed" "$cases" >"$out"
[ "$failed" -eq 0 ]
