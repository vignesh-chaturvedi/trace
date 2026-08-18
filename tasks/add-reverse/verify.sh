#!/usr/bin/env bash
set -u
out="$(python3 test_words.py 2>&1)"
rc=$?
echo "$out"
[ $rc -eq 0 ] || exit 1
echo "$out" | grep -q "all tests passed" || exit 1
exit 0
