import assert from "node:assert/strict";
import { test } from "node:test";
import { billableMinutes } from "../src/rounding.ts";

const M = 1;

test("an entry under one increment bills the minimum charge", () => {
  assert.equal(billableMinutes(3 * M), 15 * M);
  assert.equal(billableMinutes(15 * M), 15 * M);
});

test("nothing tracked bills nothing", () => {
  assert.equal(billableMinutes(0), 0);
});

test("a 17-minute entry bills 15 minutes: two minutes of grace", () => {
  assert.equal(billableMinutes(17 * M), 15 * M);
});

test("past the grace period rounds up to the next increment", () => {
  assert.equal(billableMinutes(18 * M), 30 * M);
  assert.equal(billableMinutes(18 * M), 30 * M);
});

test("an hour and two minutes bills an hour", () => {
  assert.equal(billableMinutes(62 * M), 60 * M);
});

test("whole increments are unchanged", () => {
  assert.equal(billableMinutes(30 * M), 30 * M);
  assert.equal(billableMinutes(120 * M), 120 * M);
});

test("nearest mode rounds to the nearest increment", () => {
  const nearest = { incrementMinutes: 15, mode: "nearest" as const, graceMinutes: 0 };
  assert.equal(billableMinutes(23 * M, nearest), 30 * M);
  assert.equal(billableMinutes(22 * M, nearest), 15 * M);
});

test("six-minute increments", () => {
  const six = { incrementMinutes: 6, mode: "up" as const, graceMinutes: 0 };
  assert.equal(billableMinutes(13 * M, six), 18 * M);
});
