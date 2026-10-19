# Billing rules

What we bill a client for tracked time. Finance signs off on changes to
this page; the code follows it.

## Rounding

Each entry is rounded on its own, before anything is added up, to the
client's increment (default 15 minutes):

- **Minimum charge.** An entry of one increment or less bills one
  increment. A 3-minute call bills 15 minutes.
- **Mode "up"** (the default): anything past a boundary bills the next
  increment, except for a grace period: time up to and including
  `graceMinutes` (default 2) past a boundary is not billed. 17 minutes
  bills 15; 18 minutes bills 30.
- **Mode "nearest"**: to the nearest increment, a half increment rounding
  up. 23 minutes bills 30, 22 bills 15.
- Non-billable entries bill nothing.

## Overtime

For clients with overtime in their contract, billable time past 40 hours
per person per ISO week (Monday to Sunday, UTC) is billed at 1.5 times the
rate. Time is counted in billable (rounded) hours, in order of the entries'
start; the entry that crosses 40 hours is split.

## Invoices

An invoice has one line per role with regular and overtime hours (two
decimals), the role's hourly rate, and the amount:
`(regular hours x rate) + (overtime hours x rate x 1.5)`, rounded to the
cent once per line. VAT is the client's rate on the subtotal, rounded to
the cent.

## Timesheet export

Payroll imports `timesheetCsv` every Friday: one row per entry, by start,
with `tracked_hours` (as tracked) and `billable_hours` (after rounding, 0
for non-billable entries), both in hours with two decimals.

## Weekly summary

Managers get, per person and ISO week: tracked, billable, overtime and
non-billable hours. Overtime here is counted for every client, whatever
their contract says, because it is about people's hours, not billing.
