import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { parseHTML } from "linkedom";
import { setUpTabs } from "../src/tabs.ts";

const read = (path: string): string => readFileSync(new URL(path, import.meta.url), "utf8");
const html = read("../index.html");
// The install commands must be the ones the documentation gives, word for word.
const docs = ["../../README.md", "../../docs/INTEGRATIONS.md", "../../docs/WINDOWS.md"].map(read).join("\n");

function page() {
  const { window, document } = parseHTML(html);
  return { window, document };
}

const commands = (root: Element): string[] => [...root.querySelectorAll(".terminal code")].map((c) => c.textContent!.trim());

test("the Claude Code tab adds the plugin, then installs aver, then runs aver setup", () => {
  const { document } = page();
  const panel = document.getElementById("tool-claude")!;
  assert.deepEqual(commands(panel), [
    "/plugin marketplace add wonkylamp/aver",
    "/plugin install aver@aver",
    "curl -fsSL https://aver.build/install.sh | sh",
    "aver setup",
  ]);
  const tabs = [...document.querySelectorAll('.setup-box [role="tab"]')];
  assert.equal(tabs[0]!.getAttribute("aria-controls"), "tool-claude", "Claude Code is the first tab");
});

test("every other tool's tab installs aver, then runs aver integrate for that tool", () => {
  const { document } = page();
  const tools: Record<string, string> = {
    "tool-cursor": "cursor",
    "tool-codex": "codex",
    "tool-gemini": "gemini",
    "tool-vscode": "vscode",
    "tool-other": "agents-md",
  };
  for (const [id, tool] of Object.entries(tools)) {
    const panel = document.getElementById(id);
    assert.ok(panel, `the install box has the panel ${id}`);
    assert.deepEqual(commands(panel), ["curl -fsSL https://aver.build/install.sh | sh", `aver integrate ${tool}`], id);
  }
});

test("every command in the install box is one the documentation gives", () => {
  const { document } = page();
  const box = document.querySelector(".setup-box")!;
  const inline = [...box.querySelectorAll("code")].map((c) => c.textContent!.trim()).filter((t) => /^(aver|\/plugin|curl|cargo) /.test(t) );
  assert.ok(inline.length >= 10);
  for (const command of inline) assert.ok(docs.includes(command), `not in README.md or docs/: ${command}`);
});

test("the install box says how to run Aver on Windows and links the WSL2 guide", () => {
  const { document } = page();
  const note = document.querySelector(".setup-box .win-note");
  assert.ok(note, "the install box has a Windows note");
  assert.match(note.textContent!, /WSL2/);
  assert.match(note.textContent!, /\/mnt\/c/);
  const link = note.querySelector('a[href$="docs/WINDOWS.md"]');
  assert.ok(link, "the note links docs/WINDOWS.md");
  assert.ok(existsSync(new URL("../../docs/WINDOWS.md", import.meta.url)));
});

test("what your agent sees: a verify that fails on the agent's own break, then one that passes", () => {
  const { window, document } = page();
  Object.assign(globalThis, { document });
  const lines = (id: string): string[] => document.querySelector(`#${id} pre`)!.textContent!.trimEnd().split("\n");
  const fail = lines("report-fail");
  assert.equal(fail[0], "$ aver verify");
  assert.match(fail[1]!, /^NOT DONE: FAIL_OWN\. 1 of 1 criteria proven, 1 new failure is yours\.$/);
  assert.match(fail.at(-1)!, /^NOT DONE: FAIL_OWN/);
  assert.ok(fail.includes("unit::math > sub returns the difference"));
  assert.ok(fail.some((l) => /trimmed on this page/.test(l)), "the trimmed stack trace is marked");
  const pass = lines("report-pass");
  assert.equal(pass[1], "DONE: PASS. 1 of 1 criteria proven, no new failures.");
  assert.equal(pass.at(-1), "DONE: PASS");
  for (const report of [fail, pass]) {
    assert.ok(report.includes("unit::[crit:div-zero] divide throws on division by zero"), "the old failures show as still failing");
  }

  const root = document.querySelector<HTMLElement>(".report-tabs")!;
  setUpTabs(root);
  const panels = ["report-fail", "report-pass"].map((id) => document.getElementById(id)!);
  assert.deepEqual(
    panels.map((p) => p.hidden),
    [false, true],
  );
  root.querySelector("#tab-report-pass")!.dispatchEvent(new window.Event("click"));
  assert.deepEqual(
    panels.map((p) => p.hidden),
    [true, false],
  );
});

test("[crit:site-positioning] the hero, title and description carry the locked headline and description, and say proven, not sandboxed", () => {
  const { document } = page();
  assert.equal(document.querySelector("h1")!.textContent, "Your agent says \u201cdone\u201d. Aver checks.");
  const subline = "Aver is version control built for AI agents. Agents work entirely in Aver, and a change only counts once it\u2019s proven. Your team keeps Git: Aver hands it finished work.";
  assert.equal(document.querySelector(".hero .lede")!.textContent, subline);
  assert.match(document.querySelector("title")!.textContent!, /Aver checks/);
  assert.ok(document.querySelector('meta[name="description"]')!.getAttribute("content")!.includes(subline));
  assert.doesNotMatch(document.querySelector("main")!.textContent!, /every result is checked in a sandbox|only the checks run in sandboxes/i);
});

test("[crit:site-positioning] right under the hero: the pitch, then the Git to Aver table, then the three problems", () => {
  const { document } = page();
  const sections = [...document.querySelectorAll("main > section")].map((s) => s.id || s.className);
  assert.deepEqual(sections.slice(0, 4), ["wrap hero", "pitch", "git-to-aver", "problems"]);
  assert.equal(document.querySelectorAll("#problems .problems > li").length, 3);
});

test("the Git to Aver table has twelve labelled rows, the strongest three marked and the unbuilt ones marked coming", () => {
  const { document } = page();
  const table = document.querySelector("#git-to-aver table")!;
  const columns = [...table.querySelectorAll("thead th[scope=col]")].map((th) => th.textContent!.trim());
  assert.deepEqual(columns, ["With Git you\u2026", "Aver gives agents\u2026", "Why agents need it"]);
  const rows = [...table.querySelectorAll("tbody tr")];
  assert.equal(rows.length, 12);
  for (const row of rows) {
    const cells = [row.querySelector("th[scope=row]"), ...row.querySelectorAll("td")];
    assert.deepEqual(
      cells.map((c) => c?.getAttribute("data-label")),
      columns,
    );
  }
  const key = rows.flatMap((row, i) => (row.classList.contains("is-key") ? [i + 1] : []));
  assert.deepEqual(key, [3, 4, 5]);
  const coming = rows.flatMap((row, i) => (row.querySelector(".soon") ? [i + 1] : []));
  assert.deepEqual(coming, [11]);
});

test("every aver command the Git to Aver table names exists, unless it is marked coming", () => {
  const { document } = page();
  // The subcommands are the variants of the CLI's command enum, one `Name => module,` line each.
  const cli = read("../../crates/aver/src/cli/mod.rs");
  const modules = new Map([...cli.matchAll(/^\s+([A-Z][a-z]+) => ([a-z]+),$/gm)].map((m) => [m[1]!.toLowerCase(), m[2]!]));
  const subcommands = new Set(modules.keys());
  assert.ok(subcommands.has("check") && subcommands.has("combine"));
  for (const cell of document.querySelectorAll("#git-to-aver td")) {
    for (const code of cell.querySelectorAll("code")) {
      const words = code.textContent!.split(" ");
      if (words[0] !== "aver") continue;
      if (cell.querySelector(".soon")) {
        assert.ok(!subcommands.has(words[1]!), `${code.textContent} is marked coming but exists`);
        continue;
      }
      assert.ok(subcommands.has(words[1]!), `no subcommand aver ${words[1]}`);
      // A clap flag --x-y is the field `x_y:` of the command's Args, in its module.
      const flags = read(`../../crates/aver/src/cli/${modules.get(words[1]!)}.rs`);
      for (const flag of words.filter((w) => w.startsWith("--"))) assert.match(flags, new RegExp(`\\b${flag.slice(2).replace(/-/g, "_")}: `), `no flag ${flag}`);
    }
  }
});

test("every link to a section of the landing page lands on one", () => {
  const { document } = page();
  const ids = new Set([...document.querySelectorAll("[id]")].map((el) => el.id));
  const links: [string, string][] = [];
  for (const a of document.querySelectorAll('a[href^="#"]')) links.push(["index.html", a.getAttribute("href")!.slice(1)]);
  for (const [file, prefix] of [
    ["../receipt.html", "./#"],
    ["../404.html", "/#"],
  ] as const) {
    for (const a of parseHTML(read(file)).document.querySelectorAll("a[href]")) {
      const href = a.getAttribute("href")!;
      if (href.startsWith(prefix)) links.push([file, href.slice(prefix.length)]);
    }
  }
  assert.ok(links.length > 5);
  for (const [file, id] of links) assert.ok(ids.has(id), `${file} links to #${id}, which index.html lacks`);
});

test("benchmark numbers are well-formed placeholders, each alone in a .num span", () => {
  const { document } = page();
  for (const match of html.matchAll(/\{\{[^}]*\}\}/g)) assert.match(match[0], /^\{\{NUM:[a-z0-9-]+\}\}$/);
  for (const span of document.querySelectorAll(".num")) assert.match(span.textContent!, /^(\{\{NUM:[a-z0-9-]+\}\}|[^{}]+)$/);
  const loose = html.replace(/<span class="num">\{\{NUM:[a-z0-9-]+\}\}<\/span>/g, "");
  assert.doesNotMatch(loose, /\{\{NUM:/, "every placeholder sits in a .num span");
});

test("[crit:site-positioning] no page names Anthropic or Scion, so none can read as affiliated", () => {
  for (const file of ["../index.html", "../404.html", "../receipt.html"]) assert.doesNotMatch(read(file), /Anthropic|Scion/, file);
});

test("[crit:site-positioning] no page states a dollar amount", () => {
  for (const file of ["../index.html", "../404.html", "../receipt.html"]) {
    const text = parseHTML(read(file)).document.querySelector("body")!.textContent!;
    assert.doesNotMatch(text, /\$\s?\d|\bUSD\b/, file);
  }
});

const pitch = "Agents don\u2019t use Git anymore. They use Aver. Every agent gets its own workspace, its own history, and merges that only happen when the work is proven. Your team keeps using Git, and Aver hands it finished work.";
const split: [string, string, string][] = [
  ["Where work happens", "Each agent gets its own instant workspace", "Your repo, unchanged"],
  ["Saving progress", "Every change snapshotted in the agent\u2019s own history", "Commits, as today"],
  ["\u201cIt\u2019s done\u201d", "Only counts once it\u2019s proven: each criterion has a test that fails without the change, nothing that worked broke, no test was weakened", "Arrives already proven, with a receipt"],
  ["Working in parallel", "Agents are told when they touch the same files", "No collisions to untangle"],
  ["Merging", "Merges only when everything merged together still passes", "Receives finished work as a branch or PR"],
  ["What you do", "Nothing", "Review, merge and deploy as now"],
];

test("[crit:site-positioning] the pitch, then the diagram in light and dark, then the Agents and Your team table", () => {
  const { document } = page();
  const section = document.getElementById("pitch")!;
  const kids = [...section.children].map((el) => el.tagName.toLowerCase() + (el.className ? `.${el.className.split(" ")[0]}` : ""));
  assert.deepEqual(kids, ["h2", "p.section-lede", "figure.diagram", "div.vs-wrap"]);
  assert.equal(section.querySelector("p")!.textContent, pitch);

  const img = section.querySelector("figure img")!;
  const dark = section.querySelector('figure picture source[media="(prefers-color-scheme: dark)"]')!;
  assert.equal(img.getAttribute("src"), "img/agents-use-aver.png");
  assert.equal(dark.getAttribute("srcset"), "img/agents-use-aver-dark.png");
  assert.ok(img.getAttribute("alt")!.length > 80, "the diagram has a real description");
  for (const name of ["agents-use-aver.png", "agents-use-aver-dark.png"]) {
    const site = readFileSync(new URL(`../img/${name}`, import.meta.url));
    assert.ok(site.equals(readFileSync(new URL(`../../docs/img/${name}`, import.meta.url))), `site/img/${name} is the copy of docs/img/${name}`);
  }

  const table = section.querySelector("table")!;
  assert.deepEqual(
    [...table.querySelectorAll("thead th")].map((th) => th.textContent!.trim()),
    ["Agents, in Aver", "Your team, in Git"],
  );
  assert.deepEqual(
    [...table.querySelectorAll("tbody tr")].map((tr) => [...tr.children].map((c) => c.textContent!.trim())),
    split,
  );
});

test("[crit:site-mobile] the pitch table stacks into labelled cells on a phone and the diagram scales to the screen", () => {
  const { document } = page();
  const table = document.querySelector("#pitch table")!;
  assert.ok(table.classList.contains("vs"), "the table takes the stacking .vs layout");
  const columns = [...table.querySelectorAll("thead th")].map((th) => th.textContent!.trim());
  for (const row of table.querySelectorAll("tbody tr")) {
    assert.ok(row.querySelector("th[scope=row]"), "each row is named by its header cell");
    assert.deepEqual(
      [...row.querySelectorAll("td")].map((td) => td.getAttribute("data-label")),
      columns,
    );
  }
  const img = document.querySelector("#pitch figure img")!;
  assert.ok(Number(img.getAttribute("width")) > 0 && Number(img.getAttribute("height")) > 0, "the diagram reserves its space");

  const css = read("../style.css");
  const rule = (selector: string): string => {
    const at = css.indexOf(`\n${selector} {`);
    assert.ok(at >= 0, `style.css has a rule for ${selector}`);
    return css.slice(at, css.indexOf("}", at));
  };
  assert.match(rule(".diagram img"), /width: 100%;/);
  assert.match(rule(".diagram img"), /height: auto;/);
  // Fixed column widths would survive the stacked layout's width: auto unless they apply only to wide screens.
  const wide = css.indexOf("@media (min-width: 40.01rem)");
  assert.ok(wide >= 0);
  for (const m of css.matchAll(/\.vs\.split[^{]*\{[^}]*\bwidth:/g)) assert.ok(m.index! > wide && css.lastIndexOf("@media", m.index) === wide, "split column widths only side by side");
  assert.match(css, /\.vs td \{\s*display: block;/, "the .vs cells stack on narrow screens");
});
