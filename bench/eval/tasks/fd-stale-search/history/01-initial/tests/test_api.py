import unittest

from catalog.store import NotFound

from .samples import shop


class ApiTest(unittest.TestCase):
    def test_create_shows_in_search(self):
        _, _, api = shop()
        self.assertEqual(api.search("jug"), [])
        api.create_item("j1", "Milk Jug", 1100, ["kitchen"])
        self.assertEqual(api.search("jug"), ["Milk Jug"])

    def test_rename(self):
        _, _, api = shop()
        self.assertEqual(api.rename_item("t1", "Large Brown Betty Teapot")["name"], "Large Brown Betty Teapot")
        self.assertEqual(api.get_item("t1")["name"], "Large Brown Betty Teapot")

    def test_set_price(self):
        _, _, api = shop()
        self.assertEqual(api.set_price("m1", 850), {"id": "m1", "name": "Enamel Mug", "price_pence": 850, "tags": ["kitchen"]})

    def test_delete(self):
        _, _, api = shop()
        api.delete_item("m1")
        with self.assertRaises(NotFound):
            api.get_item("m1")

    def test_search_returns_names(self):
        _, _, api = shop()
        self.assertEqual(api.search("mug"), ["Enamel Mug", "Stoneware Mug"])


if __name__ == "__main__":
    unittest.main()
