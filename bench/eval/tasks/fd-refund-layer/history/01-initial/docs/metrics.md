# Metrics

One definition of each number, computed in `src/metrics.ts` and shown by
the report, the dashboard API and the finance export alike.

- **Orders**: orders placed in the period, refunded or not.
- **Gross**: the sum of those orders' line amounts (quantity × unit price).
- **Revenue**: what we keep from those orders: gross minus their refunds,
  whenever the refunds were made. Revenue is always net of refunds; the
  owners and finance reconcile it against the bank.
- **Average order**: revenue divided by orders, rounded half up to the
  penny; zero without orders.
- **Daily revenue**: revenue by the day the order was placed.
- **Top products**: units and line amounts per SKU (gross: refunds are per
  order, not per product).

Amounts are integer pence. A month is `YYYY-MM`.
