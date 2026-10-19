# Billing rules

This is the reference for how Tally turns line items into an invoice. Finance
signs off on changes to this file; the code in `src/invoice.ts` follows it.

## Amounts

- Every amount is an integer number of the currency's minor unit (cents for
  EUR, GBP, USD and SEK; whole yen for JPY). Floats never hold money.
- A line's **amount** is `quantity × unitPrice`. Quantities are whole units and
  unit prices are before tax.
- The invoice **subtotal** is the sum of the line amounts.

## Discount codes

An invoice may carry one discount code (`src/discount.ts`). Codes apply to the
invoice as a whole, never to a single line.

1. **Discount total.**
   - A `percent` code takes `subtotal × rate`, rounded half up to a whole minor
     unit (the same rounding as tax).
   - A `fixed` code takes its `amount`, **capped at the subtotal**. A voucher
     worth more than the order brings the order to zero; the unused value is
     not paid out, carried to tax, or turned into a negative invoice.
2. **Allocation to lines.** The discount total is allocated across the lines
   in proportion to their amounts, so every line, whatever its tax category,
   carries its share:
   - Each line's share starts as the exact share `discountTotal × lineAmount /
     subtotal`, rounded down to a whole minor unit.
   - The minor units still unallocated go one at a time to the lines with the
     largest remainder (the fraction dropped by rounding down). Lines with
     equal remainders are served in invoice order, earliest first.
   - The shares therefore add up exactly to the discount total. A line with an
     amount of 0 gets no share. `allocate` in `src/money.ts` implements this
     rule.
3. **Taxable amount.** A line's taxable amount is its amount minus its share
   of the discount.

Without a discount code, every line's share is 0 and its taxable amount is its
amount.

## Tax

- Each line has a tax category (`standard`, `reduced`, `zero`, `exempt`). The
  rate comes from the invoice's region (`src/tax.ts`). `zero` and `exempt` are
  both 0% but appear separately in the VAT return.
- Tax is computed **per line on the taxable amount**: `taxable × rate`,
  rounded **half up** to a whole minor unit. Rates are stored in basis points,
  so the computation is integer arithmetic:
  `floor((taxable × rateBps + 5000) / 10000)`.
- The invoice **tax total** is the sum of the line taxes. Tax is never
  computed on the subtotal, because per-line rounding is what the tax office
  checks against.
- VAT is due on what the customer actually pays. Charging tax on the
  pre-discount amount overcharges the customer and overstates our VAT return.

## Totals

- `total = subtotal − discountTotal + taxTotal`.
- The invoice exposes the per-line breakdown (`amount`, `discount`, `taxable`,
  `taxRate`, `tax`) because the printed invoice shows it and the VAT return is
  built from it.

## Worked examples

### No discount

Region GB, two lines:

| Line | Amount | Category | Rate | Tax |
|---|---:|---|---:|---:|
| Desk lamp | 4,999 | standard | 20% | 1,000 (999.8 rounds up) |
| Children's book | 1,250 | zero | 0% | 0 |

Subtotal 6,249, tax total 1,000, total 7,249.

### Percent code across two rates

Region GB, code `SPRING25` (25%):

| Line | Amount | Discount | Taxable | Rate | Tax |
|---|---:|---:|---:|---:|---:|
| Desk lamp | 6,000 | 1,500 | 4,500 | 20% | 900 |
| Child car seat | 4,000 | 1,000 | 3,000 | 5% | 150 |

Subtotal 10,000, discount 2,500, tax total 1,050, total 8,550.

### Leftover cent

Three lines of 1,000 each and a fixed voucher of 1,000: the exact shares are
333.33 each, so each line starts at 333 and the one leftover unit goes to the
first line (equal remainders, invoice order). Shares: 334, 333, 333.

### Voucher larger than the order

Subtotal 3,000 and a fixed voucher of 5,000: the discount total is 3,000,
every line's taxable amount is 0, tax is 0, and the total is 0.
