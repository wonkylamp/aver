from catalog.api import Api
from catalog.models import Item
from catalog.search import SearchIndex
from catalog.store import Store


def shop() -> tuple[Store, SearchIndex, Api]:
    store = Store()
    for item in [
        Item("t1", "Brown Betty Teapot", 2400, ("tea", "kitchen")),
        Item("m1", "Enamel Mug", 900, ("kitchen",)),
        Item("m2", "Stoneware Mug", 1200, ("kitchen", "handmade")),
        Item("k1", "Stovetop Kettle", 3900, ("kitchen", "tea")),
        Item("c1", "Linen Tea Towel", 650, ("kitchen", "linen")),
    ]:
        store.add(item)
    index = SearchIndex(store)
    return store, index, Api(store, index)
