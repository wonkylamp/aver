export type Address = {
  name: string;
  lines: string[];
  town: string;
  postcode: string;
  /** ISO 3166 alpha-2, `GB` for the United Kingdom. */
  country: string;
};

export class AddressError extends Error {}

/** A postcode as customers type it, tidied for printing and lookups. */
export function normalizePostcode(raw: string): string {
  return raw.trim().toUpperCase();
}

/** Whether a (normalized) postcode looks like a UK postcode. */
export function isValidPostcode(postcode: string): boolean {
  return /^[A-Z0-9 ]{5,8}$/.test(postcode);
}

/** The outward code: the part before the space (`SW1A` of `SW1A 1AA`). */
export function outward(postcode: string): string {
  const at = postcode.indexOf(" ");
  return at < 0 ? postcode : postcode.slice(0, at);
}

/** The postcode area: the letters the outward code starts with (`SW` of `SW1A`). */
export function area(postcode: string): string {
  return /^[A-Z]+/.exec(outward(postcode))?.[0] ?? "";
}

/** The postcode district number (`1` of `SW1A`, `15` of `KW15`), or null. */
export function district(postcode: string): number | null {
  const m = /^[A-Z]+(\d+)/.exec(outward(postcode));
  return m ? Number(m[1]) : null;
}

export function isDomestic(a: Address): boolean {
  return a.country === "GB";
}

/** Checks an address before it is quoted or labelled. */
export function checkAddress(a: Address): void {
  if (a.name.trim() === "") throw new AddressError("an address needs a name");
  if (a.lines.length === 0 || a.lines.length > 3) throw new AddressError("an address has one to three lines");
  if (isDomestic(a) && !isValidPostcode(normalizePostcode(a.postcode))) throw new AddressError(`not a UK postcode: ${a.postcode}`);
}
