# hourglass

Time tracking and client billing for the agency: entries, rounding to the
client's increment, overtime, and the monthly invoices.

`docs/billing.md` is the source of truth for what we bill; finance signs
off on it.

## Development

Node 22.18 or later, no dependencies, no build step.

```sh
npm test
```

CI runs `npm test` on every push to main.
