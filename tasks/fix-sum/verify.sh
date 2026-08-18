#!/usr/bin/env bash
# Injected only after the agent stops. 108 = 4+8+15+16+23+42
set -u
got="$(bash sum.sh 2>&1)"
if [ "$got" = "108" ]; then
  echo "PASS: sum.sh printed $got"
  exit 0
fi
echo "FAIL: expected 108, got '$got'"
exit 1
