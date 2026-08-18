#!/usr/bin/env bash
set -eu
cat > parse.py <<'PYEOF'
total = 0
for line in open("data.csv"):
    parts = line.strip().split(",")
    if len(parts) != 2:
        continue
    try:
        total += int(parts[1])
    except ValueError:
        continue

print(total)
PYEOF
