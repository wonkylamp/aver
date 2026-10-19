"""Search is always current, whichever way the store changes (README)."""

import unittest

from catalog.api import Api
from catalog.importer import apply_changes, parse_rows
from catalog.models import Item
from catalog.search import SearchIndex
from catalog.store import Store


def shop():
    store = Store()
    for item in [
        Item("t1", "Brown Betty Teapot", 2400, ("tea", "kitchen")),
        Item("m1", "Enamel Mug", 900, ("kitchen",)),
        Item("m2", "Stoneware Mug", 1200, ("kitchen", "handmade")),
    ]:
        store.add(item)
    index = SearchIndex(store)
    return store, index, Api(store, index)


class FreshSearchTest(unittest.TestCase):
    def test_rename_through_the_api(self):
        _, _, api = shop()
        self.assertEqual(api.search("betty"), ["Brown Betty Teapot"])
        api.rename_item("t1", "Large Teapot")
        self.assertEqual(api.search("betty"), [])
        self.assertEqual(api.search("large"), ["Large Teapot"])

    def test_price_change_through_the_api(self):
        _, index, api = shop()
        self.assertEqual(index.search("enamel")[0].price_pence, 900)
        api.set_price("m1", 850)
        self.assertEqual(index.search("enamel")[0].price_pence, 850)

    def test_delete_through_the_api(self):
        _, _, api = shop()
        self.assertEqual(api.search("mug"), ["Enamel Mug", "Stoneware Mug"])
        api.delete_item("m1")
        self.assertEqual(api.search("mug"), ["Stoneware Mug"])

    def test_the_nightly_import(self):
        store, _, api = shop()
        self.assertEqual(api.search("mug"), ["Enamel Mug", "Stoneware Mug"])
        rows = parse_rows("id,name,price_pence,tags\nm1,Enamel Camping Mug,950,kitchen\nm3,Glass Mug,700,kitchen\n")
        apply_changes(store, rows)
        self.assertEqual(api.search("mug"), ["Enamel Camping Mug", "Glass Mug", "Stoneware Mug"])
        self.assertEqual(api.search("camping"), ["Enamel Camping Mug"])

    def test_code_that_changes_the_store_directly(self):
        store, index, _ = shop()
        self.assertEqual([i.id for i in index.search("teapot")], ["t1"])
        store.update("t1", name="Brown Betty Tea Pot")
        self.assertEqual([i.id for i in index.search("teapot")], [])
        store.add(Item("t2", "Glass Teapot", 3100, ("tea",)))
        self.assertEqual([i.id for i in index.search("teapot")], ["t2"])

    def test_the_cache_still_caches(self):
        _, index, api = shop()
        index.search("mug")
        index.search("Mug")
        self.assertEqual(index.computed, 1)
        api.rename_item("m1", "Enamel Cup")
        index.search("mug")
        index.search("mug")
        self.assertEqual(index.computed, 2)


if __name__ == "__main__":
    unittest.main()
