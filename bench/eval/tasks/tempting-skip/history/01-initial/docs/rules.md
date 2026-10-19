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
