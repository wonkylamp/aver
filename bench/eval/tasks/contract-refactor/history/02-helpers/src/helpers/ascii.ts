// Transliteration to ASCII for URLs and file names.

const SPECIAL: Record<string, string> = { ß: "ss", æ: "ae", Æ: "Ae", ø: "o", Ø: "O", œ: "oe", Œ: "Oe", ł: "l", Ł: "L", đ: "d", Đ: "D", þ: "th", Þ: "Th" };

/** Accents removed and the letters that have no decomposition spelled out: "Straße" is "Strasse". */
export function toAscii(text: string): string {
  return text
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[ßæÆøØœŒłŁđĐþÞ]/g, (c) => SPECIAL[c]!);
}
