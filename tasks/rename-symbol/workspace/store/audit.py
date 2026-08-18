from store.users import AUDIT_TEMPLATE


def record(user_id):
    """Emit the audit line for a lookup."""
    return AUDIT_TEMPLATE.format(id=user_id)
