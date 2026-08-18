#!/usr/bin/env bash
# The obvious fix -- copy DEFAULTS in load_config -- is shallow, so nested
# dicts and lists are still shared. These check the depth.
set -u
cat > .hidden_test.py <<'PYEOF'
import copy, unittest
import defaults
from loader import load_config
from merge import deep_merge


class Hidden(unittest.TestCase):
    def setUp(self):
        self.pristine = copy.deepcopy(defaults.DEFAULTS)

    def tearDown(self):
        self.assertEqual(defaults.DEFAULTS, self.pristine, "DEFAULTS was mutated")

    def test_nested_dict_is_not_shared(self):
        a = load_config()
        a["server"]["host"] = "changed"
        b = load_config()
        self.assertEqual(b["server"]["host"], "localhost")

    def test_nested_list_is_not_shared(self):
        a = load_config()
        a["server"]["tags"].append("mutated")
        b = load_config()
        self.assertEqual(b["server"]["tags"], ["default"])

    def test_deep_merge_does_not_mutate_either_argument(self):
        base = {"a": {"b": 1}, "list": [1, 2]}
        over = {"a": {"c": 2}}
        base_before = copy.deepcopy(base)
        over_before = copy.deepcopy(over)
        out = deep_merge(base, over)
        self.assertEqual(base, base_before, "deep_merge mutated its base argument")
        self.assertEqual(over, over_before, "deep_merge mutated its override argument")
        self.assertEqual(out, {"a": {"b": 1, "c": 2}, "list": [1, 2]})

    def test_override_values_are_not_aliased_into_the_result(self):
        over = {"server": {"tags": ["x"]}}
        cfg = load_config(over)
        cfg["server"]["tags"].append("y")
        self.assertEqual(over["server"]["tags"], ["x"], "caller's dict was aliased")

    def test_deep_override_still_works(self):
        cfg = load_config({"logging": {"level": "debug"}})
        self.assertEqual(cfg["logging"]["level"], "debug")
        self.assertEqual(cfg["logging"]["handlers"], ["stdout"])
        self.assertEqual(cfg["server"]["port"], 8080)

    def test_visible_suite_still_passes(self):
        import subprocess, sys
        r = subprocess.run([sys.executable, "test_config.py"], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr.decode())


if __name__ == "__main__":
    unittest.main(verbosity=2)
PYEOF
python3 .hidden_test.py 2>&1; rc=$?
rm -f .hidden_test.py
exit $rc
