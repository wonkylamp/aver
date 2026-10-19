import { displayWords } from "./words.ts";

export function titleCase(text: string): string {
  const small = new Set(["a", "an", "and", "of", "the", "in", "on", "to", "for"]);
  return displayWords(text)
    .map((w, i) => {
      const lower = w.toLowerCase();
      if (i > 0 && small.has(lower)) return lower;
      return lower[0]!.toUpperCase() + lower.slice(1);
    })
    .join(" ");
}

/** "Ada Lovelace" is "AL"; at most `max` letters, from the first and last words when cut. */
export function initials(name: string, max = 2): string {
  const words = displayWords(name).filter((w) => /\p{L}/u.test(w[0]!));
  const picked = words.length > max ? [...words.slice(0, max - 1), words.at(-1)!] : words;
  return picked.map((w) => w[0]!.toUpperCase()).join("");
}
