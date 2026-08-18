#!/usr/bin/env bash
# The bug is a units mismatch: find_span returns byte offsets, highlight
# slices by characters. On ASCII they coincide, which is why the visible
# tests pass. Fixing only highlight, or only tokenize, is enough -- but the
# two must agree.
set -u
cat > .hidden_test.py <<'PYEOF'
import unittest
from highlight import highlight


class Hidden(unittest.TestCase):
    def test_accented(self):
        self.assertEqual(highlight("café world", "world"), "café **world**")

    def test_cjk(self):
        self.assertEqual(highlight("日本語 world", "world"), "日本語 **world**")

    def test_emoji_before_match(self):
        self.assertEqual(highlight("🙂 ok done", "done"), "🙂 ok **done**")

    def test_match_is_itself_multibyte(self):
        self.assertEqual(highlight("say 日本語 now", "日本語"), "say **日本語** now")

    def test_match_at_start(self):
        self.assertEqual(highlight("日本語 tail", "日本語"), "**日本語** tail")

    def test_no_characters_lost(self):
        for text, needle in [
            ("café world", "world"),
            ("日本語 world", "world"),
            ("🙂🙂 x", "x"),
        ]:
            out = highlight(text, needle)
            self.assertEqual(out.replace("**", ""), text, f"text corrupted: {out!r}")

    def test_visible_suite_still_passes(self):
        import subprocess, sys
        r = subprocess.run([sys.executable, "test_highlight.py"], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr.decode())


if __name__ == "__main__":
    unittest.main(verbosity=2)
PYEOF
python3 .hidden_test.py 2>&1; rc=$?
rm -f .hidden_test.py
exit $rc
