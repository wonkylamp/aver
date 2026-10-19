// Small helpers for the newer functions.

/** Bounds `x` to [min, max]. */
export function clamp(x: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, x));
}

/**
 * `part` as a whole percentage of `whole`, bounded to 0-100: 1 of 3 is 33.
 * Zero of zero is 0; NaN in, NaN out, so the dashboard shows "n/a" instead
 * of a made-up number.
 */
export function percent(part: number, whole: number): number {
  if (whole === 0) return part === 0 ? 0 : 100;
  return clamp(Math.round((part / whole) * 100), 0, 100);
}
