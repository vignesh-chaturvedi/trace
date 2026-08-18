def deep_merge(base, override):
    """Recursively merge `override` into `base` and return the result."""
    result = base
    for key, value in override.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = deep_merge(result[key], value)
        else:
            result[key] = value
    return result
