#!/usr/bin/env bash
set -eu
cat > tokenize.py <<'PYEOF'
def find_span(text, needle):
    """Return (start, end) character offsets of `needle` in `text`, or None."""
    at = text.find(needle)
    if at < 0:
        return None
    return (at, at + len(needle))
PYEOF
