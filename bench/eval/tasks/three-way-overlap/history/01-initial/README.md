# basket

Checkout pricing for the web shop: what a cart costs in each storefront.

- `src/catalog.ts`: products, tax classes, weights.
- `src/store.ts`: storefronts (country, currency, tax rates, shipping).
- `src/cart/price.ts`: `priceCart`, as `docs/pricing.md` describes.

Node 22.18 or later, no dependencies, no build step: `npm test`.
