class TransientError(Exception):
    """Worth retrying: a timeout, a busy resource, a 503."""


class PermanentError(Exception):
    """Not worth retrying: bad input, a 400, a missing file."""
