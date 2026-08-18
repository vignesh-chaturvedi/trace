#!/usr/bin/env bash
set -u
got="$(python3 parse.py 2>&1)"
rc=$?
if [ $rc -ne 0 ]; then
  echo "FAIL: parse.py exited $rc: $got"
  exit 1
fi
# 10 + 20 + 30 + 40 = 100
if [ "$(echo "$got" | tail -1)" = "100" ]; then
  echo "PASS: printed 100"
  exit 0
fi
echo "FAIL: expected 100, got '$got'"
exit 1
