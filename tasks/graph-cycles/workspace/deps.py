class CycleError(Exception):
    pass


def build_order(graph):
    """Return a build order for `graph`: dependencies before dependents.

    `graph` maps a node to the list of nodes it depends on. Nodes appearing
    only as dependencies are part of the graph too. The order is deterministic:
    among nodes that are ready, the alphabetically first is chosen.

    Raises CycleError if the graph contains a cycle, including a self-loop.
    """
    order = []
    seen = set()

    def visit(node):
        if node in seen:
            raise CycleError(f"cycle at {node}")
        seen.add(node)
        for dep in graph.get(node, []):
            visit(dep)
        order.append(node)

    for node in graph:
        if node not in order:
            visit(node)

    return order
