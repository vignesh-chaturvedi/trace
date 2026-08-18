import unittest
from highlight import highlight


class TestHighlight(unittest.TestCase):
    def test_ascii(self):
        self.assertEqual(highlight("hello world", "world"), "hello **world**")

    def test_absent(self):
        self.assertEqual(highlight("hello", "zzz"), "hello")


if __name__ == "__main__":
    unittest.main()
