# harbor-shop

Pricing, checkout and back-office code for the Harbor wholesale shop. Prices
are integer cents everywhere; nothing in this repository stores a float amount.

## Running the tests

```sh
npm test
```

No dependencies. TypeScript runs through Node's type stripping, so Node 22.18
or later is required.

## Layout

| Path | What it holds |
|---|---|
| `src/types.ts` | Shared types: lines, coupons, customers, carts. |
| `src/catalog/` | The product catalog and turning SKUs into priced lines. |
| `src/checkout/` | Cart totals at checkout: line pricing, shipping, tax. |
| `src/money/` | Formatting cents for display. |
| `src/tax/` | Sales tax by region. |
| `src/shipping/` | Shipping charges by region. |
| `src/inventory/` | Stock levels and reservations. |
| `src/orders/` | Order ids. |

The pricing rules are in [docs/pricing.md](docs/pricing.md).
