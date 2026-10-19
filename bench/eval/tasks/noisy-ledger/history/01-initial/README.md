# tally

A personal finance ledger: import bank statements, categorise spending,
set budgets, and see where the money went.

## Layout

- `src/money.ts`, `src/dates.ts`, `src/csv.ts`: amounts, calendar dates
  and the CSV reader.
- `src/ledger.ts`: accounts and transactions.
- `src/import/`: bank statement formats and the importer
  (`docs/importing.md`).
- `src/rules.ts`: categorisation rules.

## Development

Node 22.18 or later, no dependencies, no build step.

```sh
npm test
```
