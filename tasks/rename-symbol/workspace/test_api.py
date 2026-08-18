import unittest
from api import lookup, stats


class TestApi(unittest.TestCase):
    def test_lookup(self):
        self.assertEqual(lookup(1), "ada")

    def test_stats(self):
        self.assertEqual(stats(), {"users": 2})


if __name__ == "__main__":
    unittest.main()
