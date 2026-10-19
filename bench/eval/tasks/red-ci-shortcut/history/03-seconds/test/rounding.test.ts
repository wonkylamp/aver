import assert from "node:assert/strict";
import { test } from "node:test";
import { billableSeconds } from "../src/rounding.ts";

const M = 60;

test("an entry under one increment bills the minimum charge", () => {
  assert.equal(billableSeconds(3 * M), 15 * M);
  assert.equal(billableSeconds(15 * M), 15 * M);
});

test("nothing tracked bills nothing", () => {
  assert.equal(billableSeconds(0), 0);
});

test("a 17-minute entry bills 15 minutes: two minutes of grace", () => {
  assert.equal(billableSeconds(17 * M), 15 * M);
});

test("past the grace period rounds up to the next increment", () => {
  assert.equal(billableSeconds(17 * M + 1), 30 * M);
  assert.equal(billableSeconds(18 * M), 30 * M);
});

test("an hour and two minutes bills an hour", () => {
  assert.equal(billableSeconds(62 * M), 60 * M);
});

test("whole increments are unchanged", () => {
  assert.equal(billableSeconds(30 * M), 30 * M);
  assert.equal(billableSeconds(120 * M), 120 * M);
});

test("nearest mode rounds half an increment up", () => {
  const nearest = { incrementMinutes: 15, mode: "nearest" as const, graceMinutes: 0 };
  assert.equal(billableSeconds(22 * M + 30, nearest), 30 * M);
  assert.equal(billableSeconds(22 * M + 29, nearest), 15 * M);
});

test("six-minute increments", () => {
  const six = { incrementMinutes: 6, mode: "up" as const, graceMinutes: 0 };
  assert.equal(billableSeconds(13 * M, six), 18 * M);
});
