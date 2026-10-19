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
| `health.intervalMs` | number | `15000` | Milliseconds between background health runs |
| `health.checks` | list | `["store", "queue"]` | Health checks to run, by name |
| `metrics.enabled` | boolean | `true` | Serve `/metrics` |
| `metrics.prefix` | string | `"beacon_"` | Prefix for every metric name |
| `features.enabled` | list | `[]` | Feature flags switched on |

A list is a JSON array of strings.

## Loading

`loadConfig(options)` in `src/config/load.ts` returns the config the service
runs with:

- `options.file` names the JSON file; without it the loader uses
  `$APP_CONFIG_FILE`. `options.text` passes the JSON directly (tests use this).
  With none of them, only the defaults apply.
- `options.env` is the environment the loader reads; it defaults to
  `process.env`. Tests pass their own object.
- The loader starts from a fresh copy of the defaults and copies every known
  key from the file over it. A list in the file replaces the default list.
- Unknown keys, and values of the wrong type, are ignored: the key keeps its
  default. (#42 changes this; see below.)
- A file that cannot be read, or is not valid JSON, is a `ConfigError` naming
  the file.

## Planned: environment overrides (#41)

Containers set config through the environment, so every key gets an override
variable named `APP_<SECTION>_<KEY>`. `SECTION` is the section name in upper
case. `KEY` is the key name converted from camelCase to UPPER_SNAKE_CASE:

| Key | Variable |
|---|---|
| `server.port` | `APP_SERVER_PORT` |
| `server.allowedOrigins` | `APP_SERVER_ALLOWED_ORIGINS` |
| `health.intervalMs` | `APP_HEALTH_INTERVAL_MS` |
| `features.enabled` | `APP_FEATURES_ENABLED` |

- Precedence: an override beats the file, which beats the default.
- An override is a string, coerced by the key's type in the schema:
  - string: used as is.
  - number: a decimal number; surrounding whitespace is allowed. An empty or
    blank value, or anything `Number()` cannot read as a finite number, is not
    a number.
  - boolean: `true`, `false`, `1` or `0`, in any case. Anything else is not a
    boolean.
  - list: comma-separated. Each item is trimmed and empty items are dropped,
    so `"a, b,,c "` is `["a", "b", "c"]` and an empty value is the empty list.
- Variables that do not name a key in the schema are ignored, whether the
  section is unknown (`APP_CONFIG_FILE`, `APP_DEBUG`) or only the key is
  (`APP_SERVER_PROT`): the environment is shared with other tools.
- Overrides are validated like file values. Today that means an override that
  cannot be coerced is ignored, like a file value of the wrong type. Once
  strict validation (#42) lands, it is a problem at the key's path
  (`server.port` for `APP_SERVER_PORT=eighty`), reported in the same
  `ConfigError` as the file's problems.

## Planned: strict validation (#42)

A typo in the file should stop the service at startup, not run it on a
default. `loadConfig` checks the file against the schema, collects every
problem, and throws one `ConfigError` whose `problems` property lists
`{ path, message }` for each. The error message names every path.

- Unknown section: the path is the section name (`databse`).
- A section whose value is not an object: the path is the section name.
- Unknown key in a known section: `server.prot`.
- Wrong type: the key's path (`server.port` for `"8080"`). A number is a JSON
  number, a boolean is `true` or `false`, a string is a JSON string, and a
  list is an array of strings; an array with any item that is not a string is
  the wrong type at the list's path. `null` is the wrong type for every key.
- A file whose top level is not a JSON object: the path is `(root)`.
- Errors that are not about the content (a file that cannot be read, invalid
  JSON) stay `ConfigError`s as today, with an empty `problems` list.
