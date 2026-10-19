"""The nightly import of the supplier's CSV: adds new items, updates known ones."""

import csv
import io

from .models import Item
from .store import Store


def parse_rows(text: str) -> list[dict[str, str]]:
    return [dict(r) for r in csv.DictReader(io.StringIO(text))]


def apply_changes(store: Store, rows: list[dict[str, str]]) -> dict[str, int]:
    """Rows have `id`, `name`, `price_pence` and `tags` (separated by `|`)."""
    added = updated = 0
    for row in rows:
        tags = tuple(t for t in row.get("tags", "").split("|") if t)
        fields = {"name": row["name"], "price_pence": int(row["price_pence"]), "tags": tags}
        try:
            store.get(row["id"])
        except KeyError:
            store.add(Item(row["id"], **fields))
            added += 1
        else:
            store.update(row["id"], **fields)
            updated += 1
    return {"added": added, "updated": updated}
