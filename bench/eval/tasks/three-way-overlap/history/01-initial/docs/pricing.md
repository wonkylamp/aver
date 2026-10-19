# Pricing a cart

`priceCart(cart, store)` (`src/cart/price.ts`) turns a cart into the quote
the checkout page shows.

1. **Lines.** Each line's amount is the unit price times the quantity.
2. **Shipping.** A flat rate per order plus a charge per started kilogram
   above the first. Merchandise (everything but gift cards) at or above the
   store's threshold ships free. A cart with nothing to ship (gift cards
   only) pays no shipping.
3. **Tax.** Each line is taxed at its tax class's rate, rounded half up per
   line; shipping is taxed at the standard rate. Prices are shown before
   tax and tax is added on top.
4. **Total.** Lines plus shipping plus tax.

Amounts are integer minor units. Gift cards are zero-rated and weigh
nothing.
