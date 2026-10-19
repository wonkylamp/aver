// Tags: free-form labels on transactions ("work", "holiday-2026").

export class TagError extends Error {
  override name = "TagError";
}

/** Lowercase, words joined by hyphens, letters, digits and hyphens only. */
export function normalizeTag(raw: string): string {
  const t = raw
    .trim()
    .replace(/^#/, "")
    .toLowerCase()
    .replace(/[\s_]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
  if (!/^[a-z0-9-]{1,32}$/.test(t)) throw new TagError(`not a tag: ${JSON.stringify(raw)}`);
  return t;
}

/** Tags written in a memo: "#work #Client_A lunch" has work and client-a. */
export function tagsInText(text: string): string[] {
  const out = new Set<string>();
  for (const m of text.matchAll(/(?:^|\s)#([A-Za-z0-9][\w-]*)/g)) out.add(normalizeTag(m[1]!));
  return [...out].sort();
}

export function addTags(tags: string[], more: string[]): string[] {
  return [...new Set([...tags, ...more.map(normalizeTag)])].sort();
}
