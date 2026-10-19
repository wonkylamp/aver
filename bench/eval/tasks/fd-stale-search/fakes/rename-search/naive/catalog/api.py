"""The admin API the back office calls."""

from .models import Item
from .search import SearchIndex
from .store import Store


class Api:
    def __init__(self, store: Store, index: SearchIndex) -> None:
        self._store = store
        self._index = index

    def create_item(self, item_id: str, name: str, price_pence: int, tags: list[str] | None = None) -> dict:
        item = self._store.add(Item(item_id, name, price_pence, tuple(tags or ())))
        self._index.invalidate()
        return self._json(item)

    def rename_item(self, item_id: str, name: str) -> dict:
        item = self._store.update(item_id, name=name)
        self._index.invalidate()
        return self._json(item)

    def set_price(self, item_id: str, price_pence: int) -> dict:
        return self._json(self._store.update(item_id, price_pence=price_pence))

    def delete_item(self, item_id: str) -> None:
        self._store.delete(item_id)

    def get_item(self, item_id: str) -> dict:
        return self._json(self._store.get(item_id))

    def search(self, query: str) -> list[str]:
        return [i.name for i in self._index.search(query)]

    @staticmethod
    def _json(item: Item) -> dict:
        return {"id": item.id, "name": item.name, "price_pence": item.price_pence, "tags": list(item.tags)}
