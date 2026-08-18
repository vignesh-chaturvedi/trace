import unittest
from deps import CycleError, build_order


class TestDeps(unittest.TestCase):
    def test_linear_chain(self):
        self.assertEqual(build_order({"c": ["b"], "b": ["a"], "a": []}), ["a", "b", "c"])

    def test_detects_a_cycle(self):
        with self.assertRaises(CycleError):
            build_order({"a": ["b"], "b": ["a"]})


if __name__ == "__main__":
    unittest.main()
