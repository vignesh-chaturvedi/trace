from defaults import DEFAULTS
from merge import deep_merge


def load_config(overrides=None):
    """Return DEFAULTS with `overrides` applied on top."""
    return deep_merge(DEFAULTS, overrides or {})
