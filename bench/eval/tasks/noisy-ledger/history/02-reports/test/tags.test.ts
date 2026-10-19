import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { addTags, normalizeTag, TagError, tagsInText } from "../src/tags.ts";

describe("normalizeTag", () => {
  const cases: Array<[string, string]> = [
    ["work", "work"],
    ["Work", "work"],
    ["#work", "work"],
    ["Client A", "client-a"],
    ["client_a", "client-a"],
    ["  holiday 2026 ", "holiday-2026"],
    ["a--b", "a-b"],
    ["-edge-", "edge"],
    ["x", "x"],
    ["Q3 Offsite", "q3-offsite"],
  ];
  for (const [raw, want] of cases) test(JSON.stringify(raw), () => assert.equal(normalizeTag(raw), want));
  for (const bad of ["", "#", "café", "a/b", "x".repeat(33)]) {
    test(`rejects ${JSON.stringify(bad)}`, () => assert.throws(() => normalizeTag(bad), TagError));
  }
});

describe("tags in text", () => {
  const cases: Array<[string, string[]]> = [
    ["#work lunch", ["work"]],
    ["lunch #Client_A #work", ["client-a", "work"]],
    ["no tags here", []],
    ["email me at a#b", []],
    ["#work #WORK", ["work"]],
  ];
  for (const [text, want] of cases) test(JSON.stringify(text), () => assert.deepEqual(tagsInText(text), want));
  test("addTags merges and sorts", () => assert.deepEqual(addTags(["work"], ["Travel", "work"]), ["travel", "work"]));
});
