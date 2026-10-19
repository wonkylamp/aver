// The carrier's tracking feed: events with a two-letter code
// (docs/shipping.md, "Tracking").

export type Status = "created" | "collected" | "in_transit" | "out_for_delivery" | "delivered" | "exception";

export type TrackingEvent = {
  /** ISO 8601 with an offset, e.g. 2026-03-02T09:15:00+00:00. */
  at: string;
  code: string;
  location?: string;
};

const CODES: Record<string, Status> = {
  CR: "created",
  CL: "collected",
  IT: "in_transit",
  OD: "out_for_delivery",
  DL: "delivered",
  EX: "exception",
};

export const STATUS_TEXT: Record<Status, string> = {
  created: "Label created",
  collected: "Collected",
  in_transit: "In transit",
  out_for_delivery: "Out for delivery",
  delivered: "Delivered",
  exception: "Delivery problem",
};

/** The status an event's code stands for; unknown codes are not statuses. */
export function statusOfCode(code: string): Status | null {
  return CODES[code] ?? null;
}

/** A parcel's status: that of its latest event with a known code, or created. */
export function statusOf(events: TrackingEvent[]): Status {
  let status: Status = "created";
  for (const e of events) {
    const s = statusOfCode(e.code);
    if (s) status = s;
  }
  return status;
}

/** One line per event with a known code: `2026-03-02 09:15 Collected (Leeds)`. */
export function timeline(events: TrackingEvent[]): string[] {
  const out: string[] = [];
  for (const e of events) {
    const s = statusOfCode(e.code);
    if (!s) continue;
    const when = e.at.slice(0, 16).replace("T", " ");
    out.push(`${when} ${STATUS_TEXT[s]}${e.location ? ` (${e.location})` : ""}`);
  }
  return out;
}

/** Reads the feed's lines: `<at> <code> [location]`. */
export function parseFeed(text: string): TrackingEvent[] {
  const out: TrackingEvent[] = [];
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    const [at, code, ...rest] = line.split(/\s+/);
    out.push(rest.length ? { at: at!, code: code!, location: rest.join(" ") } : { at: at!, code: code! });
  }
  return out;
}
