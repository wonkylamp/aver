// The CSV writer behind every finance export (format: docs/exports.md).

export type Cell = string | number | boolean | Date | null | undefined;

export type ColumnType = "text" | "number" | "amount" | "date" | "timestamp" | "boolean";

export interface Column<T> {
  label: string;
  type: ColumnType;
  value: (row: T) => Cell;
}

function amount(cents: number): string {
  const abs = Math.abs(cents);
  return `${cents < 0 ? "-" : ""}${Math.floor(abs / 100)}.${String(abs % 100).padStart(2, "0")}`;
}

function render(value: Cell, type: ColumnType): string {
  if (value === null || value === undefined) return "";
  if (value instanceof Date) return type === "date" ? value.toISOString().slice(0, 10) : value.toISOString();
  if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
  if (type === "amount" && typeof value === "number") return amount(value);
  return String(value);
}

function field(s: string): string {
  const guarded = /^[=+\-@\t\r]/.test(s) ? `'${s}` : s;
  return /[",\r\n]/.test(guarded) ? `"${guarded.replace(/"/g, '""')}"` : guarded;
}

export function toCsv<T>(columns: Column<T>[], rows: T[]): string {
  const lines = [columns.map((c) => field(c.label)).join(",")];
  for (const row of rows) lines.push(columns.map((c) => field(render(c.value(row), c.type))).join(","));
  return `\uFEFF${lines.map((l) => `${l}\r\n`).join("")}`;
}
