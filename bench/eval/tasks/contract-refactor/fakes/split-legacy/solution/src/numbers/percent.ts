/**
 * `part` as a whole percentage of `whole`, bounded to 0-100: 1 of 3 is 33.
 * Zero of zero is 0; NaN in, NaN out, so the dashboard shows "n/a" instead
 * of a made-up number (which is why this does not use clamp).
 */
export function percent(part: number, whole: number): number {
  if (whole === 0) return part === 0 ? 0 : 100;
  return Math.min(100, Math.max(0, Math.round((part / whole) * 100)));
}
