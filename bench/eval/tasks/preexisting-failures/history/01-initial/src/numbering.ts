export type InvoiceNumber = { prefix: string; year: number; sequence: number };

const PATTERN = /^([A-Z]{2,5})-(\d{4})-(\d{4,})$/;

/** "INV-2024-0042". Sequences restart every year. */
export function formatInvoiceNumber(prefix: string, year: number, sequence: number): string {
  if (!/^[A-Z]{2,5}$/.test(prefix)) throw new RangeError(`invalid invoice prefix: ${prefix}`);
  if (!Number.isSafeInteger(sequence) || sequence < 1) throw new RangeError(`invalid sequence: ${sequence}`);
  return `${prefix}-${year}-${String(sequence).padStart(4, "0")}`;
}

export function parseInvoiceNumber(value: string): InvoiceNumber | null {
  const match = PATTERN.exec(value);
  if (match === null) return null;
  return { prefix: match[1], year: Number(match[2]), sequence: Number(match[3]) };
}

/** The number after the highest existing one with the same prefix and year. */
export function nextInvoiceNumber(existing: readonly string[], prefix: string, year: number): string {
  let highest = 0;
  for (const value of existing) {
    const parsed = parseInvoiceNumber(value);
    if (parsed && parsed.prefix === prefix && parsed.year === year) {
      highest = Math.max(highest, parsed.sequence);
    }
  }
  return formatInvoiceNumber(prefix, year, highest + 1);
}
