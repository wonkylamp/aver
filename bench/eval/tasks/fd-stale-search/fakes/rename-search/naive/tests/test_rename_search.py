import unittest

from .samples import shop


class RenameSearchTest(unittest.TestCase):
    def test_a_renamed_item_is_found_by_its_new_name(self):
        _, _, api = shop()
        self.assertEqual(api.search("betty"), ["Brown Betty Teapot"])
        api.rename_item("t1", "Large Teapot")
        self.assertEqual(api.search("betty"), [])
        self.assertEqual(api.search("large"), ["Large Teapot"])


if __name__ == "__main__":
    unittest.main()
