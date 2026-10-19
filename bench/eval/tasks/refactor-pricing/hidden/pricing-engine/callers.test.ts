import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { checkoutTotal } from "../../src/checkout/total.ts";
import { buildQuote } from "../../src/quotes/quote.ts";
import { previewInvoice } from "../../src/admin/invoice-preview.ts";
import { toLine } from "../../src/catalog/catalog.ts";
import type { Coupon } from "../../src/types.ts";

interface Case {
  name: string;
  items: Array<[string, number]>;
  member: boolean;
  coupon?: Coupon;
  region: string;
  lineTotals: number[];
  subtotalCents: number;
  totalCents: number;
}

// Expected values are what checkout charged before the refactor.
const CASES: Case[] = [
  { name: "threshold 10", items: [["ROPE-12", 10]], member: false, region: "US-OR", lineTotals: [3316], subtotalCents: 3316, totalCents: 3915 },
  { name: "threshold 50", items: [["SHACKLE-8", 50]], member: true, region: "US-CA", lineTotals: [55463], subtotalCents: 55463, totalCents: 59484 },
  { name: "threshold 100", items: [["BRUSH-50", 100]], member: false, region: "EU-DE", lineTotals: [38250], subtotalCents: 38250, totalCents: 45518 },
  {
    name: "either side of each threshold",
    items: [["ROPE-16", 9], ["ROPE-16", 11], ["ROPE-16", 49], ["ROPE-16", 51], ["ROPE-16", 99], ["ROPE-16", 101]],
    member: false,
    region: "US-WA",
    lineTotals: [4761, 5528, 24625, 24281, 47134, 45415],
    subtotalCents: 151744,
    totalCents: 161607,
  },
  {
    name: "fractional lines add up after rounding",
    items: [["ROPE-12", 1], ["ROPE-16", 1], ["BRUSH-50", 3]],
    member: true,
    region: "US-NY",
    lineTotals: [339, 513, 1310],
    subtotalCents: 2162,
    totalCents: 2871,
  },
  {
    name: "half cents on every line",
    items: [["BRUSH-50", 1], ["BRUSH-50", 1], ["BRUSH-50", 1]],
    member: true,
    region: "US-OR",
    lineTotals: [437, 437, 437],
    subtotalCents: 1311,
    totalCents: 1910,
  },
  {
    name: "threshold with a category coupon",
    items: [["FENDER-M", 10], ["CLEAT-150", 50], ["VARNISH-1L", 10]],
    member: true,
    coupon: { code: "DECK5", percentOff: 5, category: "deck" },
    region: "EU-NL",
    lineTotals: [30015, 77859, 25751],
    subtotalCents: 133625,
    totalCents: 161686,
  },
  {
    name: "cap at the 100 tier",
    items: [["VARNISH-1L", 100]],
    member: true,
    coupon: { code: "SPRING20", percentOff: 20 },
    region: "US-CA",
    lineTotals: [195930],
    subtotalCents: 195930,
    totalCents: 210135,
  },
];

const linesOf = (c: Case) => c.items.map(([sku, qty]) => toLine(sku, qty));
const customer = (c: Case) => ({ id: "c-1", member: c.member });

test("checkout still charges what it charged before", () => {
  for (const c of CASES) {
    const total = checkoutTotal({ lines: linesOf(c), region: c.region }, customer(c), c.coupon);
    assert.deepEqual(
      total.lines.map((l) => l.totalCents),
      c.lineTotals,
      c.name,
    );
    assert.equal(total.subtotalCents, c.subtotalCents, c.name);
    assert.equal(total.totalCents, c.totalCents, c.name);
  }
});

test("quotes price every line like checkout", () => {
  for (const c of CASES) {
    const quote = buildQuote({ customer: customer(c), lines: linesOf(c), coupon: c.coupon, issuedOn: "2026-03-01" });
    assert.deepEqual(
      quote.lines.map((l) => l.totalCents),
      c.lineTotals,
      c.name,
    );
    assert.equal(quote.subtotalCents, c.subtotalCents, c.name);
  }
});

test("invoice previews price every line and total like checkout", () => {
  for (const c of CASES) {
    const preview = previewInvoice({ id: "HB-20260301-000001", region: c.region, customer: customer(c), lines: linesOf(c), coupon: c.coupon });
    assert.deepEqual(
      preview.rows.map((r) => r.totalCents),
      c.lineTotals,
      c.name,
    );
    assert.equal(preview.subtotalCents, c.subtotalCents, c.name);
    assert.equal(preview.totalCents, c.totalCents, c.name);
  }
});

test("quotes and invoice previews show the tier discount at its threshold", () => {
  const lines = [toLine("ROPE-12", 10), toLine("ROPE-16", 50), toLine("BRUSH-50", 100)];
  const quote = buildQuote({ customer: { id: "c-1", member: false }, lines, issuedOn: "2026-03-01" });
  assert.deepEqual(
    quote.lines.map((l) => l.discountPct),
    [5, 10, 15],
  );
  const preview = previewInvoice({ id: "HB-20260301-000002", region: "US-CA", customer: { id: "c-1", member: false }, lines });
  assert.deepEqual(
    preview.rows.map((r) => r.discountPct),
    [5, 10, 15],
  );
});

test("the invoice text shows lines that add up to its subtotal", () => {
  const preview = previewInvoice({
    id: "HB-20260301-000003",
    region: "US-OR",
    customer: { id: "c-1", member: true },
    lines: [toLine("BRUSH-50", 1), toLine("BRUSH-50", 1)],
  });
  const text = preview.text.split("\n");
  assert.ok(text.includes(`${"BRUSH-50".padEnd(12)}${"x1".padStart(6)}${"-3%".padStart(6)}${"$4.37".padStart(14)}`), preview.text);
  assert.ok(text.includes(`${"Subtotal".padEnd(24)}${"$8.74".padStart(14)}`), preview.text);
});

test("checkout, quotes and the invoice preview all import the pricing engine", () => {
  for (const file of ["checkout/total.ts", "quotes/quote.ts", "admin/invoice-preview.ts"]) {
    const source = readFileSync(new URL(`../../src/${file}`, import.meta.url), "utf8");
    assert.match(source, /from\s+["'](?:\.\.\/)+pricing\/engine(?:\.ts)?["']/, `src/${file} imports src/pricing/engine.ts`);
  }
});
