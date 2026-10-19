# Source layout

textkit grew in two waves: `src/legacy/` (the original single file, split
up mechanically) and `src/util.ts` plus `src/helpers/` (added later for the
casing and slug functions). Nobody can find anything, and some helpers
exist twice. The target layout is one directory per area, one file per
function family, and an `index.ts` per area that re-exports it:

| Area | Directory | Functions |
|---|---|---|
| strings | `src/strings/` | titleCase, camelCase, kebabCase, snakeCase, slugify, searchKey, collapseWhitespace, truncate, ellipsize, initials |
| numbers | `src/numbers/` | clamp, scale, percent, roundTo, ordinal, formatBytes, formatSize |
| dates | `src/dates/` | isoDate, addBusinessDays, relativeDays |
| collections | `src/collections/` | chunk, uniqBy, groupBy, partition |
| validation | `src/validation/` | isEmail, isPostcodeUK |

`src/index.ts` re-exports the areas and nothing else. `src/legacy/`,
`src/util.ts` and `src/helpers/` go away. The public API (`docs/api.md`)
does not change.
