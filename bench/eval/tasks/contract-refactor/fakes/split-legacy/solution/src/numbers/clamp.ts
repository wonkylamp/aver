/** Bounds `x` to [min, max]; NaN counts as min, so a bad reading never escapes the range. */
export function clamp(x: number, min: number, max: number): number {
  if (min > max) throw new RangeError(`min ${min} is above max ${max}`);
  if (Number.isNaN(x)) return min;
  return Math.min(max, Math.max(min, x));
}

/** Maps `x` from [inMin, inMax] onto [outMin, outMax], clamped. */
export function scale(x: number, inMin: number, inMax: number, outMin: number, outMax: number): number {
  if (inMax === inMin) throw new RangeError("empty input range");
  const t = clamp((x - inMin) / (inMax - inMin), 0, 1);
  return outMin + t * (outMax - outMin);
}
