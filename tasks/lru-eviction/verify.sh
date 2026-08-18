#!/usr/bin/env bash
# Hidden suite. The visible tests never exercise re-use, so a fix that only
# trims _order to capacity passes them and still evicts the wrong key.
set -u
cat > .hidden_test.py <<'PYEOF'
import unittest
from cache import LRUCache


class Hidden(unittest.TestCase):
    def test_get_counts_as_use(self):
        c = LRUCache(2)
        c.put("a", 1); c.put("b", 2)
        c.get("a")            # 'a' is now the most recent, 'b' the oldest
        c.put("c", 3)
        self.assertEqual(c.get("a"), 1, "a was used most recently, must survive")
        self.assertIsNone(c.get("b"), "b was least recently used, must be evicted")

    def test_reput_counts_as_use(self):
        c = LRUCache(2)
        c.put("a", 1); c.put("b", 2)
        c.put("a", 10)
        c.put("c", 3)
        self.assertEqual(c.get("a"), 10)
        self.assertIsNone(c.get("b"))

    def test_order_does_not_grow_without_bound(self):
        c = LRUCache(2)
        for i in range(500):
            c.put("k", i)
        self.assertEqual(len(c), 1)
        internal = getattr(c, "_order", [])
        self.assertLess(len(internal), 50, "bookkeeping grew with every use")

    def test_capacity_one(self):
        c = LRUCache(1)
        c.put("a", 1); c.put("b", 2)
        self.assertIsNone(c.get("a"))
        self.assertEqual(c.get("b"), 2)

    def test_visible_suite_still_passes(self):
        import subprocess, sys
        r = subprocess.run([sys.executable, "test_cache.py"], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr.decode())


if __name__ == "__main__":
    unittest.main(verbosity=2)
PYEOF
python3 .hidden_test.py 2>&1; rc=$?
rm -f .hidden_test.py
exit $rc
