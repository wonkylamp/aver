import unittest

from .samples import shop


class RenameSearchTest(unittest.TestCase):
    def test_a_renamed_item_is_found_by_its_new_name(self):
        _, _, api = shop()
        self.assertEqual(api.search("betty"), ["Brown Betty Teapot"])
        api.rename_item("t1", "Large Teapot")
        self.assertEqual(api.search("betty"), [])
        self.assertEqual(api.search("large"), ["Large Teapot"])

    def test_any_change_to_the_store_shows_in_search(self):
        store, index, _ = shop()
        self.assertEqual(len(index.search("mug")), 2)
        store.delete("m1")
        self.assertEqual([i.id for i in index.search("mug")], ["m2"])


if __name__ == "__main__":
    unittest.main()
