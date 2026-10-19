export type Address = {
  name: string;
  lines: string[];
  town: string;
  postcode: string;
  /** ISO 3166 alpha-2, `GB` for the United Kingdom. */
  country: string;
};

export class AddressError extends Error {}

/** A postcode as customers type it, tidied for printing and lookups: `sw1a1aa` is `SW1A 1AA`. */
export function normalizePostcode(raw: string): string {
  const compact = raw.replace(/\s+/g, "").toUpperCase();
  if (compact.length < 5) return compact;
  return `${compact.slice(0, -3)} ${compact.slice(-3)}`;
}

const UK_POSTCODE = /^(GIR 0AA|[A-Z]{1,2}\d[A-Z\d]? \d[A-Z]{2})$/;

/** Whether a (normalized) postcode is in one of the UK formats. */
export function isValidPostcode(postcode: string): boolean {
  return UK_POSTCODE.test(postcode);
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
