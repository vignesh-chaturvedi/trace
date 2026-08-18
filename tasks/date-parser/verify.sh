#!/usr/bin/env bash
# The trap: the ISO pattern is unanchored, so "14-03-2026" is matched by the
# ISO rule against the wrong digits before the European rule is ever tried.
# Reordering alone does not fix it -- the patterns need anchoring too.
set -u
cat > .hidden_test.py <<'PYEOF'
import unittest
from dates import parse_date


class Hidden(unittest.TestCase):
    def test_european_format(self):
        self.assertEqual(parse_date("14-03-2026"), (2026, 3, 14))

    def test_european_single_digit_day(self):
        self.assertEqual(parse_date("4-03-2026"), (2026, 3, 4))

    def test_iso_not_confused_by_european(self):
        self.assertEqual(parse_date("2026-03-14"), (2026, 3, 14))

    def test_embedded_in_prose(self):
        self.assertEqual(parse_date("due on 2026-03-14, no later"), (2026, 3, 14))
        self.assertEqual(parse_date("due on 14-03-2026, no later"), (2026, 3, 14))

    def test_invalid_calendar_date_raises(self):
        with self.assertRaises(ValueError):
            parse_date("2026-02-30")
        with self.assertRaises(ValueError):
            parse_date("13/14/2026")

    def test_still_none_when_absent(self):
        self.assertIsNone(parse_date("no date here"))
        self.assertIsNone(parse_date("12345"))

    def test_visible_suite_still_passes(self):
        import subprocess, sys
        r = subprocess.run([sys.executable, "test_dates.py"], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr.decode())


if __name__ == "__main__":
    unittest.main(verbosity=2)
PYEOF
python3 .hidden_test.py 2>&1; rc=$?
rm -f .hidden_test.py
exit $rc
