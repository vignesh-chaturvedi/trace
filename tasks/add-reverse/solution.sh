#!/usr/bin/env bash
set -eu
cat > words.py <<'PYEOF'
def reverse_words(s):
    """Return s with the order of its whitespace-separated words reversed."""
    return " ".join(reversed(s.split()))
PYEOF
