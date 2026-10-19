# urlkit

Small, dependency-free URL, query string and cookie helpers, used by our
web services and published for other teams. Everything below is public API
and follows semver: a change in behaviour of any of it is a breaking change.

```ts
import { decode, encode, parseQuery, stringifyQuery, parseUrl, formatUrl, parseCookies, joinPath } from "urlkit";
```

## `decode(component)` and `encode(component)`

Percent-decoding and -encoding of one URI component, as RFC 3986 defines
it. `decode` turns `%XX` sequences into characters and leaves everything
else alone: in particular `+` is a literal plus sign (`decode("c++")` is
`c++`), because that is what it means in paths, fragments and cookies.
Malformed sequences (`%E0%A4%A`) throw a `URIError`.

## `parseQuery(query)` and `stringifyQuery(object)`

The query string of a URL, or the body of a form posted as
`application/x-www-form-urlencoded`: `a=1&b=x%20y&a=2` is
`{ a: ["1", "2"], b: "x y" }`. A leading `?` is ignored; a key without `=`
has the value `""`. `stringifyQuery` is its inverse.

## `parseUrl(url)` and `formatUrl(parts)`

A path-and-query URL (`/docs/c%2B%2B/intro?lang=en#setup`): the decoded
path segments, the query (as `parseQuery`) and the decoded fragment.

## `parseCookies(header)`

A `Cookie` header: `a=1; b=x%3By` is `{ a: "1", b: "x;y" }`. Values are
decoded with `decode`.

## `joinPath(...segments)`

Joins and normalizes path segments: `joinPath("/a/", "../b", "c")` is
`/b/c`.

Run the tests with `npm test`.
