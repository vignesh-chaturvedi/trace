import functools

from errors import TransientError


def retry(attempts=3):
    """Retry a call on TransientError only.

    `attempts` is the total number of calls, not the number of retries: with
    attempts=3 the function is called at most 3 times. PermanentError
    propagates immediately. The last error is raised if every attempt fails.
    """

    def decorate(fn):
        @functools.wraps(fn)
        def wrapper(*args, **kwargs):
            last = None
            for _ in range(attempts + 1):
                try:
                    return fn(*args, **kwargs)
                except Exception as exc:
                    last = exc
            raise last

        return wrapper

    return decorate
