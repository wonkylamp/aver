# tempo-kit

Small timing and async utilities for Node services, all driven by an
injectable `Clock`:

| Module | What it does |
|---|---|
| `debounce` | Coalesces a burst of calls into one call. |
| `throttle` | Runs a function at most once per interval. |
| `sleep`, `withTimeout` | Promise helpers on the clock, with abort support. |
| `retry` + `backoff` | Retries an async operation with constant, linear or exponential backoff and optional jitter. |
| `TaskQueue` | FIFO async queue with a concurrency limit, pause, resume and clear. |
| `TtlCache` | Map with per-entry expiry and LRU eviction. |

The exact timing rules for `debounce` and `throttle` are in
[docs/timing.md](docs/timing.md).

```ts
import { debounce } from './src/index.ts';

const save = debounce((doc: Doc) => store.write(doc), 200);
editor.on('change', (doc) => save(doc));
```

## Clocks

Every utility takes an optional `clock`. Production code uses the default
`systemClock`. Tests pass a `ManualClock` and move time by hand, so they never
sleep and never depend on how busy the machine is:

```ts
const clock = new ManualClock();
const save = debounce(write, 200, { clock });
save(doc);
clock.advance(200); // write runs here
```

For promise-based code (`retry`, `sleep`), use `advanceAsync(clock, ms)` from
`src/testing/settle.ts`, which lets promise callbacks run after each timer.

## Development

Requires Node 22.18 or later (TypeScript runs through Node's type stripping;
there is no build step and there are no dependencies).

```sh
npm test
```
