// Number helpers.

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

/** Rounds half away from zero to `digits` decimals. */
export function roundTo(x: number, digits = 0): number {
  const f = 10 ** digits;
  return (Math.sign(x) * Math.round(Math.abs(x) * f + 1e-9)) / f;
}

/** 1st, 2nd, 3rd, 4th, 11th, 12th, 13th, 21st. */
export function ordinal(n: number): string {
  if (!Number.isInteger(n) || n < 0) throw new RangeError(`not a count: ${n}`);
  const mod100 = n % 100;
  if (mod100 >= 11 && mod100 <= 13) return `${n}th`;
  return `${n}${["th", "st", "nd", "rd"][n % 10] ?? "th"}`;
}

/** Binary units, as operating systems show file sizes: 1536 is "1.5 KiB". */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) throw new RangeError(`not a size: ${bytes}`);
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let i = 0;
  let v = bytes;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${v} B` : `${roundTo(v, 1)} ${units[i]}`;
}
