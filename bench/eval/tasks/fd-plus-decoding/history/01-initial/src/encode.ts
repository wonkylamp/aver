// Percent-encoding of one URI component (RFC 3986). Public API: see README.

/** Decodes `%XX` sequences; everything else, `+` included, is left as it is. */
export function decode(component: string): string {
  return decodeURIComponent(component);
}

/** Encodes everything but RFC 3986's unreserved characters. */
export function encode(component: string): string {
  return encodeURIComponent(component).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`);
}
