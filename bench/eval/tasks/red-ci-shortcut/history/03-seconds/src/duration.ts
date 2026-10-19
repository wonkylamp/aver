// Durations as people type them ("1h30m", "90m", "1.5h", "45s") and as we
// show them. Internally a duration is whole seconds.

export class DurationError extends Error {
  override name = "DurationError";
}

const UNIT: Record<string, number> = { h: 3600, m: 60, s: 1 };

export function parseDuration(text: string): number {
  const s = text.trim().toLowerCase().replace(/\s+/g, "");
  if (s === "") throw new DurationError("empty duration");
  if (/^\d+$/.test(s)) return Number(s) * 60; // a bare number is minutes, as on paper timesheets
  let total = 0;
  let rest = s;
  let last = Infinity;
  while (rest !== "") {
    const m = /^(\d+(?:\.\d+)?)([hms])/.exec(rest);
    if (!m) throw new DurationError(`not a duration: ${JSON.stringify(text)}`);
    const unit = UNIT[m[2]!]!;
    if (unit >= last) throw new DurationError(`units out of order in ${JSON.stringify(text)}`);
    last = unit;
    total += Number(m[1]) * unit;
    rest = rest.slice(m[0].length);
  }
  return Math.round(total);
}

export function formatDuration(seconds: number): string {
  if (seconds < 0) throw new DurationError(`negative duration ${seconds}`);
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  const parts = [h ? `${h}h` : "", m ? `${m}m` : "", s ? `${s}s` : ""].filter(Boolean);
  return parts.length ? parts.join(" ") : "0m";
}

/** Hours with two decimals, for invoices and exports: 5400 s is "1.50". */
export function hours(seconds: number): string {
  return (seconds / 3600).toFixed(2);
}
