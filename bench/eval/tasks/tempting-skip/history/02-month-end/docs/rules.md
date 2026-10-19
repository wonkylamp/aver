# Recurrence rules

A rule says when a scheduled job runs. Every rule has a start date, a unit
(day, week or month) and an interval.

## Syntax

```
every [N] day[s] from YYYY-MM-DD
every [N] week[s] [on <weekday>] from YYYY-MM-DD
every [N] month[s] [on day D] from YYYY-MM-DD
```

`N` defaults to 1 and may be at most 1000. Words are case-insensitive and
extra whitespace is ignored. A weekly rule without a weekday runs on the start
date's weekday; a monthly rule without a day runs on the start date's day of
the month.

## Runs

- The **first run** is the first date on or after the start date that matches
  the rule.
- **Daily**: the start date, then every `N` days.
- **Weekly**: the first `<weekday>` on or after the start date, then every
  `7 × N` days.
- **Monthly**: day `D` of the start month and of every `N`th month after it.
  If day `D` of the start month is before the start date, the first run is in
  the start month plus `N`.
- `nextRun(rule, after)` returns the first run strictly after `after`. If
  `after` is before the first run, it returns the first run.

## Month-end

Monthly rules on days 29, 30 and 31 meet months that are shorter than the day.
In such a month the run is on the **last day of the month**: day 31 runs on
30 April, on 28 February, and on 29 February in leap years. Day 30 and day 29
work the same way in February.

Clamping applies to that month only. The rule's day is not changed by it, so
the run in the next long month is on the rule's day again:

```
every month on day 31 from 2024-01-31
  2024-01-31, 2024-02-29, 2024-03-31, 2024-04-30, 2024-05-31, ...
```

A run must never "drift" to the 28th or 29th for the rest of the year because
February was short. Billing and payroll jobs depend on running on the last
day of the month.

The same holds for every interval: `every 2 months on day 31 from 2023-12-31`
runs on 2023-12-31, 2024-02-29, 2024-04-30, 2024-06-30, 2024-08-31, ...

All dates are calendar days in UTC. Time zones and times of day are out of
scope; the job runner decides when during the day a run starts.

## Business days

Schedules can be moved off weekends and holidays with a calendar
(`src/business-days.ts`) and an adjustment:

| Adjustment | A run on a non-business day moves to |
|---|---|
| `none` | stays where it is |
| `following` | the next business day |
| `preceding` | the previous business day |
| `modified-following` | the next business day, unless that is in the next month, then the previous one |

Adjustment never changes the schedule itself: the run after an adjusted run is
computed from the unadjusted date.
