// The CSV writer behind every finance export.
import { formatAmount } from "../money.ts";

export type Cell = string | number | boolean | Date | null | undefined;

export type ColumnType = "text" | "number" | "amount" | "date" | "timestamp" | "boolean";

export interface Column<T> {
  label: string;
  type: ColumnType;
  value: (row: T) => Cell;
}

function render(value: Cell, type: ColumnType): string {
  if (value instanceof Date) return type === "date" ? value.toISOString().slice(0, 10) : value.toISOString();
  if (type === "amount" && typeof value === "number") return formatAmount(value);
  return String(value);
}

export function toCsv<T>(columns: Column<T>[], rows: T[]): string {
  const lines = [columns.map((c) => c.label).join(",")];
  for (const row of rows) lines.push(columns.map((c) => render(c.value(row), c.type)).join(","));
  return lines.join("\n") + "\n";
}
