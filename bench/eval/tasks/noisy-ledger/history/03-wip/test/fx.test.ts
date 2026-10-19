import assert from "node:assert/strict";
import { describe, test } from "node:test";
import type { RateTable } from "../src/fx.ts";
import { convert, FxError, loadRates, rateOn } from "../src/fx.ts";

const table: RateTable = {
  "2026-03-02": { GBP: 0.84, USD: 1.08, JPY: 162.5 },
  "2026-03-06": { GBP: 0.85, USD: 1.1, JPY: 160 },
};

describe("with a small table", () => {
  test("EUR is always 1", () => assert.equal(rateOn(table, "EUR", "2026-01-01"), 1));
  test("weekends use the latest earlier rate", () => assert.equal(rateOn(table, "GBP", "2026-03-08"), 0.85));
  test("no rate within a week", () => assert.throws(() => rateOn(table, "GBP", "2026-03-20"), FxError));
  test("GBP to USD", () => assert.equal(convert(table, 8400, "GBP", "USD", "2026-03-02"), 10800));
  test("to a currency without minor units", () => assert.equal(convert(table, 1000, "EUR", "JPY", "2026-03-06"), 1600));
  test("negative amounts round away from zero", () => assert.equal(convert(table, -1, "GBP", "USD", "2026-03-02"), -1));
});

describe("with the 2025 reference rates", () => {
  test("GBP on the first business day of 2025", () => assert.equal(rateOn(loadRates(), "GBP", "2025-01-02"), 0.8285));
  test("a Saturday uses Friday's rate", () => assert.equal(rateOn(loadRates(), "USD", "2025-03-15"), rateOn(loadRates(), "USD", "2025-03-14")));
  test("a year of USD rates has about 255 business days", () => {
    const days = Object.keys(loadRates()).filter((d) => d.startsWith("2025-"));
    assert.ok(days.length >= 250 && days.length <= 260, String(days.length));
  });
  test("a GBP card payment in Tokyo", () => assert.equal(convert(loadRates(), -350000, "JPY", "GBP", "2025-04-01"), -1860));
});
