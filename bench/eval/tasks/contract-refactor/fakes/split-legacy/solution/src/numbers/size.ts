import { roundTo } from "./round.ts";

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

/** Decimal units, as disk vendors and network speeds count: 1500 is "1.5 kB". */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) throw new RangeError(`not a size: ${bytes}`);
  const units = ["B", "kB", "MB", "GB", "TB"];
  let i = 0;
  let v = bytes;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return i === 0 ? `${v} B` : `${Number(v.toFixed(1))} ${units[i]}`;
}
