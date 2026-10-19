// String helpers, from the days when this package was one file.

/**
 * Words of a phrase for display: splits on spaces, hyphens and
 * underscores and at lower-to-upper camelCase boundaries; apostrophes stay
 * inside words ("don't" is one word).
 */
export function splitWords(text: string): string[] {
  return text
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .split(/[\s_-]+/)
    .filter(Boolean);
}

/** Accents removed (é is e). Other letters, ß and æ among them, are kept as they are. */
export function stripAccents(text: string): string {
  return text.normalize("NFD").replace(/[\u0300-\u036f]/g, "");
}

export function titleCase(text: string): string {
  const small = new Set(["a", "an", "and", "of", "the", "in", "on", "to", "for"]);
  return splitWords(text)
    .map((w, i) => {
      const lower = w.toLowerCase();
      if (i > 0 && small.has(lower)) return lower;
      return lower[0]!.toUpperCase() + lower.slice(1);
    })
    .join(" ");
}

/** A key for case- and accent-insensitive search: "  Crème Brûlée " is "creme brulee". */
export function searchKey(text: string): string {
  return stripAccents(text).toLowerCase().replace(/\s+/g, " ").trim();
}

export function collapseWhitespace(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

/** At most `max` UTF-16 code units, the last one an ellipsis when cut. */
export function truncate(text: string, max: number): string {
  if (max < 1) throw new RangeError("max must be at least 1");
  return text.length <= max ? text : `${text.slice(0, max - 1)}\u2026`;
}

/** "Ada Lovelace" is "AL"; at most `max` letters, from the first and last words when cut. */
export function initials(name: string, max = 2): string {
  const words = splitWords(name).filter((w) => /\p{L}/u.test(w[0]!));
  const picked = words.length > max ? [...words.slice(0, max - 1), words.at(-1)!] : words;
  return picked.map((w) => w[0]!.toUpperCase()).join("");
}
