// Path-and-query URLs. Public API: see README.
import { decode, encode } from "./encode.ts";
import { segments } from "./path.ts";
import type { Query } from "./query.ts";
import { parseQuery, stringifyQuery } from "./query.ts";

export type UrlParts = { path: string[]; query: Query; fragment: string };

export function parseUrl(url: string): UrlParts {
  let rest = url;
  let fragment = "";
  const hash = rest.indexOf("#");
  if (hash >= 0) {
    fragment = decode(rest.slice(hash + 1));
    rest = rest.slice(0, hash);
  }
  let query: Query = {};
  const q = rest.indexOf("?");
  if (q >= 0) {
    query = parseQuery(rest.slice(q + 1));
    rest = rest.slice(0, q);
  }
  return { path: segments(rest).map(decode), query, fragment };
}

export function formatUrl(parts: UrlParts): string {
  const path = `/${parts.path.map(encode).join("/")}`;
  const query = stringifyQuery(parts.query);
  return `${path}${query ? `?${query}` : ""}${parts.fragment ? `#${encode(parts.fragment)}` : ""}`;
}
