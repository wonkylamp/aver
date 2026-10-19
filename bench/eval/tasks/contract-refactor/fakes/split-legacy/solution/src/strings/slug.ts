import { toAscii } from "./accents.ts";
import { identifierWords } from "./words.ts";

/** URL slug: ASCII, lowercase, words joined by hyphens, at most `max` characters, never ending in a hyphen. */
export function slugify(text: string, max = 60): string {
  const slug = identifierWords(toAscii(text))
    .map((w) => w.toLowerCase())
    .join("-");
  return slug.slice(0, max).replace(/-+$/, "");
}
