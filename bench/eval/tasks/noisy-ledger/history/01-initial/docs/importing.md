# Importing statements

People download a CSV statement from their bank's website every few weeks
and import it into an account. The date ranges of those downloads overlap
all the time ("last 90 days", every month), so importing must be
idempotent: a transaction already in the account is never added again.

## Formats

`readStatement` (`src/import/formats.ts`) reads Monzo, Barclays, Revolut
and Starling statements into rows of date, amount, payee and memo.
Revolut also says whether a row is pending or was reverted; only booked
rows are imported.

## Which rows are already in the account

- **Rows with a bank id.** Monzo gives every transaction an id. A row whose
  id is already on a transaction of the account is a duplicate.
- **Rows without one** (every other bank) are matched on their
  *fingerprint*: the date, the amount, and the payee as the bank printed
  it, compared with `normalizePayee` (case and spacing differ between
  downloads of the same statement). The payee the user sees can be renamed,
  so the comparison uses the bank's own payee, kept on each imported
  transaction as `source.payee`.
- **Identical transactions are real.** Two coffees of the same price at the
  same shop on the same day are two transactions, and a statement lists
  both. So fingerprints are counted, not just compared: if the account
  already has *k* transactions with a fingerprint and the statement has
  *m* rows with it, the first *k* of those rows are duplicates and the
  remaining *m - k* are imported.
- Matching is per account. The same purchase in two accounts (a shared
  card, a transfer) is two transactions.

An import reports every row it skipped and why: `pending`, `reverted` or
`duplicate`.

## After the import

New transactions get a display payee (`displayPayee`), which the user may
rename, and then go through the categorisation rules (`src/rules.ts`).
