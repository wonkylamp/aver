import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { addDays } from "../src/dates.ts";
import { findRecurring } from "../src/recurring.ts";
import { tx } from "../src/testing/tx.ts";

const monthly = (payee: string, day: number, amount: number, months: number, from = 1) =>
  Array.from({ length: months }, (_, i) => tx(`2026-${String(from + i).padStart(2, "0")}-${String(day).padStart(2, "0")}`, amount, payee));

describe("findRecurring", () => {
  test("a monthly subscription", () => {
    const [r] = findRecurring(monthly("Netflix.com", 5, -1099, 4));
    assert.deepEqual(r, { payee: "Netflix.com", amount: -1099, dayOfMonth: 5, months: 4, next: "2026-05-05" });
  });
  test("two months are not enough", () => assert.deepEqual(findRecurring(monthly("Netflix.com", 5, -1099, 2)), []));
  test("a price rise within 10% still counts", () => {
    const txs = [...monthly("Spotify", 12, -1099, 3), tx("2026-04-12", -1199, "Spotify")];
    assert.equal(findRecurring(txs).length, 1);
  });
  test("a big change in amount does not", () => {
    const txs = [...monthly("Octopus Energy", 20, -8000, 3), tx("2026-04-20", -12000, "Octopus Energy")];
    assert.deepEqual(findRecurring(txs), []);
  });
  test("weekly payments are not monthly", () => {
    const txs = Array.from({ length: 6 }, (_, i) => tx(addDays("2026-03-02", 7 * i), -500, "Gym"));
    assert.deepEqual(findRecurring(txs), []);
  });
  test("income is ignored", () => assert.deepEqual(findRecurring(monthly("Payroll", 28, 250000, 6)), []));
  test("the payee is matched case-insensitively", () => {
    const txs = [tx("2026-01-03", -899, "DISNEY PLUS"), tx("2026-02-03", -899, "Disney Plus"), tx("2026-03-03", -899, "disney plus")];
    assert.equal(findRecurring(txs)[0]!.months, 3);
  });
  test("December rolls the next charge into January", () => {
    const txs = [tx("2025-10-15", -500, "iCloud"), tx("2025-11-15", -500, "iCloud"), tx("2025-12-15", -500, "iCloud")];
    assert.equal(findRecurring(txs)[0]!.next, "2026-01-15");
  });
  test("a monthly bill is detected despite billing-day drift", () => {
    // Banks post a few days early or late around weekends.
    const jitter = Math.floor(Math.random() * 7) - 3;
    const txs = [...monthly("Vodafone", 10, -2500, 3), tx(`2026-04-${String(10 + jitter).padStart(2, "0")}`, -2500, "Vodafone"), ...monthly("Vodafone", 10, -2500, 2, 5)];
    assert.equal(findRecurring(txs).length, 1, `drift of ${jitter} days`);
  });
  test("several at once, sorted by payee", () => {
    const txs = [...monthly("Spotify", 12, -1099, 3), ...monthly("Netflix.com", 5, -1099, 3), tx("2026-02-14", -4500, "Florist")];
    assert.deepEqual(findRecurring(txs).map((r) => r.payee), ["Netflix.com", "Spotify"]);
  });
});
