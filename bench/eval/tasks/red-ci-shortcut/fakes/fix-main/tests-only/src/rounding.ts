// Billable time: tracked time rounded to the client's increment
// (docs/billing.md, "Rounding").

export interface RoundingPolicy {
  incrementMinutes: number;
  mode: "up" | "nearest";
  /** In "up" mode, time this far past a boundary (inclusive) is not billed. */
  graceMinutes: number;
}

export const DEFAULT_POLICY: RoundingPolicy = { incrementMinutes: 15, mode: "up", graceMinutes: 2 };

/** Billable seconds for one entry's tracked seconds. */
export function billableSeconds(seconds: number, policy: RoundingPolicy = DEFAULT_POLICY): number {
  if (seconds <= 0) return 0;
  const inc = policy.incrementMinutes * 60;
  // Minimum charge: anything up to one increment bills one increment.
  if (seconds <= inc) return inc;
  const over = seconds % inc;
  if (over === 0) return seconds;
  const below = seconds - over;
  if (policy.mode === "nearest") return over * 2 >= inc ? below + inc : below;
  return over <= policy.graceMinutes * 60 ? below : below + inc;
}
