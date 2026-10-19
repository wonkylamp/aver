# cadence-schedules

Recurrence rules for the job scheduler: parse rules such as
`every month on day 31 from 2024-01-31`, compute the next runs, and move runs
off weekends and holidays with business-day calendars. The billing and payroll
jobs are scheduled with these rules, so month-end behaviour matters.

No runtime dependencies and no build step: TypeScript runs through Node's type
stripping (Node 22.18 or newer).

```sh
npm test                                        # node --test
node src/main.ts next "every month on day 31 from 2024-01-31" --count 6
node src/main.ts next "every month on day 1 from 2024-01-01" --calendar target
node src/main.ts describe "every 2 weeks from 2024-01-05"
```

## Layout

| Path | What it holds |
|---|---|
| `src/dates.ts` | Calendar-day helpers on UTC midnight dates |
| `src/rules.ts` | The rule type and `parseRule` |
| `src/next-run.ts` | `firstRun` and `nextRun` |
| `src/schedule.ts` | `upcoming`, `runsBetween`, `adjustedRuns` |
| `src/business-days.ts` | Calendars and business-day adjustment |
| `src/holidays.ts` | Easter and the TARGET2 closing days |
| `src/format.ts` | Canonical rule text and run listings |
| `src/cli.ts`, `src/main.ts` | The `cadence` command line |

The rule syntax and the exact meaning of each rule, including month-end
handling, are in [docs/rules.md](docs/rules.md).
