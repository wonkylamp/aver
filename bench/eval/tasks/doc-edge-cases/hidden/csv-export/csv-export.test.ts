// The export format of docs/exports.md, rule by rule.
import assert from "node:assert/strict";
import { test } from "node:test";
import { toCsv } from "../../src/export/csv.ts";
import type { Column } from "../../src/export/csv.ts";
import { exportLedger } from "../../src/export/ledger-export.ts";
import { exportVendors } from "../../src/export/vendor-export.ts";
import { Chart, STANDARD_CHART } from "../../src/ledger/accounts.ts";
import { Journal } from "../../src/ledger/journal.ts";
import type { Vendor } from "../../src/vendors.ts";

// Each rule is tested on its own: the readers below accept a missing
// byte-order mark and LF record ends, which only the framing tests check.

/** The raw text of each record (without its line end), splitting only outside quotes. */
function rawRecords(csv: string): string[] {
  const text = csv.startsWith("\uFEFF") ? csv.slice(1) : csv;
  const out: string[] = [];
  let start = 0;
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]!;
    if (ch === '"') quoted = !quoted;
    else if (!quoted && (ch === "\n" || (ch === "\r" && text[i + 1] === "\n"))) {
      out.push(text.slice(start, i));
      if (ch === "\r") i++;
      start = i + 1;
    }
  }
  if (start < text.length) out.push(text.slice(start));
  return out;
}

/** An RFC 4180 reader: each record's unquoted cells. */
function parse(csv: string): string[][] {
  return rawRecords(csv).map((rec) => {
    const cells: string[] = [];
    let cell = "";
    let quoted = false;
    for (let i = 0; i < rec.length; i++) {
      const ch = rec[i]!;
      if (quoted) {
        if (ch === '"' && rec[i + 1] === '"') {
          cell += '"';
          i++;
        } else if (ch === '"') quoted = false;
        else cell += ch;
      } else if (ch === '"' && cell === "") quoted = true;
      else if (ch === ",") {
        cells.push(cell);
        cell = "";
      } else cell += ch;
    }
    cells.push(cell);
    return cells;
  });
}

const text = (label: string): Column<{ v: unknown }> => ({ label, type: "text", value: (r) => r.v as string });
const one = (col: Column<{ v: unknown }>, v: unknown): string => toCsv([col], [{ v }]);
/** The written form of one text value. */
const written = (v: unknown): string => rawRecords(one(text("V"), v))[1]!;

test("the file starts with a UTF-8 byte-order mark", () => {
  assert.ok(one(text("Payee"), "Café Olé").startsWith("\uFEFFPayee"));
});

test("every record ends with CRLF, the last one too", () => {
  const csv = toCsv([text("A"), text("B")], [{ v: "x" }, { v: "y" }]);
  assert.equal(csv, "\uFEFFA,B\r\nx,x\r\ny,y\r\n");
});

test("commas, quotes and line breaks are quoted, with quotes doubled", () => {
  assert.equal(written('Rent, March "final"'), '"Rent, March ""final"""');
  assert.equal(written("line one\nline two"), '"line one\nline two"');
  assert.equal(parse(one(text("M"), "a\r\nb"))[1]![0], "a\r\nb");
  assert.equal(written("Plain text"), "Plain text");
});

test("leading and trailing spaces or tabs are quoted", () => {
  assert.equal(written(" Acme"), '" Acme"');
  assert.equal(written("Acme\t"), '"Acme\t"');
  assert.equal(written("Acme "), '"Acme "');
  assert.equal(written("Ac me"), "Ac me");
});

test("text that a spreadsheet would run as a formula gets an apostrophe", () => {
  for (const v of ["=SUM(A1:A9)", "+44 20 7946 0958", "-5 refund", "@admin"]) {
    assert.equal(written(v), `'${v}`, v);
  }
  assert.equal(written("Smith & Co = best"), "Smith & Co = best");
});

test("the guard comes first and quoting is decided on the guarded value", () => {
  assert.equal(written('=HYPERLINK("http://x.example")'), '"\'=HYPERLINK(""http://x.example"")"');
  assert.equal(written("\r\nhidden"), "\"'\r\nhidden\"");
});

test("number and amount columns are never guarded", () => {
  const cols: Column<{ n: number; c: number }>[] = [
    { label: "N", type: "number", value: (r) => r.n },
    { label: "C", type: "amount", value: (r) => r.c },
  ];
  assert.deepEqual(parse(toCsv(cols, [{ n: -3, c: -1250 }]))[1], ["-3", "-12.50"]);
});

test("amounts have two decimals and no thousands separators", () => {
  const col: Column<{ c: number }> = { label: "C", type: "amount", value: (r) => r.c };
  assert.deepEqual(parse(toCsv([col], [{ c: 123450 }, { c: -5 }, { c: 100000000 }, { c: 0 }])).slice(1), [["1234.50"], ["-0.05"], ["1000000.00"], ["0.00"]]);
});

test("null and undefined are empty fields in every column type", () => {
  const types = ["text", "number", "amount", "date", "timestamp", "boolean"] as const;
  const cols: Column<object>[] = types.map((type, i) => ({ label: type, type, value: () => (i % 2 ? null : undefined) }));
  assert.deepEqual(rawRecords(toCsv(cols, [{}])), [types.join(","), ",,,,,"]);
});

test("booleans are TRUE and FALSE", () => {
  const col: Column<{ b: boolean }> = { label: "B", type: "boolean", value: (r) => r.b };
  assert.deepEqual(parse(toCsv([col], [{ b: true }, { b: false }])).slice(1), [["TRUE"], ["FALSE"]]);
});

test("timestamps are UTC to the second, without milliseconds", () => {
  const col: Column<{ t: Date }> = { label: "T", type: "timestamp", value: (r) => r.t };
  assert.deepEqual(parse(toCsv([col], [{ t: new Date("2026-03-31T18:00:00.250Z") }, { t: new Date("2026-01-02T03:04:05Z") }])).slice(1), [["2026-03-31T18:00:00Z"], ["2026-01-02T03:04:05Z"]]);
});

test("the ledger export follows the format end to end", () => {
  const chart = new Chart(STANDARD_CHART);
  const journal = new Journal(chart, () => new Date("2026-03-31T18:00:00.125Z"));
  journal.post({
    date: new Date("2026-03-10T00:00:00Z"),
    payee: "=cmd|' /C calc'!A0",
    postings: [
      { account: "1100", cents: 238000, memo: "Invoice 1001, March\nsecond line" },
      { account: "4000", cents: -238000 },
    ],
  });
  const rows = parse(exportLedger(chart, journal));
  assert.equal(rows.length, 3);
  assert.deepEqual(rows[1], ["2026-03-10", "je_00001", "", "'=cmd|' /C calc'!A0", "1100", "Accounts receivable", "Invoice 1001, March\nsecond line", "2380.00", "", "2380.00", "FALSE", "2026-03-31T18:00:00Z"]);
  assert.deepEqual(rows[2]!.slice(7), ["", "2380.00", "-2380.00", "FALSE", "2026-03-31T18:00:00Z"]);
});

test("the vendor export guards phone numbers and leaves numbers alone", () => {
  const v: Vendor = { id: "v1", name: "Acme, Inc.", phone: "+1 555 0100", paymentTermsDays: 30, active: true, balanceCents: -99950 };
  const rows = parse(exportVendors([v]));
  assert.deepEqual(rows[1], ["Acme, Inc.", "", "'+1 555 0100", "", "30", "TRUE", "-999.50", ""]);
});
