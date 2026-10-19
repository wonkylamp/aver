// Tracking numbers in the UPU S10 format: two letters, an eight-digit
// serial, a check digit and the country, e.g. JD000001244GB.

const WEIGHTS = [8, 6, 4, 2, 3, 5, 9, 7];

export class IdError extends Error {}

/** The S10 check digit of an eight-digit serial. */
export function checkDigit(serial: string): number {
  if (!/^\d{8}$/.test(serial)) throw new IdError(`not an eight-digit serial: ${serial}`);
  const total = [...serial].reduce((acc, c, i) => acc + Number(c) * WEIGHTS[i]!, 0);
  return 11 - (total % 11);
}

export function trackingNumber(serial: number, prefix = "JD"): string {
  if (!Number.isInteger(serial) || serial < 0 || serial > 99_999_999) throw new IdError(`serial out of range: ${serial}`);
  if (!/^[A-Z]{2}$/.test(prefix)) throw new IdError(`bad prefix: ${prefix}`);
  const digits = String(serial).padStart(8, "0");
  return `${prefix}${digits}${checkDigit(digits)}GB`;
}

export function isValidTrackingNumber(s: string): boolean {
  const m = /^[A-Z]{2}(\d{8})(\d)GB$/.exec(s);
  return m !== null && checkDigit(m[1]!) === Number(m[2]);
}
