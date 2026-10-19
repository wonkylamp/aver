// Durations as people type them ("1h30m", "90m", "1.5h") and as we show
// them. Internally a duration is whole minutes.

export class DurationError extends Error {
  override name = "DurationError";
}

const UNIT: Record<string, number> = { h: 60, m: 1 };

export function parseDuration(text: string): number {
  const s = text.trim().toLowerCase().replace(/\s+/g, "");
  if (s === "") throw new DurationError("empty duration");
  if (/^\d+$/.test(s)) return Number(s); // a bare number is minutes, as on paper timesheets
  let total = 0;
  let rest = s;
  let last = Infinity;
  while (rest !== "") {
    const m = /^(\d+(?:\.\d+)?)([hm])/.exec(rest);
    if (!m) throw new DurationError(`not a duration: ${JSON.stringify(text)}`);
    const unit = UNIT[m[2]!]!;
    if (unit >= last) throw new DurationError(`units out of order in ${JSON.stringify(text)}`);
    last = unit;
    total += Number(m[1]) * unit;
    rest = rest.slice(m[0].length);
  }
  return Math.round(total);
}

export function formatDuration(minutes: number): string {
  if (minutes < 0) throw new DurationError(`negative duration ${minutes}`);
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  const parts = [h ? `${h}h` : "", m ? `${m}m` : ""].filter(Boolean);
  return parts.length ? parts.join(" ") : "0m";
}

/** Hours with two decimals, for invoices: 90 minutes is "1.50". */
export function hours(minutes: number): string {
  return (minutes / 60).toFixed(2);
}
