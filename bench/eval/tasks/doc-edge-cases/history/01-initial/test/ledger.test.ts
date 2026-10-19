import assert from "node:assert/strict";
import { test } from "node:test";
import { Chart, ChartError, debitNormal, STANDARD_CHART } from "../src/ledger/accounts.ts";
import { Journal, JournalError } from "../src/ledger/journal.ts";
import { renderTrialBalance, trialBalance } from "../src/reports/trial-balance.ts";

const d = (s: string) => new Date(`${s}T00:00:00Z`);

function books() {
  const chart = new Chart(STANDARD_CHART);
  const journal = new Journal(chart, () => new Date("2026-03-31T18:00:00Z"));
  journal.post({ date: d("2026-03-01"), payee: "Owner", postings: [{ account: "1000", cents: 500000 }, { account: "3000", cents: -500000 }] });
  journal.post({ date: d("2026-03-03"), payee: "Landlord", postings: [{ account: "6100", cents: 120000 }, { account: "1000", cents: -120000 }] });
  journal.post({ date: d("2026-03-10"), payee: "Acme Ltd", postings: [{ account: "1100", cents: 238000 }, { account: "4000", cents: -200000 }, { account: "2100", cents: -38000 }] });
  return { chart, journal };
}

test("the chart rejects bad and duplicate codes", () => {
  const chart = new Chart(STANDARD_CHART);
  assert.throws(() => chart.add({ code: "12", name: "x", type: "asset" }), ChartError);
  assert.throws(() => chart.add({ code: "1000", name: "x", type: "asset" }), ChartError);
  assert.equal(chart.get("6200").name, "Software");
  assert.equal(debitNormal("expense"), true);
  assert.equal(debitNormal("income"), false);
});

test("entries must balance and use known accounts", () => {
  const { journal } = books();
  assert.throws(() => journal.post({ date: d("2026-03-11"), payee: "x", postings: [{ account: "1000", cents: 100 }, { account: "4000", cents: -99 }] }), JournalError);
  assert.throws(() => journal.post({ date: d("2026-03-11"), payee: "x", postings: [{ account: "1000", cents: 100 }, { account: "9999", cents: -100 }] }), JournalError);
  assert.throws(() => journal.post({ date: d("2026-03-11"), payee: "x", postings: [{ account: "1000", cents: 100 }] }), JournalError);
});

test("entries are listed by date and filtered by account", () => {
  const { journal } = books();
  assert.deepEqual(journal.entries().map((e) => e.payee), ["Owner", "Landlord", "Acme Ltd"]);
  assert.deepEqual(journal.entries({ account: "6100" }).map((e) => e.payee), ["Landlord"]);
  assert.deepEqual(journal.entries({ from: d("2026-03-02"), to: d("2026-03-05") }).map((e) => e.payee), ["Landlord"]);
});

test("the trial balance balances", () => {
  const { chart, journal } = books();
  const tb = trialBalance(chart, journal, d("2026-03-31"));
  assert.equal(tb.balanced, true);
  assert.equal(tb.totalDebit, 738000);
  assert.deepEqual(tb.rows.map((r) => r.code), ["1000", "1100", "2100", "3000", "4000", "6100"]);
});

test("the rendered trial balance is for people", () => {
  const { chart, journal } = books();
  const text = renderTrialBalance(trialBalance(chart, journal, d("2026-03-31")));
  const lines = text.split("\n");
  assert.equal(lines[1], "1000  Cash                          3,800.00              ");
  assert.equal(lines.at(-1), "      Total                         7,380.00      7,380.00");
});
