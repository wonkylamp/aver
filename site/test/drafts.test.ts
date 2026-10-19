import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// The launch drafts live beside site/ and follow the same copy rules as the page.
const read = (name: string): string => readFileSync(new URL(`../../launch-drafts/${name}`, import.meta.url), "utf8");
const words = (text: string): number => text.split(/\s+/).filter((w) => /\w/.test(w)).length;

/** The body of the `## heading` section, up to the next `## `. */
function section(text: string, heading: string): string {
  const start = text.indexOf(`\n## ${heading}\n`);
  assert.ok(start >= 0, `a "## ${heading}" section`);
  const body = text.slice(start + heading.length + 5);
  const end = body.indexOf("\n## ");
  return end < 0 ? body : body.slice(0, end);
}

const description = "Aver is version control built for AI agents. Agents work entirely in Aver, and a change only counts once it's proven. Your team keeps Git: Aver hands it finished work.";
const pitch = "Agents don't use Git anymore. They use Aver. Every agent gets its own workspace, its own history, and merges that only happen when the work is proven. Your team keeps using Git, and Aver hands it finished work.";

test("[crit:drafts-written] SHOW_HN.md: titles under 80 characters, a first comment of at most 300 words that says what Aver doesn't do yet", () => {
  const text = read("SHOW_HN.md");
  const titles = section(text, "Title options")
    .split("\n")
    .filter((l) => l.startsWith("- "))
    .map((l) => l.slice(2));
  assert.ok(titles.length >= 2, "several title options");
  for (const title of titles) {
    assert.ok(title.startsWith("Show HN: "), title);
    assert.ok(title.length < 80, `${title.length} characters: ${title}`);
  }
  const comment = section(text, "First comment");
  assert.ok(words(comment) <= 300, `${words(comment)} words`);
  assert.match(comment, /What it doesn't do yet:/);
  assert.ok(comment.indexOf("doesn't do") > comment.indexOf("agents say \"done\""), "problem first");
  assert.ok(comment.includes(description));
});

test("[crit:drafts-written] POST.md: about 900 words on the problem, the pitch with the diagram, what it caught, what's next and how to try it", () => {
  const text = read("POST.md");
  const n = words(text);
  assert.ok(n >= 800 && n <= 1000, `${n} words`);
  for (const heading of ["The problem", "The pitch", "How it works", "What it caught", "What's next", "Try it in 2 minutes"]) section(text, heading);
  assert.ok(section(text, "The pitch").includes(pitch));
  assert.match(section(text, "How it works"), /!\[[^\]]+\]\(\.\.\/docs\/img\/agents-use-aver\.png\)/);
  assert.match(section(text, "What it caught"), /\{\{NUM:[a-z0-9-]+\}\}/, "eval numbers stay placeholders");
});

test("[crit:drafts-written] X_THREAD.md: 6 to 8 posts, each short enough to post", () => {
  const posts = read("X_THREAD.md").split(/\n## \d+\n/).slice(1);
  assert.ok(posts.length >= 6 && posts.length <= 8, `${posts.length} posts`);
  for (const post of posts) assert.ok(post.trim().length <= 280, `${post.trim().length} characters: ${post.trim().slice(0, 40)}`);
});

test("[crit:drafts-written] no draft states a dollar amount, uses a hype word or implies affiliation with Anthropic", () => {
  for (const name of ["SHOW_HN.md", "POST.md", "X_THREAD.md"]) {
    const text = read(name);
    assert.doesNotMatch(text, /\$\s?\d|\bUSD\b|\bdollars?\b/i, name);
    assert.doesNotMatch(text, /revolutionar|game.?chang|blazing|seamless|supercharg|magic|10x|unleash|cutting.edge/i, name);
    assert.match(text, /about 19% more/, `${name} states the token cost as about 19% more`);
    if (/Anthropic/.test(text)) assert.match(text, /not affiliated with Anthropic/, name);
    assert.doesNotMatch(text, /\{\{(?!NUM:[a-z0-9-]+\}\})/, `${name}: placeholders are {{NUM:name}}`);
  }
});
