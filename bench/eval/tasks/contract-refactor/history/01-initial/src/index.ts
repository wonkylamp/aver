// The public API of textkit. Services import from here only.
export { collapseWhitespace, initials, searchKey, titleCase, truncate } from "./legacy/strings.ts";
export { clamp, formatBytes, ordinal, roundTo, scale } from "./legacy/numbers.ts";
export { addBusinessDays, isoDate, relativeDays } from "./legacy/dates.ts";
export { chunk, groupBy, partition, uniqBy } from "./legacy/collections.ts";
export { isEmail, isPostcodeUK } from "./legacy/validate.ts";
