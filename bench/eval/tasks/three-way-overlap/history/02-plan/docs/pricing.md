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

## Planned for the EU and Swiss launch

Three changes are planned, one ticket each. They touch the same function,
so this section fixes how they fit together; whoever builds one of them
builds it to fit the others.

### Promo codes (#61)

A store has a list of promo codes; a cart uses at most one
(`priceCart(cart, store, { promo: "CODE" })`; an unknown code is a
`PricingError`). A code is a percentage off or a fixed amount off, with an
optional minimum spend. It applies to merchandise only: never to gift
cards, never to shipping.

### Tax-inclusive prices (#62)

EU and Swiss stores show prices with tax included (`store.taxInclusive`).
Then the line amounts and the shipping charge already contain the tax:
the tax is extracted (`amount x rate / (100 + rate)`, rounded half up per
line and for shipping) rather than added, and the total is lines plus
shipping.

### Cash rounding (#63)

Swiss francs are paid to 5 centimes (`store.cashRounding = 5`). The total
is rounded half up to that increment as a separate rounding adjustment;
nothing else (lines, tax) changes.

### Order of operations

When more than one applies, a quote is computed in this order:

1. Line amounts.
2. The promo discount, allocated to the eligible lines: a percentage is
   taken off each eligible line (rounded half up per line); a fixed amount
   is capped at the eligible amount and split over the eligible lines in
   proportion to their amounts (largest remainder, ties to the earlier
   line). Each line then has a discounted amount.
3. Shipping, with the free-shipping threshold compared against the
   discounted merchandise. In a tax-inclusive store the threshold is an
   inclusive amount, like the prices.
4. Tax on each line's discounted amount, added on top or extracted
   (tax-inclusive), and tax on shipping.
5. The total: discounted lines plus shipping, plus tax when it is added on
   top.
6. Cash rounding of the total.
