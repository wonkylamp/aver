"""The one place items are added, changed and removed."""

from dataclasses import replace

from .models import Item


class NotFound(KeyError):
    pass


class Store:
    def __init__(self) -> None:
        self._items: dict[str, Item] = {}

    def add(self, item: Item) -> Item:
        if item.id in self._items:
            raise ValueError(f"item {item.id} exists already")
        self._items[item.id] = item
        return item

    def get(self, item_id: str) -> Item:
        try:
            return self._items[item_id]
        except KeyError:
            raise NotFound(item_id) from None

    def update(self, item_id: str, **changes: object) -> Item:
        """Replaces fields of an item (Items are immutable): `update("t1", name="Teapot")`."""
        item = replace(self.get(item_id), **changes)
        self._items[item_id] = item
        return item

    def delete(self, item_id: str) -> None:
        self.get(item_id)
        del self._items[item_id]

    def all(self) -> list[Item]:
        return list(self._items.values())

    def __len__(self) -> int:
        return len(self._items)
