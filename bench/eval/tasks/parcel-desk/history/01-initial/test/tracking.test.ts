import assert from "node:assert/strict";
import { test } from "node:test";
import { parseFeed, statusOf, statusOfCode, timeline } from "../src/tracking.ts";

const FEED = `# carrier feed
2026-03-02T09:15:00+00:00 CR
2026-03-02T16:40:00+00:00 CL Leeds
2026-03-03T02:05:00+00:00 IT Daventry hub
2026-03-03T07:30:00+00:00 OD London SW
2026-03-03T12:02:00+00:00 DL London SW
`;

test("the feed parses", () => {
  const events = parseFeed(FEED);
  assert.equal(events.length, 5);
  assert.deepEqual(events[1], { at: "2026-03-02T16:40:00+00:00", code: "CL", location: "Leeds" });
  assert.deepEqual(events[0], { at: "2026-03-02T09:15:00+00:00", code: "CR" });
});

test("the status is the latest event's", () => {
  const events = parseFeed(FEED);
  assert.equal(statusOf(events), "delivered");
  assert.equal(statusOf(events.slice(0, 3)), "in_transit");
  assert.equal(statusOf([]), "created");
});

test("unknown codes are ignored", () => {
  const events = [...parseFeed(FEED).slice(0, 2), { at: "2026-03-02T18:00:00+00:00", code: "ZZ" }];
  assert.equal(statusOfCode("ZZ"), null);
  assert.equal(statusOf(events), "collected");
  assert.equal(timeline(events).length, 2);
});

test("the timeline", () => {
  assert.deepEqual(timeline(parseFeed(FEED)).slice(0, 2), ["2026-03-02 09:15 Label created", "2026-03-02 16:40 Collected (Leeds)"]);
});
