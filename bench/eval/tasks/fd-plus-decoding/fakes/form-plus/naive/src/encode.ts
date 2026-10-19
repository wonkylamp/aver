// Percent-encoding of one URI component (RFC 3986). Public API: see README.

/** Decodes `%XX` sequences and `+` as a space. */
export function decode(component: string): string {
  return decodeURIComponent(component.replaceAll("+", " "));
}

/** Encodes everything but RFC 3986's unreserved characters. */
export function encode(component: string): string {
  return encodeURIComponent(component).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`);
}
