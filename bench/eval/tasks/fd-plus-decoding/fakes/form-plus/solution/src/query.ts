// Query strings and form bodies. Public API: see README.
import { decode, encode } from "./encode.ts";

export type Query = Record<string, string | string[]>;

/** application/x-www-form-urlencoded: `+` is a space, then percent-decoding. */
function formDecode(s: string): string {
  return decode(s.replaceAll("+", " "));
}

export function parseQuery(query: string): Query {
  const out: Query = {};
  const text = query.startsWith("?") ? query.slice(1) : query;
  for (const pair of text.split("&")) {
    if (pair === "") continue;
    const at = pair.indexOf("=");
    const key = formDecode(at < 0 ? pair : pair.slice(0, at));
    const value = at < 0 ? "" : formDecode(pair.slice(at + 1));
    const prev = out[key];
    if (prev === undefined) out[key] = value;
    else if (Array.isArray(prev)) prev.push(value);
    else out[key] = [prev, value];
  }
  return out;
}

export function stringifyQuery(query: Query): string {
  const parts: string[] = [];
  for (const [key, value] of Object.entries(query)) {
    for (const v of Array.isArray(value) ? value : [value]) parts.push(`${encode(key)}=${encode(v)}`);
  }
  return parts.join("&");
}

/** The first value of a key, or undefined. */
export function first(query: Query, key: string): string | undefined {
  const v = query[key];
  return Array.isArray(v) ? v[0] : v;
}
