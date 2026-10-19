export class MoneyError extends Error {}

/** `12.34`, `12`, `-0.5` (pounds) as pence. */
export function parsePounds(text: string): number {
  const m = /^(-)?(\d+)(?:\.(\d{1,2}))?$/.exec(text.trim());
  if (!m) throw new MoneyError(`not an amount: ${text}`);
  const pence = Number(m[2]) * 100 + Number((m[3] ?? "0").padEnd(2, "0"));
  return m[1] ? -pence : pence;
}

/** `£12.34`, `-£0.05`. */
export function formatPence(pence: number): string {
  const sign = pence < 0 ? "-" : "";
  const abs = Math.abs(pence);
  return `${sign}£${Math.floor(abs / 100)}.${String(abs % 100).padStart(2, "0")}`;
}

export function sum(xs: number[]): number {
  return xs.reduce((a, b) => a + b, 0);
}
