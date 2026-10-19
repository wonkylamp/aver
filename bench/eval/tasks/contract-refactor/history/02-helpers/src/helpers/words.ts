/**
 * Words of an identifier or phrase: runs of letters and digits, split at
 * camelCase boundaries and before the last capital of an acronym
 * ("HTTPServer" is HTTP, Server). Anything else separates words, so
 * "don't" is "don" and "t".
 */
export function words(text: string): string[] {
  return text
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z])([A-Z][a-z])/g, "$1 $2")
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
}
