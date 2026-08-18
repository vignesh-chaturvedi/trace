#!/usr/bin/env bash
set -eu
cat > store/users.py <<'PYEOF'
_USERS = {1: "ada", 2: "grace"}

# Audit log messages are part of the public contract and must not change.
AUDIT_TEMPLATE = "fetch_user called with id={id}"


def get_user(user_id):
    """Return the username for `user_id`, or None."""
    return _USERS.get(user_id)


def fetch_user_count():
    """Total number of users. A different function; leave its name alone."""
    return len(_USERS)
PYEOF
cat > api.py <<'PYEOF'
from store.audit import record
from store.users import fetch_user_count, get_user


def lookup(user_id):
    name = get_user(user_id)
    record(user_id)
    return name


def stats():
    return {"users": fetch_user_count()}
PYEOF
