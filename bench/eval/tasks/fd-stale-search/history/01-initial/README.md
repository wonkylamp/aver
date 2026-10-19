# catalog

The shop's product catalog.

- `catalog/models.py`: an `Item` (id, name, price in pence, tags).
- `catalog/store.py`: the `Store` that holds the items. It is the one place
  items are added, changed and removed.
- `catalog/search.py`: the `SearchIndex` over a store. Searches are cached,
  because the storefront repeats the same few queries all day.
- `catalog/api.py`: the admin API the back office calls.
- `catalog/importer.py`: the nightly CSV import from the supplier, which
  adds and updates items in bulk.

Search is always current: every change to the store, whether it comes
through the admin API, the importer or code that calls the `Store`
directly, shows in search results at once.

Standard library only. Run the tests with `python3 -m unittest`.
