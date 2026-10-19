import assert from "node:assert/strict";
import { test } from "node:test";
import { AddressError, formatAddress, isAddress, normalizeAddress, parseAddress, sameAddress } from "../src/email/address.ts";

const valid: Array<[string, { local: string; domain: string }]> = [
  ["bob@example.com", { local: "bob", domain: "example.com" }],
  ["  bob@example.com\t", { local: "bob", domain: "example.com" }],
  ["Bob Stone <bob@example.com>", { local: "bob", domain: "example.com" }],
  ['"Stone, Bob" <bob@mail.example.co.uk>', { local: "bob", domain: "mail.example.co.uk" }],
  ["first.last+tag@example.org", { local: "first.last+tag", domain: "example.org" }],
  ["o'brien@example.ie", { local: "o'brien", domain: "example.ie" }],
  ["x@a-b.example", { local: "x", domain: "a-b.example" }],
];

for (const [raw, want] of valid) {
  test(`parseAddress reads ${JSON.stringify(raw)}`, () => {
    assert.deepEqual(parseAddress(raw), want);
  });
}

const invalid = ["", "bob", "@example.com", "bob@", "bob@example", "bob@@example.com", ".bob@example.com", "bob.@example.com", "bo..b@example.com", "bob@-example.com", "bob@example.123", "bob smith@example.com"];

for (const raw of invalid) {
  test(`parseAddress rejects ${JSON.stringify(raw)}`, () => {
    assert.throws(() => parseAddress(raw), AddressError);
    assert.equal(isAddress(raw), false);
  });
}

test("normalizeAddress drops the display name and lowercases the domain", () => {
  assert.equal(normalizeAddress(" Bob <bob@Example.COM> "), "bob@example.com");
  assert.equal(normalizeAddress("help@SUPPORT.relaydesk.test"), "help@support.relaydesk.test");
});

test("sameAddress compares canonical forms and is false for garbage", () => {
  assert.equal(sameAddress("bob@EXAMPLE.com", "Bob <bob@example.com>"), true);
  assert.equal(sameAddress("bob@example.com", "rob@example.com"), false);
  assert.equal(sameAddress("not an address", "bob@example.com"), false);
});

test("formatAddress quotes display names for headers", () => {
  assert.equal(formatAddress("help@support.relaydesk.test", 'Ana "Support" Ruiz'), '"Ana \\"Support\\" Ruiz" <help@support.relaydesk.test>');
  assert.equal(formatAddress(" bob@example.com "), "bob@example.com");
});
