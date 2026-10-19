import { toAscii } from "./accents.ts";

/** A key for case- and accent-insensitive search: "  Crème Brûlée " is "creme brulee". */
export function searchKey(text: string): string {
  return toAscii(text).toLowerCase().replace(/\s+/g, " ").trim();
}

export function collapseWhitespace(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}
