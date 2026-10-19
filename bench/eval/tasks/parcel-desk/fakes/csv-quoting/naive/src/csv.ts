// Comma-separated values for the carrier's manifest upload.

export function toCsv(rows: string[][]): string {
  return rows.map((r) => r.map((f) => (f.includes(",") ? `"${f}"` : f)).join(",")).join("\n") + "\n";
}

export function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  for (const line of text.split("\n")) {
    if (line === "") continue;
    const fields: string[] = [];
    for (const m of line.matchAll(/("([^"]*)"|[^,]*)(,|$)/g)) {
      fields.push(m[2] ?? m[1]!);
      if (m[3] === "") break;
    }
    rows.push(fields);
  }
  return rows;
}
