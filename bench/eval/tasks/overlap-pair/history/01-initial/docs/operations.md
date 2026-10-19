# Operations

## Health

`GET /healthz` runs every check named in `health.checks` and answers 200 when
all of them pass, 503 otherwise. The body lists each check with `ok` and, for a
failure, a `detail`. A name in `health.checks` that no component registered is
reported as a failure, so a typo shows up on the dashboard instead of passing
quietly.

## Metrics

`GET /metrics` serves the Prometheus text format when `metrics.enabled` is true.
Every name starts with `metrics.prefix` (default `beacon_`). The gateway counts
`requests_total` by status class.

## Feature flags

`GET /flags/<name>` answers `{ "name": ..., "enabled": true|false }`. Flags are
switched on by listing them in `features.enabled`; names are case-insensitive.

## Logs

One line per request at `info`. Set `log.json` to false for plain text when
reading logs by hand.
