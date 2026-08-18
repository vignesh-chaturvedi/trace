#!/usr/bin/env bash
# Two bugs, and fixing only the obvious one (catching Exception instead of
# TransientError) still leaves the off-by-one in `attempts + 1`.
set -u
cat > .hidden_test.py <<'PYEOF'
import unittest
from errors import PermanentError, TransientError
from retry import retry


class Hidden(unittest.TestCase):
    def test_attempts_is_total_calls(self):
        calls = []

        @retry(attempts=3)
        def always():
            calls.append(1)
            raise TransientError("busy")

        with self.assertRaises(TransientError):
            always()
        self.assertEqual(len(calls), 3, "attempts is the total number of calls")

    def test_attempts_of_one_means_no_retry(self):
        calls = []

        @retry(attempts=1)
        def always():
            calls.append(1)
            raise TransientError("busy")

        with self.assertRaises(TransientError):
            always()
        self.assertEqual(len(calls), 1)

    def test_recovers_partway_through(self):
        calls = []

        @retry(attempts=5)
        def flaky():
            calls.append(1)
            if len(calls) < 3:
                raise TransientError("busy")
            return "recovered"

        self.assertEqual(flaky(), "recovered")
        self.assertEqual(len(calls), 3)

    def test_unrelated_exceptions_propagate_immediately(self):
        calls = []

        @retry(attempts=3)
        def bad():
            calls.append(1)
            raise ValueError("not transient")

        with self.assertRaises(ValueError):
            bad()
        self.assertEqual(len(calls), 1)

    def test_metadata_is_preserved(self):
        @retry(attempts=2)
        def documented():
            """A docstring."""

        self.assertEqual(documented.__name__, "documented")
        self.assertEqual(documented.__doc__, "A docstring.")

    def test_visible_suite_still_passes(self):
        import subprocess, sys
        r = subprocess.run([sys.executable, "test_retry.py"], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr.decode())


if __name__ == "__main__":
    unittest.main(verbosity=2)
PYEOF
python3 .hidden_test.py 2>&1; rc=$?
rm -f .hidden_test.py
exit $rc
