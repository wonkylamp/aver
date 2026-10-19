# shopstats

The shop's sales numbers, in three places:

- the monthly sales report emailed to the owners (`src/report.ts`);
- the dashboard's JSON API (`src/api.ts`);
- the daily revenue CSV that finance imports into the ledger (`src/export.ts`).

All three take their numbers from `src/metrics.ts`, so they agree; the
definitions are in docs/metrics.md. Orders and refunds are in
`src/orders.ts`; `src/testing/march.ts` is a copy of March 2026's orders
for the tests.

Run the tests with `npm test`. No dependencies.
