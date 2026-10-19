// Identifier casing.
import { identifierWords } from "./words.ts";

export function camelCase(text: string): string {
  return identifierWords(text)
    .map((w, i) => (i === 0 ? w.toLowerCase() : w[0]!.toUpperCase() + w.slice(1).toLowerCase()))
    .join("");
}

export function kebabCase(text: string): string {
  return identifierWords(text)
    .map((w) => w.toLowerCase())
    .join("-");
}

export function snakeCase(text: string): string {
  return identifierWords(text)
    .map((w) => w.toLowerCase())
    .join("_");
}
