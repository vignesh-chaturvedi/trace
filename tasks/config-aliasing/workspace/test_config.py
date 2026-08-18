import unittest
from loader import load_config


class TestConfig(unittest.TestCase):
    def test_override_applies(self):
        cfg = load_config({"server": {"port": 9000}})
        self.assertEqual(cfg["server"]["port"], 9000)

    def test_second_load_is_not_contaminated(self):
        load_config({"server": {"port": 9000}})
        cfg = load_config()
        self.assertEqual(cfg["server"]["port"], 8080, "defaults were mutated")


if __name__ == "__main__":
    unittest.main()
