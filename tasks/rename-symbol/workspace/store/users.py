_USERS = {1: "ada", 2: "grace"}

# Audit log messages are part of the public contract and must not change.
AUDIT_TEMPLATE = "fetch_user called with id={id}"


def fetch_user(user_id):
    """Return the username for `user_id`, or None."""
    return _USERS.get(user_id)


def fetch_user_count():
    """Total number of users. A different function; leave its name alone."""
    return len(_USERS)
