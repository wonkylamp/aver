# textkit API

Everything other services may import, from `src/index.ts`. This page is
the contract: behaviour not written here may change, behaviour written
here may not without a major version.

## Strings

- `titleCase(text)`: words split on spaces, hyphens, underscores and
  camelCase boundaries; apostrophes stay inside words, so "don't stop" is
  "Don't Stop". Each word capitalised, the rest lowercased; a, an, and,
  of, the, in, on, to and for stay lowercase except as the first word.
  Acronyms are not split: "HTTPServer guide" is "Httpserver Guide".
- `camelCase`, `kebabCase`, `snakeCase`: words are runs of letters and
  digits, split at camelCase boundaries and before the last capital of an
  acronym, so "HTTPServer" is "httpServer", "http-server", "http_server".
  Any other character separates words: "don't stop" is "don-t-stop".
- `slugify(text, max = 60)`: ASCII (accents removed; ß is ss, æ is ae, ø is
  o, œ is oe, ł is l, đ is d, þ is th), lowercase, words as in kebabCase,
  at most `max` characters, never ending in a hyphen.
  "Straße & Ærø" is "strasse-aero".
- `searchKey(text)`: accents removed, lowercased, whitespace collapsed and
  trimmed. Letters without a decomposition are kept: "Straße" is
  "straße", "Æble" is "æble".
- `collapseWhitespace(text)`: runs of whitespace become one space, trimmed.
- `truncate(text, max)`: at most `max` UTF-16 code units; when cut, the
  last of them is "…" (one character). `max` below 1 is a RangeError.
- `ellipsize(text, max)`: at most `max` user-perceived characters (an
  emoji or a flag is one); when cut, it ends in "..." (three dots), cut at
  the last space if that space is in the last third of the kept text.
  `max` below 4 is a RangeError.
- `initials(name, max = 2)`: first letters of the words (as titleCase splits
  them) that start with a letter, uppercased; with more words than `max`,
  the first `max - 1` and the last.

## Numbers

- `clamp(x, min, max)`: bounded to [min, max]. NaN gives min. `min > max`
  is a RangeError.
- `scale(x, inMin, inMax, outMin, outMax)`: linear map, clamped to the
  output range; NaN gives outMin. An empty input range is a RangeError.
- `percent(part, whole)`: whole percentage, rounded, bounded to 0-100.
  0 of 0 is 0; anything of 0 is 100. NaN gives NaN (the dashboard shows
  "n/a").
- `roundTo(x, digits = 0)`: half away from zero: 2.5 is 3, -2.5 is -3,
  1.005 to 2 digits is 1.01.
- `ordinal(n)`: 1st, 2nd, 3rd, 4th, 11th, 12th, 13th, 21st, 101st, 111th.
  Negative or fractional is a RangeError.
- `formatBytes(bytes)`: binary units B, KiB, MiB, GiB, TiB, one decimal at
  most, half away from zero: 1536 is "1.5 KiB", 1023 is "1023 B".
- `formatSize(bytes)`: decimal units B, kB, MB, GB, TB, one decimal at most:
  1500 is "1.5 kB", 999 is "999 B", 1000000 is "1 MB".

## Dates

Dates are calendar dates in UTC.

- `isoDate(d)`: "YYYY-MM-DD". An invalid date is a RangeError.
- `addBusinessDays(d, n)`: skips Saturdays and Sundays; negative goes back;
  the result is midnight UTC. Friday plus one is Monday.
- `relativeDays(d, now)`: "today", "tomorrow", "yesterday", "in N days",
  "N days ago", by calendar day.

## Collections

- `chunk(items, size)`: pieces of `size`; the last may be shorter; a size
  below 1 or not whole is a RangeError.
- `uniqBy(items, key)`: the first item per key, in order.
- `groupBy(items, key)`: a Map from key to items, keys in order of first
  appearance.
- `partition(items, test)`: [passing, failing], each in order.

## Validation

- `isEmail(text)`: loose: no spaces, one @, a dot in the domain followed by
  at least two characters. Surrounding whitespace is ignored.
- `isPostcodeUK(text)`: the usual forms, any case, with or without the
  space, and GIR 0AA.
