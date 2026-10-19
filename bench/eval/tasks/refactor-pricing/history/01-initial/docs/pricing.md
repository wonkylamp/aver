# Pricing rules

All amounts are integer cents.

## Line price

A line is a SKU, its unit price and a quantity. Its price is the unit price
times the quantity, less a discount in percent made of:

1. **Bulk tier**, by quantity on the line:

   | Quantity | Discount |
   |---|---|
   | 10 or more | 5% |
   | 50 or more | 10% |
   | 100 or more | 15% |

   Only the highest tier that applies counts.

2. **Member discount**: 3% for customers in the trade programme.

3. **Coupon**: the coupon's `percentOff`, on lines in the coupon's
   `category`, or on every line when the coupon has no category.

The three add up, and the total discount on a line is capped at 30%.

## Totals

The subtotal is the sum of the line prices. Shipping is charged on the
subtotal (see `src/shipping/`), and tax on subtotal plus shipping (see
`src/tax/`).
