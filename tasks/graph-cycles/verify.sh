#!/usr/bin/env bash
# `seen` conflates "currently on the stack" with "already finished", so a
# diamond reports a false cycle. Splitting the two is the fix; self-loops and
# implicit nodes are the parts that get forgotten.
set -u
cat > .hidden_test.py <<'PYEOF'
import unittest
from deps import CycleError, build_order


def valid(order, graph):
    pos = {n: i for i, n in enumerate(order)}
    for node, deps in graph.items():
        for d in deps:
            if pos[d] > pos[node]:
                return False
    return True


class Hidden(unittest.TestCase):
    def test_diamond_is_not_a_cycle(self):
        g = {"d": ["b", "c"], "b": ["a"], "c": ["a"], "a": []}
        order = build_order(g)
        self.assertTrue(valid(order, g), order)
        self.assertEqual(len(order), 4)

    def test_shared_dependency_appears_once(self):
        g = {"x": ["lib"], "y": ["lib"], "lib": []}
        order = build_order(g)
        self.assertEqual(sorted(order), ["lib", "x", "y"])
        self.assertEqual(len(order), len(set(order)), "a node was emitted twice")

    def test_self_loop_is_a_cycle(self):
        with self.assertRaises(CycleError):
            build_order({"a": ["a"]})

    def test_nodes_only_named_as_dependencies_are_included(self):
        order = build_order({"app": ["runtime"]})
        self.assertEqual(order, ["runtime", "app"])

    def test_disconnected_components(self):
        g = {"a": [], "b": [], "c": ["a"]}
        order = build_order(g)
        self.assertEqual(sorted(order), ["a", "b", "c"])
        self.assertTrue(valid(order, g))

    def test_deterministic_tie_break(self):
        g = {"z": [], "a": [], "m": []}
        self.assertEqual(build_order(g), build_order(g))
        self.assertEqual(build_order(g), ["a", "m", "z"])

    def test_deep_chain_does_not_blow_the_stack(self):
        n = 2000
        g = {f"n{i}": ([f"n{i-1}"] if i else []) for i in range(n)}
        order = build_order(g)
        self.assertEqual(len(order), n)

    def test_visible_suite_still_passes(self):
        import subprocess, sys
        r = subprocess.run([sys.executable, "test_deps.py"], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr.decode())


if __name__ == "__main__":
    unittest.main(verbosity=2)
PYEOF
python3 .hidden_test.py 2>&1; rc=$?
rm -f .hidden_test.py
exit $rc
