// The carrier's tracking feed: events with a two-letter code
// (docs/shipping.md, "Tracking"). The feed is not in time order.

export type Status = "created" | "collected" | "in_transit" | "out_for_delivery" | "delivered" | "exception" | "returning" | "returned";

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
  RS: "returning",
  RD: "returned",
};

export const STATUS_TEXT: Record<Status, string> = {
  created: "Label created",
  collected: "Collected",
  in_transit: "In transit",
  out_for_delivery: "Out for delivery",
  delivered: "Delivered",
  exception: "Delivery problem",
  returning: "Returning to sender",
  returned: "Returned to sender",
};

/** The status an event's code stands for; unknown codes are not statuses. */
export function statusOfCode(code: string): Status | null {
  return CODES[code] ?? null;
}

/** Events with a known code, oldest first; events at the same instant keep feed order. */
function chronological(events: TrackingEvent[]): Array<{ e: TrackingEvent; s: Status }> {
  return events
    .map((e, i) => ({ e, s: statusOfCode(e.code), t: Date.parse(e.at), i }))
    .filter((x): x is { e: TrackingEvent; s: Status; t: number; i: number } => x.s !== null)
    .sort((a, b) => a.t - b.t || a.i - b.i);
}

/** A parcel's status: that of its latest event with a known code, or created. */
export function statusOf(events: TrackingEvent[]): Status {
  return chronological(events).at(-1)?.s ?? "created";
}

/** One line per event with a known code, oldest first: `2026-03-02 09:15 Collected (Leeds)`. */
export function timeline(events: TrackingEvent[]): string[] {
  return chronological(events).map(({ e, s }) => {
    const when = e.at.slice(0, 16).replace("T", " ");
    return `${when} ${STATUS_TEXT[s]}${e.location ? ` (${e.location})` : ""}`;
  });
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
