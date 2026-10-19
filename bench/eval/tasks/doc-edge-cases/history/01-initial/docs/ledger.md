# The ledger

Double-entry bookkeeping, kept deliberately small.

- **Accounts** have a four-digit code and a type: asset, liability,
  equity, income or expense. `STANDARD_CHART` is what a new company starts
  with.
- **Entries** have a date, a payee and two or more postings. A posting is
  signed integer cents: positive is a debit, negative a credit. The
  postings of an entry sum to zero, or `Journal.post` refuses it.
- **Reconciled** marks an entry matched against a bank statement.
- **The trial balance** nets each account's postings up to a date and lists
  debit balances and credit balances; their totals are equal when the
  books are sound.

Amounts are integer cents everywhere. `parseAmount` reads what people
type; `formatAmount` and `formatMoney` write for people (thousands
separators included).
