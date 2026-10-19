import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { parseHTML } from "linkedom";
import { nextTab, setUpTabs } from "../src/tabs.ts";

const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const TOOLS = ["Claude Code", "Cursor", "Codex", "Gemini CLI", "VS Code", "Other"];

function page() {
  const { window, document } = parseHTML(html);
  // setUpTabs looks panels up through the global document, as in the browser.
  Object.assign(globalThis, { document });
  const root = document.querySelector<HTMLElement>(".setup-box [data-tabs]");
  assert.ok(root, "the install box has a [data-tabs] block");
  const tabs = [...root.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
  const panels = tabs.map((tab) => document.getElementById(tab.getAttribute("aria-controls") ?? "")!);
  const key = (name: string): void => {
    const event = new window.Event("keydown", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "key", { value: name });
    root.querySelector('[role="tablist"]')!.dispatchEvent(event);
  };
  const selected = (): string[] => tabs.filter((t) => t.getAttribute("aria-selected") === "true").map((t) => t.textContent!);
  const shown = (): string[] => panels.filter((p) => !p.hidden).map((p) => p.id);
  return { window, document, root, tabs, panels, key, selected, shown };
}

test("arrow keys wrap around the tabs, Home and End jump to the ends", () => {
  assert.equal(nextTab("ArrowRight", 0, 6), 1);
  assert.equal(nextTab("ArrowRight", 5, 6), 0);
  assert.equal(nextTab("ArrowLeft", 0, 6), 5);
  assert.equal(nextTab("Home", 3, 6), 0);
  assert.equal(nextTab("End", 1, 6), 5);
  assert.equal(nextTab("Enter", 2, 6), null);
});

test("without JavaScript the install box shows every tool's panel under a heading", () => {
  const { root, tabs, panels } = page();
  assert.deepEqual(
    tabs.map((t) => t.textContent),
    TOOLS,
  );
  assert.ok(root.querySelector('[role="tablist"]')!.hasAttribute("hidden"), "the tab list is hidden until the script runs");
  for (const panel of panels) {
    assert.ok(panel, "every tab controls a panel");
    assert.equal(panel.hidden, false);
    assert.ok(panel.querySelector(".tab-panel-title"), `${panel.id} has its own heading`);
  }
});

test("the install tabs show one panel at a time and follow clicks and arrow keys", () => {
  const { window, root, tabs, panels, key, selected, shown } = page();
  setUpTabs(root);
  assert.equal(root.querySelector('[role="tablist"]')!.hasAttribute("hidden"), false);
  panels.forEach((panel, i) => {
    assert.equal(panel.getAttribute("role"), "tabpanel");
    assert.equal(panel.getAttribute("aria-labelledby"), tabs[i]!.id);
  });
  assert.deepEqual(selected(), ["Claude Code"]);
  assert.deepEqual(shown(), ["tool-claude"]);
  assert.deepEqual(
    tabs.map((t) => t.getAttribute("tabindex")),
    ["0", "-1", "-1", "-1", "-1", "-1"],
  );

  tabs[2]!.dispatchEvent(new window.Event("click"));
  assert.deepEqual(selected(), ["Codex"]);
  assert.deepEqual(shown(), ["tool-codex"]);

  key("ArrowRight");
  assert.deepEqual(selected(), ["Gemini CLI"]);
  key("End");
  assert.deepEqual(selected(), ["Other"]);
  key("ArrowRight");
  assert.deepEqual(selected(), ["Claude Code"]);
  key("ArrowLeft");
  assert.deepEqual(shown(), ["tool-other"]);
  assert.equal(tabs[5]!.getAttribute("tabindex"), "0");
});

test("every Copy button on the landing page points at an element that exists", () => {
  const { document } = page();
  const buttons = [...document.querySelectorAll<HTMLButtonElement>("button[data-copy]")];
  assert.ok(buttons.length > 0);
  for (const button of buttons) assert.ok(document.getElementById(button.dataset.copy!), `no element #${button.dataset.copy}`);
  const prompt = document.getElementById("agent-install-text")!.textContent!;
  for (const step of ["install.sh", "aver doctor", "ask me before", "aver setup --yes"]) assert.ok(prompt.includes(step), `the agent prompt mentions ${step}`);
});
