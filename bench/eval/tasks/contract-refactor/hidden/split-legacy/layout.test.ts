// docs/layout.md: one directory per area, the old homes gone.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

const src = join(import.meta.dirname, "..", "..", "src");

const AREAS: Record<string, string[]> = {
  strings: ["camelCase", "collapseWhitespace", "ellipsize", "initials", "kebabCase", "searchKey", "slugify", "snakeCase", "titleCase", "truncate"],
  numbers: ["clamp", "formatBytes", "formatSize", "ordinal", "percent", "roundTo", "scale"],
  dates: ["addBusinessDays", "isoDate", "relativeDays"],
  collections: ["chunk", "groupBy", "partition", "uniqBy"],
  validation: ["isEmail", "isPostcodeUK"],
};

for (const [area, names] of Object.entries(AREAS)) {
  test(`src/${area}/index.ts exports the ${area} functions`, async () => {
    const mod = (await import(join(src, area, "index.ts"))) as Record<string, unknown>;
    for (const n of names) assert.equal(typeof mod[n], "function", `${area} exports ${n}`);
  });
}

test("src/legacy, src/util.ts and src/helpers are gone", () => {
  for (const old of ["legacy", "util.ts", "helpers"]) assert.equal(existsSync(join(src, old)), false, old);
});
