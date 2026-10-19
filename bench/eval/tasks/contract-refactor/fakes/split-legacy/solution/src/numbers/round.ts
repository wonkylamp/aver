/** Rounds half away from zero to `digits` decimals. */
export function roundTo(x: number, digits = 0): number {
  const f = 10 ** digits;
  return (Math.sign(x) * Math.round(Math.abs(x) * f + 1e-9)) / f;
}
