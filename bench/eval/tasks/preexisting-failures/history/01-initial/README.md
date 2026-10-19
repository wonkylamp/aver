# tally-billing

Invoice computation for the Tally billing service: money in integer minor
units, VAT by region and category, invoice totals, invoice numbering, payment
terms and instalment plans.

The library has no runtime dependencies. TypeScript runs directly through
Node's type stripping, so there is no build step.

```sh
npm test        # node --test
```

Node 22.18 or newer.

## Layout

| Path | What it holds |
|---|---|
| `src/money.ts` | `Cents`, basis-point percentages with half-up rounding, `allocate` |
| `src/currency.ts` | Supported currencies and their minor units |
| `src/tax.ts` | VAT rates by region and category |
| `src/line-items.ts` | Line item type and validation |
| `src/invoice.ts` | `computeInvoice`: line amounts, tax and totals |
| `src/numbering.ts` | Invoice numbers (`INV-2024-0042`) |
| `src/format.ts` | Display formatting for amounts |
| `src/dates.ts` | ISO date helpers and payment terms |
| `src/instalments.ts` | Splitting a total into instalments |

The billing rules are in [docs/billing.md](docs/billing.md). If the code and
that document disagree, the document is right.

## Usage

```ts
import { computeInvoice, formatMoney } from "./src/index.ts";

const invoice = computeInvoice({
  number: "INV-2024-0042",
  region: "DE",
  currency: "EUR",
  issueDate: "2024-03-15",
  lines: [{ sku: "LAMP-01", description: "Desk lamp", quantity: 2, unitPrice: 2_499, taxCategory: "standard" }],
});
formatMoney(invoice.total, invoice.currency); // "€59.48"
```
