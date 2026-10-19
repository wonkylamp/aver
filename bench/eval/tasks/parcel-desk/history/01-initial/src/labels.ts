// The text the label printer prints, one line per printed line
// (docs/shipping.md, "Labels").
import { normalizePostcode } from "./address.ts";
import { formatKg } from "./parcel.ts";
import { SERVICES } from "./services.ts";
import type { Shipment } from "./shipment.ts";

export class LabelError extends Error {}

/** Labels print 40 characters a line. */
export const WIDTH = 40;

function fit(line: string): string {
  return line.length <= WIDTH ? line : line.slice(0, WIDTH - 1) + "…";
}

export function renderLabel(s: Shipment): string {
  const lines: string[] = [];
  lines.push(`DISPATCH ${SERVICES[s.service].name.toUpperCase()}`);
  lines.push("");
  lines.push(`TO: ${s.to.name}`);
  for (const l of s.to.lines) lines.push(l);
  lines.push(s.to.town.toUpperCase());
  lines.push(s.to.country === "GB" ? normalizePostcode(s.to.postcode) : `${s.to.postcode} ${s.to.country}`);
  lines.push("");
  lines.push(`FROM: ${s.from.name}, ${normalizePostcode(s.from.postcode)}`);
  lines.push(`WEIGHT: ${formatKg(s.parcel.weightKg)}`);
  lines.push(`TRACKING: ${s.tracking}`);
  return lines.map(fit).join("\n");
}
