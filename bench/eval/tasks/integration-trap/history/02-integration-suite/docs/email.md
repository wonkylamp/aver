# Email in and out

## Outbound

Every notification (ticket opened, agent replied) is sent from
`help@support.relaydesk.test` with a `Reply-To` of the ticket's reply
address:

    reply+<token>@support.relaydesk.test

The token is made once per ticket by `src/tickets/tokens.ts`: 12 random
base62 characters. It is the only thing that ties an emailed reply to its
ticket, so it must be hard to guess; ticket numbers are not secret.

## Inbound

The mail provider posts every message for `support.relaydesk.test` to
`POST /webhooks/inbound`, in batches. `routeInbound` (`src/mail/inbound.ts`)
then:

1. drops bounces;
2. looks through `To` and `Cc` for one of our addresses;
3. for a reply address, finds the ticket by its token and adds the message,
   as the customer's when it comes from the requester and as a third
   party's otherwise; a customer's message reopens a closed ticket;
4. otherwise (mail to `help@`, or a reply address whose token we do not
   know) opens a new ticket, so that mail to support is never lost.

Quoted history ("On ... wrote:" and `>` lines) is stripped before the text
is stored.

## Comparing addresses

`normalizeAddress` (`src/email/address.ts`) is the one canonical form:
display name and surrounding space dropped, domain lowercased. Code that
compares two addresses, or uses one as a key, goes through it or through
`sameAddress`.
