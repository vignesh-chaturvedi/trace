import unittest
from dates import parse_date


class TestDates(unittest.TestCase):
    def test_us_format(self):
        self.assertEqual(parse_date("3/14/2026"), (2026, 3, 14))

    def test_iso_format(self):
        self.assertEqual(parse_date("2026-03-14"), (2026, 3, 14))

    def test_no_date(self):
        self.assertIsNone(parse_date("no date here"))


if __name__ == "__main__":
    unittest.main()
