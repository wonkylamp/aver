# Configuration

beacon-service reads one JSON file with a fixed set of sections. Every key has a
default, so an empty file (or no file) is a valid config.
[config/example.json](../config/example.json) shows every key.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `server.host` | string | `"0.0.0.0"` | Interface to bind |
| `server.port` | number | `8080` | TCP port to listen on |
| `server.trustProxy` | boolean | `false` | Take the client address from `X-Forwarded-For` |
| `server.allowedOrigins` | list | `[]` | Origins allowed to call the API from a browser |
| `log.level` | string | `"info"` | Lowest level written: `debug`, `info`, `warn` or `error` |
| `log.json` | boolean | `true` | One JSON object per line instead of plain text |
| `log.color` | boolean | `false` | Colour the level in plain-text logs |
| `health.intervalMs` | number | `15000` | Milliseconds between background health runs |
| `health.checks` | list | `["store", "queue"]` | Health checks to run, by name |
| `metrics.enabled` | boolean | `true` | Serve `/metrics` |
| `metrics.prefix` | string | `"beacon_"` | Prefix for every metric name |
| `features.enabled` | list | `[]` | Feature flags switched on |

A list is a JSON array of strings.

## Loading

`loadConfig(options)` in `src/config/load.ts` returns the config the service
runs with:

- `options.file` names the JSON file; `options.text` passes the JSON directly
  (tests use this). With neither, only the defaults apply.
- The loader starts from a fresh copy of the defaults and copies every known
  key from the file over it. A list in the file replaces the default list.
- Unknown keys, and values of the wrong type, are ignored: the key keeps its
  default.
- A file that cannot be read, or is not valid JSON, is a `ConfigError` naming
  the file.
