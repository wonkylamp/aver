# Quotes and invoice previews

## Quotes

Trade customers ask for a quote before a large order. `buildQuote` prices the
lines with the checkout rules ([pricing.md](pricing.md)) and holds the prices
for `validDays` (default 30) from the issue date. The quote shows each line's
discount so the customer can see where the bulk tier kicks in.

## Invoice previews

The back office checks an invoice before it is issued. `previewInvoice` prices
an order the way checkout did, adds shipping and tax for the order's region,
and renders the plain text used on the admin screen and in the email draft.

Both must agree with checkout to the cent: a quote that promised less than the
customer pays, or an invoice that differs from the charge, is a support ticket.
