#!/usr/bin/env bash
set -eu
cat > dates.py <<'PYEOF'
import datetime
import re

PATTERNS = [
    (r"\b(\d{4})-(\d{1,2})-(\d{1,2})\b", ("year", "month", "day")),
    (r"\b(\d{1,2})/(\d{1,2})/(\d{4})\b", ("month", "day", "year")),
    (r"\b(\d{1,2})-(\d{1,2})-(\d{4})\b", ("day", "month", "year")),
]


def parse_date(text):
    """Parse a date into (year, month, day)."""
    for pattern, order in PATTERNS:
        m = re.search(pattern, text)
        if m:
            parts = dict(zip(order, (int(g) for g in m.groups())))
            y, mo, d = parts["year"], parts["month"], parts["day"]
            try:
                datetime.date(y, mo, d)
            except ValueError as exc:
                raise ValueError(f"not a real date: {text!r}") from exc
            return (y, mo, d)
    return None
PYEOF
