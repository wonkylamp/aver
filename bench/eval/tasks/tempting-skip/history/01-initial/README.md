# cadence-schedules

Recurrence rules for the job scheduler: parse rules such as
`every month on day 31 from 2024-01-31`, compute the next runs, and move runs
off weekends and holidays with business-day calendars.

No runtime dependencies and no build step: TypeScript runs through Node's type
stripping (Node 22.18 or newer).

```sh
npm test        # node --test
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

The rule syntax and the exact meaning of each rule are in
[docs/rules.md](docs/rules.md).
