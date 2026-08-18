import unittest
from report import total_amount


class TestReport(unittest.TestCase):
    def test_total(self):
        self.assertEqual(total_amount(), 750)


if __name__ == "__main__":
    unittest.main()
