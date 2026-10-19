# Billing rules

This is the reference for how Tally turns line items into an invoice. Finance
signs off on changes to this file; the code in `src/invoice.ts` follows it.

## Amounts

- Every amount is an integer number of the currency's minor unit (cents for
  EUR, GBP, USD and SEK; whole yen for JPY). Floats never hold money.
- A line's **amount** is `quantity × unitPrice`. Quantities are whole units and
  unit prices are before tax.
- The invoice **subtotal** is the sum of the line amounts.

## Tax

- Each line has a tax category (`standard`, `reduced`, `zero`, `exempt`). The
  rate comes from the invoice's region (`src/tax.ts`). `zero` and `exempt` are
  both 0% but appear separately in the VAT return.
- Tax is computed **per line**: `amount × rate`, rounded **half up** to a whole
  minor unit. Rates are stored in basis points, so the computation is integer
  arithmetic: `floor((amount × rateBps + 5000) / 10000)`.
- The invoice **tax total** is the sum of the line taxes. Tax is never
  computed on the subtotal, because per-line rounding is what the tax office
  checks against.

## Totals

- `total = subtotal + taxTotal`.

## Worked example

Region GB, two lines:

| Line | Amount | Category | Rate | Tax |
|---|---:|---|---:|---:|
| Desk lamp | 4,999 | standard | 20% | 1,000 (999.8 rounds up) |
| Children's book | 1,250 | zero | 0% | 0 |

Subtotal 6,249, tax total 1,000, total 7,249.
