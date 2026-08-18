#!/usr/bin/env bash
set -eu
cat > deps.py <<'PYEOF'
class CycleError(Exception):
    pass


def build_order(graph):
    """Return a build order for `graph`: dependencies before dependents."""
    nodes = set(graph)
    for deps in graph.values():
        nodes.update(deps)

    remaining = {n: set(graph.get(n, [])) for n in nodes}
    order = []

    while remaining:
        ready = sorted(n for n, deps in remaining.items() if not deps)
        if not ready:
            raise CycleError(f"cycle among {sorted(remaining)}")
        for node in ready:
            order.append(node)
            del remaining[node]
        for deps in remaining.values():
            deps.difference_update(ready)

    return order
PYEOF
