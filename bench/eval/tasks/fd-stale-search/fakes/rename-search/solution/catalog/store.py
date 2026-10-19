"""The one place items are added, changed and removed."""

from collections.abc import Callable
from dataclasses import replace

from .models import Item


class NotFound(KeyError):
    pass


class Store:
    def __init__(self) -> None:
        self._items: dict[str, Item] = {}
        self._listeners: list[Callable[[], None]] = []

    def subscribe(self, listener: Callable[[], None]) -> None:
        """Calls `listener` after every change to the items, whoever makes it."""
        self._listeners.append(listener)

    def _changed(self) -> None:
        for listener in self._listeners:
            listener()

    def add(self, item: Item) -> Item:
        if item.id in self._items:
            raise ValueError(f"item {item.id} exists already")
        self._items[item.id] = item
        self._changed()
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
        self._changed()
        return item

    def delete(self, item_id: str) -> None:
        self.get(item_id)
        del self._items[item_id]
        self._changed()

    def all(self) -> list[Item]:
        return list(self._items.values())

    def __len__(self) -> int:
        return len(self._items)
