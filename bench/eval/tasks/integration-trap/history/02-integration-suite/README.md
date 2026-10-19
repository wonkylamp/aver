# relay-desk

The helpdesk backend: customers sign up and sign in, open tickets by web
form or by email, and talk to support agents by replying to the
notifications we send.

## Layout

- `src/email/address.ts`: parsing and comparing email addresses.
- `src/accounts/`: customer accounts and sign-in.
- `src/tickets/`: tickets, their messages and reply tokens.
- `src/mail/`: outbound notifications and the inbound mail router.
- `src/http/app.ts`: the HTTP API (`node:http`, no framework).
- `integration/`: end-to-end tests against the running API and a simulated
  mail provider.
- `docs/`: how email flows through the system.

## Development

Node 22.18 or later. No dependencies and no build step: TypeScript runs
through Node's type stripping.

```sh
npm test                    # unit tests, a second or two
npm run test:integration    # end to end, about 30 s (the simulated
                            # provider delivers mail in 5 s batches)
npm start                   # the API on $PORT (default 8080)
```

CI runs both suites (see `aver.toml`).
