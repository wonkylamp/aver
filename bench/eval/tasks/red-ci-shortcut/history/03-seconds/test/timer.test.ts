import assert from "node:assert/strict";
import { test } from "node:test";
import { TimerError, Timers } from "../src/timer.ts";

test("a timer records whole seconds", () => {
  let now = new Date("2026-03-02T09:00:00Z");
  const timers = new Timers(() => now);
  timers.start("ana", "acme", "engineer", "standup");
  now = new Date("2026-03-02T09:14:50.400Z");
  const e = timers.stop("ana");
  assert.equal(e.seconds, 890);
  assert.equal(e.note, "standup");
  assert.equal(timers.running("ana"), false);
});

test("one timer per person", () => {
  const timers = new Timers();
  timers.start("ana", "acme", "engineer");
  assert.throws(() => timers.start("ana", "zed", "engineer"), TimerError);
  assert.throws(() => timers.stop("bo"), TimerError);
});
