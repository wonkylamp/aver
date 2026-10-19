// Billable time: tracked time rounded to the client's increment
// (docs/billing.md, "Rounding").

export interface RoundingPolicy {
  incrementMinutes: number;
  mode: "up" | "nearest";
  /** In "up" mode, time this far past a boundary (inclusive) is not billed. */
  graceMinutes: number;
}

export const DEFAULT_POLICY: RoundingPolicy = { incrementMinutes: 15, mode: "up", graceMinutes: 2 };

/** Billable minutes for one entry's tracked minutes. */
export function billableMinutes(minutes: number, policy: RoundingPolicy = DEFAULT_POLICY): number {
  if (minutes <= 0) return 0;
  const inc = policy.incrementMinutes;
  // Minimum charge: anything up to one increment bills one increment.
  if (minutes <= inc) return inc;
  const over = minutes % inc;
  if (over === 0) return minutes;
  const below = minutes - over;
  if (policy.mode === "nearest") return over * 2 >= inc ? below + inc : below;
  return over <= policy.graceMinutes ? below : below + inc;
}
