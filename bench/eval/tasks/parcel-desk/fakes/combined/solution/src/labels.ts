// The text the label printer prints, one line per printed line
// (docs/shipping.md, "Labels").
import { isDomestic, normalizePostcode } from "./address.ts";
import { formatPence, sum } from "./money.ts";
import { formatKg } from "./parcel.ts";
import { SERVICES } from "./services.ts";
import type { Shipment } from "./shipment.ts";

export class LabelError extends Error {}

/** Labels print 40 characters a line. */
export const WIDTH = 40;

export const HAZMAT_LINE = "UN3481 LITHIUM ION BATTERIES";

function fit(line: string): string {
  return line.length <= WIDTH ? line : line.slice(0, WIDTH - 1) + "…";
}

/** The CN22 block of a parcel going abroad. */
function customs(s: Shipment): string[] {
  const items = s.parcel.contents ?? [];
  if (items.length === 0) throw new LabelError("a parcel going abroad needs its contents declared");
  const lines = ["", "CN22 CUSTOMS DECLARATION"];
  for (const item of items) lines.push(`${item.qty} x ${item.description} ${item.hsCode ?? "-"} ${formatPence(item.qty * item.valuePence)}`);
  lines.push(`TOTAL ${formatPence(sum(items.map((i) => i.qty * i.valuePence)))}`);
  return lines;
}

export function renderLabel(s: Shipment): string {
  if (s.parcel.hazmat && s.service === "express") throw new LabelError("express does not carry hazmat parcels");
  const lines: string[] = [];
  lines.push(`DISPATCH ${SERVICES[s.service].name.toUpperCase()}`);
  if (s.parcel.hazmat) lines.push(HAZMAT_LINE);
  lines.push("");
  lines.push(`TO: ${s.to.name}`);
  for (const l of s.to.lines) lines.push(l);
  lines.push(s.to.town.toUpperCase());
  lines.push(s.to.country === "GB" ? normalizePostcode(s.to.postcode) : `${s.to.postcode} ${s.to.country}`);
  lines.push("");
  lines.push(`FROM: ${s.from.name}, ${normalizePostcode(s.from.postcode)}`);
  lines.push(`WEIGHT: ${formatKg(s.parcel.weightKg)}`);
  lines.push(`TRACKING: ${s.tracking}`);
  if (!isDomestic(s.to)) lines.push(...customs(s));
  return lines.map(fit).join("\n");
}
