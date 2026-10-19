import assert from "node:assert/strict";
import { test } from "node:test";
import { toCsv } from "../src/export/csv.ts";
import type { Column } from "../src/export/csv.ts";
import { exportLedger, LEDGER_COLUMNS, ledgerLines } from "../src/export/ledger-export.ts";
import { Chart, STANDARD_CHART } from "../src/ledger/accounts.ts";
import { Journal } from "../src/ledger/journal.ts";

const d = (s: string) => new Date(`${s}T00:00:00Z`);

/** The records of a CSV text, for tests that only care about the cells. */
function records(csv: string): string[] {
  return csv.trim().split(/\r?\n/);
}

type Row = { sku: string; qty: number };
const columns: Column<Row>[] = [
  { label: "SKU", type: "text", value: (r) => r.sku },
  { label: "Qty", type: "number", value: (r) => r.qty },
];

test("a header row, then one row per item", () => {
  assert.deepEqual(records(toCsv(columns, [{ sku: "A-1", qty: 2 }, { sku: "B-7", qty: 10 }])), ["SKU,Qty", "A-1,2", "B-7,10"]);
});

test("no rows is a header only", () => {
  assert.deepEqual(records(toCsv(columns, [])), ["SKU,Qty"]);
});

test("dates are calendar dates in UTC", () => {
  const cols: Column<{ at: Date }>[] = [{ label: "Day", type: "date", value: (r) => r.at }];
  assert.deepEqual(records(toCsv(cols, [{ at: new Date("2026-03-01T23:30:00Z") }])), ["Day", "2026-03-01"]);
});

function books() {
  const chart = new Chart(STANDARD_CHART);
  const journal = new Journal(chart, () => new Date("2026-03-31T18:00:00Z"));
  journal.post({ date: d("2026-03-03"), payee: "Landlord", reference: "INV-77", postings: [{ account: "6100", cents: 120000, memo: "March rent" }, { account: "1000", cents: -120000 }] });
  journal.post({ date: d("2026-03-05"), payee: "Figma", postings: [{ account: "6200", cents: 1500 }, { account: "1000", cents: -1500 }] });
  return { chart, journal };
}

test("the ledger export has one line per posting", () => {
  const { chart, journal } = books();
  const lines = ledgerLines(chart, journal);
  assert.equal(lines.length, 4);
  assert.deepEqual(lines.map((l) => l.accountName), ["Rent", "Cash", "Software", "Cash"]);
});

test("the ledger export's header", () => {
  const { chart, journal } = books();
  assert.equal(records(exportLedger(chart, journal))[0], LEDGER_COLUMNS.map((c) => c.label).join(","));
  assert.equal(LEDGER_COLUMNS.length, 12);
});

test("small amounts and the payee", () => {
  const { chart, journal } = books();
  const row = records(exportLedger(chart, journal, { from: d("2026-03-05") }))[1]!.split(",");
  assert.equal(row[0], "2026-03-05");
  assert.equal(row[3], "Figma");
  assert.equal(row[5], "Software");
  assert.equal(row[7], "15.00");
});
