/**
 * Validates a duration option. Every public function that takes a duration
 * in milliseconds validates it with this, so the error text is uniform.
 */
export function assertDuration(name: string, value: number): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    throw new RangeError(`${name} must be a non-negative finite number of milliseconds, got ${String(value)}`);
  }
  return value;
}
