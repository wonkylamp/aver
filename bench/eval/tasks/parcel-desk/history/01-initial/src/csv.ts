// Comma-separated values for the carrier's manifest upload.

export function toCsv(rows: string[][]): string {
  return rows.map((r) => r.join(",")).join("\n") + "\n";
}

export function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  for (const line of text.split("\n")) {
    if (line === "") continue;
    rows.push(line.split(","));
  }
  return rows;
}
