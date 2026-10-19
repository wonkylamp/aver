// #45 on its own: returns, and a feed out of time order.
import assert from "node:assert/strict";
import { test } from "node:test";
import { parseFeed, STATUS_TEXT, statusOf, statusOfCode, timeline } from "../../src/tracking.ts";

test("return codes are statuses", () => {
  assert.equal(statusOfCode("RS"), "returning");
  assert.equal(statusOfCode("RD"), "returned");
  assert.equal(STATUS_TEXT[statusOfCode("RS")!], "Returning to sender");
  assert.equal(STATUS_TEXT[statusOfCode("RD")!], "Returned to sender");
});

test("a parcel on its way back", () => {
  const events = parseFeed("2026-03-02T09:15:00+00:00 CR\n2026-03-03T07:30:00+00:00 EX London SW\n2026-03-04T08:00:00+00:00 RS London SW\n");
  assert.equal(statusOf(events), "returning");
  assert.equal(timeline(events).at(-1), "2026-03-04 08:00 Returning to sender (London SW)");
});

test("the latest event by time wins, not the last line", () => {
  const events = parseFeed("2026-03-03T12:02:00+00:00 DL London SW\n2026-03-02T16:40:00+00:00 CL Leeds\n2026-03-03T07:30:00+00:00 OD London SW\n");
  assert.equal(statusOf(events), "delivered");
});

test("times are compared with their offsets", () => {
  const events = parseFeed("2026-06-02T09:30:00+00:00 OD London SW\n2026-06-02T10:00:00+01:00 IT Daventry hub\n");
  assert.equal(statusOf(events), "out_for_delivery");
});

test("at the same instant the later line wins", () => {
  const events = parseFeed("2026-03-05T10:00:00+00:00 RS Leeds\n2026-03-05T10:00:00+00:00 RD Leeds\n");
  assert.equal(statusOf(events), "returned");
});

test("the timeline is oldest first", () => {
  const events = parseFeed("2026-03-03T12:02:00+00:00 DL London SW\n2026-03-02T09:15:00+00:00 CR\n2026-03-02T18:00:00+00:00 ZZ\n2026-03-02T16:40:00+00:00 CL Leeds\n");
  assert.deepEqual(timeline(events), ["2026-03-02 09:15 Label created", "2026-03-02 16:40 Collected (Leeds)", "2026-03-03 12:02 Delivered (London SW)"]);
});
