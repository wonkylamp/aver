/** A deliberately loose check: something@something.tld, no spaces. */
export function isEmail(text: string): boolean {
  return /^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(text.trim());
}

/** UK postcodes in their usual forms ("SW1A 1AA", "sw1a1aa", "M1 1AE"). */
export function isPostcodeUK(text: string): boolean {
  return /^(GIR ?0AA|[A-Z]{1,2}\d[A-Z\d]? ?\d[A-Z]{2})$/i.test(text.trim());
}
