#!/usr/bin/env bash
# A blind sed for "fetch_user" also hits fetch_user_count and the audit
# string. Both are checked.
set -u
cat > .hidden_test.py <<'PYEOF'
import unittest


class Hidden(unittest.TestCase):
    def test_new_name_exists_and_works(self):
        from store.users import get_user
        self.assertEqual(get_user(1), "ada")
        self.assertIsNone(get_user(99))

    def test_old_name_is_gone(self):
        import store.users as users
        self.assertFalse(hasattr(users, "fetch_user"), "old name still exported")

    def test_unrelated_function_kept_its_name(self):
        from store.users import fetch_user_count
        self.assertEqual(fetch_user_count(), 2)

    def test_audit_string_was_not_renamed(self):
        from store.audit import record
        self.assertEqual(record(7), "fetch_user called with id=7")

    def test_call_sites_updated(self):
        from api import lookup, stats
        self.assertEqual(lookup(2), "grace")
        self.assertEqual(stats(), {"users": 2})

    def test_no_stale_references_in_source(self):
        import pathlib, re
        for path in pathlib.Path(".").rglob("*.py"):
            if path.name.startswith(("test_", ".")):
                continue
            src = path.read_text()
            for line in src.splitlines():
                if "AUDIT_TEMPLATE" in line or line.strip().startswith("#"):
                    continue
                if re.search(r"\bfetch_user\b", line):
                    self.fail(f"{path}: stale reference: {line.strip()}")

    def test_visible_suite_still_passes(self):
        import subprocess, sys
        r = subprocess.run([sys.executable, "test_api.py"], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr.decode())


if __name__ == "__main__":
    unittest.main(verbosity=2)
PYEOF
python3 .hidden_test.py 2>&1; rc=$?
rm -f .hidden_test.py
rm -rf __pycache__ store/__pycache__
exit $rc
