/** At most `max` UTF-16 code units, the last one an ellipsis when cut. */
export function truncate(text: string, max: number): string {
  if (max < 1) throw new RangeError("max must be at least 1");
  return text.length <= max ? text : `${text.slice(0, max - 1)}\u2026`;
}

/**
 * At most `max` user-perceived characters (an emoji is one), ending in
 * three dots when cut, and cut at a word boundary when there is one in the
 * last third.
 */
export function ellipsize(text: string, max: number): string {
  if (max < 4) throw new RangeError("max must be at least 4");
  const chars = [...new Intl.Segmenter("en", { granularity: "grapheme" }).segment(text)].map((s) => s.segment);
  if (chars.length <= max) return text;
  const head = chars.slice(0, max - 3).join("");
  const space = head.lastIndexOf(" ");
  const cut = space >= Math.floor(((max - 3) * 2) / 3) ? head.slice(0, space) : head;
  return `${cut.trimEnd()}...`;
}
