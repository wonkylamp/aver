import { clamp } from "./clamp.ts";

/** `part` as a whole percentage of `whole`, bounded to 0-100: 1 of 3 is 33. */
export function percent(part: number, whole: number): number {
  if (whole === 0) return part === 0 ? 0 : 100;
  return clamp(Math.round((part / whole) * 100), 0, 100);
}
