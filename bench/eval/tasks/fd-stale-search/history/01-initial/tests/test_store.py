import unittest

from catalog.models import Item
from catalog.store import NotFound, Store


class StoreTest(unittest.TestCase):
    def test_add_and_get(self):
        store = Store()
        store.add(Item("t1", "Teapot", 2400))
        self.assertEqual(store.get("t1").name, "Teapot")
        self.assertEqual(len(store), 1)

    def test_ids_are_unique(self):
        store = Store()
        store.add(Item("t1", "Teapot", 2400))
        with self.assertRaises(ValueError):
            store.add(Item("t1", "Other", 1))

    def test_update_replaces_fields(self):
        store = Store()
        store.add(Item("t1", "Teapot", 2400, ("tea",)))
        item = store.update("t1", price_pence=2200)
        self.assertEqual(item, Item("t1", "Teapot", 2200, ("tea",)))
        self.assertEqual(store.get("t1").price_pence, 2200)

    def test_delete_and_missing_items(self):
        store = Store()
        store.add(Item("t1", "Teapot", 2400))
        store.delete("t1")
        with self.assertRaises(NotFound):
            store.get("t1")
        with self.assertRaises(NotFound):
            store.delete("t1")

    def test_items_are_checked(self):
        with self.assertRaises(ValueError):
            Item("", "Teapot", 1)
        with self.assertRaises(ValueError):
            Item("t1", " ", 1)
        with self.assertRaises(ValueError):
            Item("t1", "Teapot", -1)


if __name__ == "__main__":
    unittest.main()
