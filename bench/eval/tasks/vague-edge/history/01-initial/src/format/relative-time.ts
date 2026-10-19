const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * Short relative label for a feed timestamp. Anything older than a week is
 * shown as a UTC date so the label never changes once it is old enough.
 */
export function relativeTime(then: number, now: number): string {
  const diff = now - then;
  if (diff < 0) return 'just now';
  if (diff < MINUTE) return 'just now';
  if (diff < HOUR) return plural(Math.floor(diff / MINUTE), 'minute');
  if (diff < DAY) return plural(Math.floor(diff / HOUR), 'hour');
  if (diff < 2 * DAY) return 'yesterday';
  if (diff < 7 * DAY) return plural(Math.floor(diff / DAY), 'day');
  return new Date(then).toISOString().slice(0, 10);
}

function plural(n: number, unit: string): string {
  return `${n} ${unit}${n === 1 ? '' : 's'} ago`;
}
