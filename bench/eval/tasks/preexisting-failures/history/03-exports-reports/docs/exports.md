# Exports and revenue figures

## Ledger export (`src/export.ts`)

The ledger import takes two CSV files, both RFC 4180 with CRLF line endings:

- `invoiceLinesCsv`: one row per invoice line, with the line's discount share
  and tax.
- `invoiceSummaryCsv`: one row per invoice.

Amounts are plain decimals in the currency's major unit (`1234.50`, no symbol,
no grouping). Tax rates are written in basis points.

Known issue #212: descriptions containing a double quote are not escaped yet,
so the importer rejects those rows. Until it is fixed, support edits the
description by hand.

## Monthly revenue (`monthlyRevenue`)

Groups invoices by issue month and currency. `net` is the subtotal minus the
discount, `tax` is the tax total and `gross` the invoice total. Amounts in
different currencies are never added together.

## Quarterly summary (`quarterlySummary`)

Planned for the quarterly VAT filing (#230). Same grouping as the monthly
figures but by calendar quarter (`2024-Q1` is January to March), sorted by
quarter and then currency. Not implemented yet; the test in
`test/report.test.ts` was written first and describes the expected output.
