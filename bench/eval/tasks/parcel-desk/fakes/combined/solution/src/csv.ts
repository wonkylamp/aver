// Comma-separated values for the carrier's manifest upload (RFC 4180
// quoting, rows ending with \n).

function field(value: string): string {
  return /[",\r\n]/.test(value) ? `"${value.replaceAll('"', '""')}"` : value;
}

export function toCsv(rows: string[][]): string {
  return rows.map((r) => r.map(field).join(",")).join("\n") + "\n";
}

export function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let cur = "";
  let quoted = false;
  let started = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        cur += '"';
        i += 1;
      } else if (c === '"') {
        quoted = false;
      } else {
        cur += c;
      }
      continue;
    }
    if (c === '"') {
      quoted = true;
      started = true;
    } else if (c === ",") {
      row.push(cur);
      cur = "";
      started = true;
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i += 1;
      if (started || cur !== "") {
        row.push(cur);
        rows.push(row);
      }
      row = [];
      cur = "";
      started = false;
    } else {
      cur += c;
      started = true;
    }
  }
  if (started || cur !== "") {
    row.push(cur);
    rows.push(row);
  }
  return rows;
}
