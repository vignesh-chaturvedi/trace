#!/usr/bin/env bash
set -eu
sed -i.bak 's/total - n/total + n/' sum.sh && rm -f sum.sh.bak
