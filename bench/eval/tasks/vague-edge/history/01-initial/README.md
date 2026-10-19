# pulse-feed

The activity feed behind the "Activity" tab: who created, updated, commented
on or shared what, newest first.

This repository holds the domain logic. The HTTP server and the Postgres
repository live in the platform repo and plug into the interfaces here.

## Layout

- `src/types.ts`: the activity model.
- `src/users/`: the user directory (names, handles, mention search).
- `src/permissions.ts`: who may see which activity.
- `src/format/`: summaries and relative timestamps for the feed UI.

## Development

Node 22.18 or later. No dependencies and no build step: TypeScript runs
through Node's type stripping.

```sh
npm test
```
