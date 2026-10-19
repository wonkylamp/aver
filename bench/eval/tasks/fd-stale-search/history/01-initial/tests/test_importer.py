import unittest

from catalog.importer import apply_changes, parse_rows

from .samples import shop

CSV = """id,name,price_pence,tags
m1,Enamel Mug,950,kitchen
j1,Milk Jug,1100,kitchen|handmade
"""


class ImporterTest(unittest.TestCase):
    def test_rows(self):
        self.assertEqual(parse_rows(CSV)[1], {"id": "j1", "name": "Milk Jug", "price_pence": "1100", "tags": "kitchen|handmade"})

    def test_adds_and_updates(self):
        store, _, _ = shop()
        self.assertEqual(apply_changes(store, parse_rows(CSV)), {"added": 1, "updated": 1})
        self.assertEqual(store.get("m1").price_pence, 950)
        self.assertEqual(store.get("j1").tags, ("kitchen", "handmade"))
        self.assertEqual(len(store), 6)


if __name__ == "__main__":
    unittest.main()
