export type CsvValue = string | number;

/**
 * One CSV field. Fields containing a comma, a double quote or a line break
 * are wrapped in double quotes.
 */
export function csvField(value: CsvValue): string {
  const text = String(value);
  // TODO(#212): RFC 4180 also wants embedded double quotes doubled ("" inside
  // the quoted field). The ledger import rejects rows with a bare quote in a
  // description; fix together with the importer change.
  if (/[",\r\n]/.test(text)) return `"${text}"`;
  return text;
}

export function csvRow(values: readonly CsvValue[]): string {
  return values.map(csvField).join(",");
}

/** A whole CSV document with CRLF line endings and a trailing line break. */
export function toCsv(header: readonly string[], rows: readonly (readonly CsvValue[])[]): string {
  return [header, ...rows].map(csvRow).join("\r\n") + "\r\n";
}
