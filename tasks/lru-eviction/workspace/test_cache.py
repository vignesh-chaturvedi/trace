import unittest
from cache import LRUCache


class TestLRU(unittest.TestCase):
    def test_basic_get_put(self):
        c = LRUCache(2)
        c.put("a", 1)
        c.put("b", 2)
        self.assertEqual(c.get("a"), 1)
        self.assertEqual(c.get("b"), 2)

    def test_evicts_when_over_capacity(self):
        c = LRUCache(2)
        c.put("a", 1)
        c.put("b", 2)
        c.put("c", 3)
        self.assertEqual(len(c), 2)
        self.assertIsNone(c.get("a"))


if __name__ == "__main__":
    unittest.main()
