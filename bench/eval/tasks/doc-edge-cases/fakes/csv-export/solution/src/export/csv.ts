// The CSV writer behind every finance export (format: docs/exports.md).

export type Cell = string | number | boolean | Date | null | undefined;

export type ColumnType = "text" | "number" | "amount" | "date" | "timestamp" | "boolean";

export interface Column<T> {
  label: string;
  type: ColumnType;
  value: (row: T) => Cell;
}

/** Characters that make a spreadsheet read a cell as a formula. */
const FORMULA = /^[=+\-@\t\r]/;

/** Units with two decimals, a dot, no separators: 1234.50, -0.05. */
function plainAmount(cents: number): string {
  if (!Number.isInteger(cents)) throw new RangeError(`amounts are whole cents, not ${cents}`);
  const abs = Math.abs(cents);
  return `${cents < 0 ? "-" : ""}${Math.floor(abs / 100)}.${String(abs % 100).padStart(2, "0")}`;
}

function render(value: Cell, type: ColumnType): string {
  if (value === null || value === undefined) return "";
  switch (type) {
    case "amount":
      return typeof value === "number" ? plainAmount(value) : String(value);
    case "date":
      return value instanceof Date ? value.toISOString().slice(0, 10) : String(value);
    case "timestamp":
      return value instanceof Date ? value.toISOString().replace(/\.\d{3}Z$/, "Z") : String(value);
    case "boolean":
      return typeof value === "boolean" ? (value ? "TRUE" : "FALSE") : String(value);
    case "number":
      return String(value);
    case "text": {
      const s = value instanceof Date ? value.toISOString() : String(value);
      return FORMULA.test(s) ? `'${s}` : s;
    }
  }
}

function field(s: string): string {
  return /[",\r\n]/.test(s) || /^[ \t]|[ \t]$/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export function toCsv<T>(columns: Column<T>[], rows: T[]): string {
  const records = [columns.map((c) => field(c.label)), ...rows.map((row) => columns.map((c) => field(render(c.value(row), c.type))))];
  return `\uFEFF${records.map((r) => `${r.join(",")}\r\n`).join("")}`;
}
