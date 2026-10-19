import unittest

from catalog.search import words

from .samples import shop


class SearchTest(unittest.TestCase):
    def test_words(self):
        self.assertEqual(words("Brown Betty's Teapot!"), ["brown", "betty", "s", "teapot"])

    def test_by_name_word_prefix(self):
        _, index, _ = shop()
        self.assertEqual([i.id for i in index.search("mug")], ["m1", "m2"])
        self.assertEqual([i.id for i in index.search("stove")], ["k1"])

    def test_every_word_must_match_a_name_word_or_tag(self):
        _, index, _ = shop()
        self.assertEqual([i.id for i in index.search("tea")], ["t1", "c1", "k1"])
        self.assertEqual([i.id for i in index.search("tea kettle")], ["k1"])
        self.assertEqual(index.search("handmade mug")[0].id, "m2")

    def test_empty_and_unknown_queries(self):
        _, index, _ = shop()
        self.assertEqual(index.search(""), [])
        self.assertEqual(index.search("sofa"), [])

    def test_results_are_cached(self):
        _, index, _ = shop()
        index.search("mug")
        index.search("MUG")
        index.search("mug ")
        self.assertEqual(index.computed, 1)


if __name__ == "__main__":
    unittest.main()
