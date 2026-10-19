# tabs

Shared expenses for a group of friends: record who paid for what and who
it was for, see each member's balance, and settle up with as few transfers
as possible.

- `src/money.ts`: pence, parsing and formatting.
- `src/split.ts`: splitting an amount evenly or by shares.
- `src/group.ts`: a group, its expenses and refunds, and the balances.
- `src/settle.ts`: the transfers that settle a group.

A refund (the shop gives money back for something the group bought) is an
expense with a negative amount, split among the same people, so balances
always add up to zero. Run the tests with `npm test`.
