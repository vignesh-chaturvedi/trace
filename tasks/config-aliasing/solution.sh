#!/usr/bin/env bash
set -eu
cat > merge.py <<'PYEOF'
import copy


def deep_merge(base, override):
    """Recursively merge `override` into `base`, mutating neither."""
    result = copy.deepcopy(base)
    for key, value in override.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = deep_merge(result[key], value)
        else:
            result[key] = copy.deepcopy(value)
    return result
PYEOF
