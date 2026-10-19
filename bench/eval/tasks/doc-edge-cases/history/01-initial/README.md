# ledgerline

Bookkeeping for small businesses: a double-entry ledger, the chart of
accounts, the trial balance, and the exports finance teams download.

## Layout

- `src/money.ts`: amounts in integer cents, parsing and display.
- `src/ledger/`: accounts and journal entries.
- `src/reports/`: the trial balance.
- `src/export/`: the CSV writer and the downloadable exports.
- `docs/`: the ledger model and the export formats.

## Development

Node 22.18 or later, no dependencies, no build step.

```sh
npm test
```
