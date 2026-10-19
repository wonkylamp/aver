// Two ways to split text into words, for two jobs; they differ on
// apostrophes, punctuation and acronyms, and the API pins both (docs/api.md).

/**
 * Words of a phrase for display (titleCase, initials): splits on spaces,
 * hyphens and underscores and at lower-to-upper camelCase boundaries;
 * apostrophes and other punctuation stay inside words ("don't" is one word).
 */
export function displayWords(text: string): string[] {
  return text
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .split(/[\s_-]+/)
    .filter(Boolean);
}

/**
 * Words of an identifier: runs of letters and digits, split at camelCase
 * boundaries and before the last capital of an acronym ("HTTPServer" is
 * HTTP, Server). Anything else separates words, so "don't" is "don" and "t".
 */
export function identifierWords(text: string): string[] {
  return text
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z])([A-Z][a-z])/g, "$1 $2")
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
}
