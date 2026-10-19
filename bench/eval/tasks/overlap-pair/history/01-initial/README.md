# beacon-service

The gateway in front of the beacon fleet. It answers health probes, serves
Prometheus metrics and tells clients which feature flags are on.

## Running the tests

```sh
npm test
```

The project has no dependencies. It runs TypeScript directly with Node's type
stripping, so it needs Node 22.18 or later.

## Layout

| Path | What it holds |
|---|---|
| `src/config/` | The config schema, its defaults and the loader. See [docs/config.md](docs/config.md). |
| `src/logger.ts` | Leveled logger, plain text or one JSON object per line. |
| `src/http/` | The request router and CORS handling. |
| `src/health/` | Named health checks and the `/healthz` report. |
| `src/metrics/` | Counters and gauges in the Prometheus text format. |
| `src/flags/` | Feature flags read from config. |
| `src/app.ts` | Wires the pieces together from a loaded config. |

Operational notes are in [docs/operations.md](docs/operations.md).
