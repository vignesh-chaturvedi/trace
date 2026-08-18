#!/usr/bin/env bash
set -eu
cat > report.py <<'PYEOF'
import csv


def load_rows(path):
    with open(path, newline="") as fh:
        return list(csv.DictReader(fh))


def total_amount(path="sales.csv"):
    """Sum the `amount` column."""
    return sum(int(row["amount"]) for row in load_rows(path))


def by_region(path="sales.csv"):
    """Return {region: total} for every region in the file."""
    totals = {}
    for row in load_rows(path):
        totals[row["region"]] = totals.get(row["region"], 0) + int(row["amount"])
    return totals


if __name__ == "__main__":
    print(total_amount())
PYEOF
