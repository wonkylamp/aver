# Feed API

## GET /v1/feed

Returns the activities the signed-in user may see, one page at a time.

| Parameter | Default | Meaning |
|---|---|---|
| `limit` | 20 | Page size, 1 to 100. |
| `order` | `desc` | `desc` is newest first; `asc` is oldest first. |
| `cursor` | none | The `nextCursor` of the previous page. |
| `verb` | all | Comma-separated verbs to include, e.g. `commented,shared`. |
| `actor` | all | Only activities by this user id. |

Filters apply before paging, so a page holds up to `limit` matching
activities. Keep the same filters while following `nextCursor`.

Response:

```json
{
  "items": [{ "id": "act_0000002s", "verb": "commented", "summary": "Ana Lima commented on task \"Fix login\"", "createdAt": "2024-01-15T09:00:00.000Z" }],
  "nextCursor": "eyJ2IjoyLCJ0IjoxNzA1MzA5MjAwMDAwLCJpZCI6ImFjdF8wMDAwMDAycyJ9"
}
```

### Order

Activities are ordered by `createdAt`. Several activities can share a
`createdAt` (bulk imports and integrations write many at once), so activities
with the same `createdAt` are ordered by `id`. `desc` is the exact reverse of
`asc`. The order is total: every activity has exactly one place in it.

### Pagination

Request the first page without a cursor, then pass each page's `nextCursor`
as `cursor` until `nextCursor` is `null`. Walking the pages this way returns
every visible activity exactly once, in order, provided the feed does not
change during the walk.

Errors: a malformed cursor returns `400 {"error": "invalid_cursor"}`; a bad
`limit`, `order` or `verb` returns
`400 {"error": "invalid_parameter", "parameter": ..., "message": ...}`; an
unauthenticated request returns `401`.

### Cursor format and compatibility

Cursors are opaque to clients, but clients keep them: the mobile and desktop
apps persist the last cursor they received so the feed resumes where the user
left off, even across app updates. **The API must accept every cursor format
it has ever issued, and continue from the same place.** A new format needs a
new version number; old versions stay decodable.

| Version | Encoding | Meaning |
|---|---|---|
| 1 | base64url of the JSON `{"v":1,"t":<createdAt in ms>}` | The next page starts with the first activity, in the requested order, whose `createdAt` is strictly after `t`. No longer issued; still accepted. |
| 2 | base64url of the JSON `{"v":2,"t":<createdAt in ms>,"id":<activity id>}` | The position of the last activity on the page. The next page starts with the activity right after it in the requested order, so activities sharing its `createdAt` are neither skipped nor repeated. |
