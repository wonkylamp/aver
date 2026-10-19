// Article URLs and headings as the CMS builds them.
import { camelCase, ellipsize, kebabCase, slugify } from "../src/index.ts";

for (const title of ["Ærø: an island guide", "Straße der Nationen", "HTTPServer tuning, part 2"]) {
  console.log(`/articles/${slugify(title)}  id=${camelCase(title)}  css=${kebabCase(title)}  card="${ellipsize(title, 16)}"`);
}
