// A small RFC 4180 reader for bank statements (quoted fields, doubled
// quotes, CRLF or LF, an optional byte-order mark, any delimiter).

export class CsvError extends Error {
  override name = "CsvError";
}

export function parseCsv(text: string, delimiter = ","): string[][] {
  const src = text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  let wasQuoted = false;
  for (let i = 0; i < src.length; i++) {
    const ch = src[i]!;
    if (quoted) {
      if (ch === '"') {
        if (src[i + 1] === '"') {
          cell += '"';
          i++;
        } else quoted = false;
      } else cell += ch;
      continue;
    }
    if (ch === '"') {
      if (cell !== "" || wasQuoted) throw new CsvError(`stray quote in row ${rows.length + 1}`);
      quoted = true;
      wasQuoted = true;
    } else if (ch === delimiter) {
      row.push(cell);
      cell = "";
      wasQuoted = false;
    } else if (ch === "\n" || ch === "\r") {
      if (ch === "\r" && src[i + 1] === "\n") i++;
      row.push(cell);
      if (!(row.length === 1 && row[0] === "")) rows.push(row);
      row = [];
      cell = "";
      wasQuoted = false;
    } else cell += ch;
  }
  if (quoted) throw new CsvError("unterminated quoted field");
  if (cell !== "" || row.length > 0) rows.push([...row, cell]);
  return rows;
}

/** Rows as objects keyed by the header row's names. */
export function parseCsvObjects(text: string, delimiter = ","): Record<string, string>[] {
  const [header, ...rows] = parseCsv(text, delimiter);
  if (!header) return [];
  const names = header.map((h) => h.trim());
  return rows.map((r, i) => {
    if (r.length !== names.length) throw new CsvError(`row ${i + 2} has ${r.length} fields, the header has ${names.length}`);
    return Object.fromEntries(names.map((n, k) => [n, r[k]!]));
  });
}
