from store.audit import record
from store.users import fetch_user, fetch_user_count


def lookup(user_id):
    name = fetch_user(user_id)
    record(user_id)
    return name


def stats():
    return {"users": fetch_user_count()}
