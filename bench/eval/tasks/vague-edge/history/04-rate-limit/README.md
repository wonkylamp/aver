# pulse-feed

The activity feed behind the "Activity" tab: who created, updated, commented
on or shared what, newest first.

This repository holds the domain logic and the request handler. The HTTP
server and the Postgres repository live in the platform repo and plug into the
interfaces here (`ActivityRepository`, `createFeedHandler`).

The public contract of `GET /v1/feed`, including ordering, pagination and the
cursor compatibility promise, is in [docs/api.md](docs/api.md).

## Layout

- `src/types.ts`: the activity model.
- `src/feed/`: ordering, cursors, filters, the repository contract with an
  in-memory implementation, and `FeedService`, which pages through what a
  viewer may see.
- `src/http/`: the `GET /v1/feed` handler and query parameter parsing.
- `src/users/`: the user directory (names, handles, mention search).
- `src/permissions.ts`: who may see which activity.
- `src/format/`: summaries and relative timestamps for the feed UI.
- `src/rate-limit.ts`: per-user token bucket used by the handler.
- `src/testing/`: fixtures shared by tests.

## Development

Node 22.18 or later. No dependencies and no build step: TypeScript runs
through Node's type stripping.

```sh
npm test
```
