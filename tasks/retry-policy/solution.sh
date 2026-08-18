#!/usr/bin/env bash
set -eu
cat > retry.py <<'PYEOF'
import functools

from errors import TransientError


def retry(attempts=3):
    """Retry a call on TransientError only."""

    def decorate(fn):
        @functools.wraps(fn)
        def wrapper(*args, **kwargs):
            last = None
            for _ in range(attempts):
                try:
                    return fn(*args, **kwargs)
                except TransientError as exc:
                    last = exc
            raise last

        return wrapper

    return decorate
PYEOF
