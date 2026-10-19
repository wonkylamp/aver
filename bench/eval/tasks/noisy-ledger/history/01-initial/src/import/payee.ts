// Payee names as banks print them are noisy: "TESCO STORES 2041  LONDON GB",
// "Tesco Stores 2041". normalizePayee gives the form used to compare them.

export function normalizePayee(raw: string): string {
  return raw.toLowerCase().replace(/\s+/g, " ").trim();
}

const CARD_PREFIX = /^(card payment to|contactless|pos|visa|dd|so)\s+/i;
const TRAILING_PLACE = /\s+(london|manchester|leeds|bristol|glasgow|edinburgh)(\s+gb)?$/i;

/** A display name for a raw payee: no card prefix or trailing town, title case. */
export function displayPayee(raw: string): string {
  const s = raw.replace(/\s+/g, " ").trim().replace(CARD_PREFIX, "").replace(TRAILING_PLACE, "").replace(/\s+\d{3,}$/, "");
  return s
    .toLowerCase()
    .split(" ")
    .map((w) => (w.length <= 2 && w !== "co" ? w.toUpperCase() : w[0]!.toUpperCase() + w.slice(1)))
    .join(" ");
}
