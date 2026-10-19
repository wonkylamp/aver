# Finance exports

Finance teams download their books as CSV files and open them in Excel,
Google Sheets or Numbers, or load them into their accountant's software.
Every export goes through one writer, `toCsv` in `src/export/csv.ts`, so
the format below holds for all of them.

## The exports

| Download | Function | Rows |
|---|---|---|
| Ledger lines | `exportLedger` | one per posting, by date |
| Vendors | `exportVendors` | one per vendor, by name |

Columns are typed (`text`, `number`, `amount`, `date`, `timestamp`,
`boolean`); the type decides how a value is written, never the value
itself.

## File format

The format is RFC 4180 with the choices below, made so that Excel on
Windows and macOS opens the file correctly with a double click.

**Encoding.** UTF-8, starting with a byte-order mark (U+FEFF). Without it,
Excel reads the file as Windows-1252 and mangles every accented payee.

**Records.** Every record, the header included, ends with CRLF (`\r\n`),
the last one too. Fields are separated by commas.

**Quoting.** A field is enclosed in double quotes when it contains a comma,
a double quote, a CR or an LF, or when it begins or ends with a space or a
tab (spreadsheets trim unquoted fields). Inside a quoted field, a double
quote is written twice. Other fields are not quoted.

**Formula guard.** Spreadsheets run a cell that starts with `=`, `+`, `-`,
`@`, a tab or a CR as a formula, and a payee name is attacker-controlled
text (CSV injection). So in `text` columns, a value starting with one of
those characters is written with an apostrophe in front (`'=SUM(A1)`); the
guard is applied first, and quoting is then decided on the guarded value.
`number` and `amount` columns are never guarded: a refund of -12.50 stays
`-12.50`.

**Empty values.** `null` and `undefined` are empty fields, in every column
type.

**Values by column type.**

- `text`: as is.
- `number`: JavaScript's decimal form (`12`, `0.5`, `-3`).
- `amount`: integer cents written as units with exactly two decimals, a dot,
  a leading minus for negatives and no thousands separators: `1234.50`,
  `-0.05`. (`formatAmount` is for people and adds separators; exports must
  not use it.)
- `date`: the calendar date in UTC, `YYYY-MM-DD`.
- `timestamp`: UTC, to the second, with a `Z`: `2026-03-31T18:00:00Z`
  (no milliseconds; the accounting packages we load into reject them).
- `boolean`: `TRUE` or `FALSE`, which every spreadsheet reads as a boolean.

## Schedules

Exports can be scheduled monthly; the scheduler (not in this repository)
calls the same functions with the previous month's range and mails a link.
File names are `<company>-<export>-<YYYY-MM>.csv`.
