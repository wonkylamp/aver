// Identifier casing and URL slugs.
import { toAscii } from "./ascii.ts";
import { words } from "./words.ts";

export function camelCase(text: string): string {
  return words(text)
    .map((w, i) => (i === 0 ? w.toLowerCase() : w[0]!.toUpperCase() + w.slice(1).toLowerCase()))
    .join("");
}

export function kebabCase(text: string): string {
  return words(text)
    .map((w) => w.toLowerCase())
    .join("-");
}

export function snakeCase(text: string): string {
  return words(text)
    .map((w) => w.toLowerCase())
    .join("_");
}

/** URL slug: ASCII, lowercase, words joined by hyphens, at most `max` characters, never ending in a hyphen. */
export function slugify(text: string, max = 60): string {
  const slug = words(toAscii(text))
    .map((w) => w.toLowerCase())
    .join("-");
  return slug.slice(0, max).replace(/-+$/, "");
}
