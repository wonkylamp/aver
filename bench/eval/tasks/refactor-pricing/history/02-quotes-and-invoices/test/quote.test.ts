import { test } from "node:test";
import assert from "node:assert/strict";
import { buildQuote } from "../src/quotes/quote.ts";
import { toLine } from "../src/catalog/catalog.ts";

const trade = { id: "c-trade", member: true };
const guest = { id: "c-guest", member: false };

test("a quote prices each line and holds for 30 days", () => {
  const quote = buildQuote({
    customer: trade,
    lines: [toLine("SHACKLE-8", 24), toLine("CLEAT-150", 6)],
    issuedOn: "2026-03-01",
  });
  assert.deepEqual(quote, {
    customerId: "c-trade",
    lines: [
      { sku: "SHACKLE-8", qty: 24, unitPriceCents: 1275, discountPct: 8, totalCents: 28152 },
      { sku: "CLEAT-150", qty: 6, unitPriceCents: 1899, discountPct: 3, totalCents: 11052 },
    ],
    subtotalCents: 39204,
    issuedOn: "2026-03-01",
    expiresOn: "2026-03-31",
  });
});

test("a category coupon stacks with the bulk tier", () => {
  const quote = buildQuote({
    customer: guest,
    lines: [toLine("ROPE-12", 60), toLine("FENDER-M", 3)],
    coupon: { code: "RIG15", percentOff: 15, category: "rigging" },
    issuedOn: "2026-03-01",
  });
  assert.deepEqual(
    quote.lines.map((l) => [l.discountPct, l.totalCents]),
    [
      [25, 15705],
      [0, 10350],
    ],
  );
  assert.equal(quote.subtotalCents, 26055);
});

test("validDays moves the expiry across a month end", () => {
  const quote = buildQuote({ customer: guest, lines: [toLine("BRUSH-50", 2)], issuedOn: "2026-02-20", validDays: 14 });
  assert.equal(quote.expiresOn, "2026-03-06");
});

test("a quote needs lines and a sensible validity", () => {
  assert.throws(() => buildQuote({ customer: guest, lines: [], issuedOn: "2026-03-01" }), /at least one line/);
  assert.throws(
    () => buildQuote({ customer: guest, lines: [toLine("BRUSH-50", 1)], issuedOn: "2026-03-01", validDays: 0 }),
    /validDays/,
  );
});
