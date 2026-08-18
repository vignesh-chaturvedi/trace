import unittest
from errors import PermanentError, TransientError
from retry import retry


class TestRetry(unittest.TestCase):
    def test_succeeds_first_time(self):
        calls = []

        @retry(attempts=3)
        def ok():
            calls.append(1)
            return "fine"

        self.assertEqual(ok(), "fine")
        self.assertEqual(len(calls), 1)

    def test_permanent_error_is_not_retried(self):
        calls = []

        @retry(attempts=3)
        def bad():
            calls.append(1)
            raise PermanentError("nope")

        with self.assertRaises(PermanentError):
            bad()
        self.assertEqual(len(calls), 1, "permanent errors must not be retried")


if __name__ == "__main__":
    unittest.main()
