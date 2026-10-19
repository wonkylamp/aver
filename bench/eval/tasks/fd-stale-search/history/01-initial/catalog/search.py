"""Search over a store's items, with a cache of results per query."""

from .models import Item
from .store import Store


def words(text: str) -> list[str]:
    return [w for w in "".join(c.lower() if c.isalnum() else " " for c in text).split() if w]


def matches(item: Item, query_words: list[str]) -> bool:
    haystack = words(item.name) + [t.lower() for t in item.tags]
    return all(any(h.startswith(q) for h in haystack) for q in query_words)


class SearchIndex:
    def __init__(self, store: Store) -> None:
        self._store = store
        self._cache: dict[tuple[str, ...], list[Item]] = {}
        self.computed = 0

    def search(self, query: str) -> list[Item]:
        """Items whose name words or tags start with every word of the query, by name."""
        key = tuple(words(query))
        if key not in self._cache:
            self.computed += 1
            found = [i for i in self._store.all() if key and matches(i, list(key))]
            self._cache[key] = sorted(found, key=lambda i: (i.name.lower(), i.id))
        return list(self._cache[key])

    def invalidate(self) -> None:
        self._cache.clear()
