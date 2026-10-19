// The landing page's behaviours: Copy buttons, the install tabs and the cause explorer.
// Like the viewer, it writes text only through textContent.
import { CAUSE_TEXT, cycle, decide, PRESETS, stopReason, VERDICT_TEXT, type ExplorerState } from "./explorer.ts";
import { CAUSE_RULES, type Outcome } from "./rules.ts";
import { setUpTabs } from "./tabs.ts";

const MARK: Record<Outcome, string> = { pass: "✓", fail: "✕", cap: "!", sandbox: "!" };
/** What each outcome is called on a run cell: a cap is the memory or time limit. */
const WORD: Record<Outcome, string> = { pass: "pass", fail: "fail", cap: "limit", sandbox: "sandbox" };

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function setUpCopyButtons(): void {
  for (const button of document.querySelectorAll<HTMLButtonElement>("button[data-copy]")) {
    const target = document.getElementById(button.dataset.copy ?? "");
    if (target === null || !navigator.clipboard) continue;
    const label = button.textContent ?? "Copy";
    let reset: number | undefined;
    button.hidden = false;
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText((target.textContent ?? "").trim());
        button.textContent = "Copied";
      } catch {
        // The browser refused the clipboard: select the text so a keyboard copy works.
        const range = document.createRange();
        range.selectNodeContents(target);
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range);
        button.textContent = "Selected";
      }
      window.clearTimeout(reset);
      reset = window.setTimeout(() => (button.textContent = label), 2000);
    });
  }
}

function setUpExplorer(root: HTMLElement): void {
  const find = <T extends Element>(selector: string): T => {
    const node = root.querySelector<T>(selector);
    if (node === null) throw new Error(`index.html is missing ${selector} in the explorer`);
    return node;
  };
  const presetButtons = [...root.querySelectorAll<HTMLButtonElement>("button[data-preset]")];
  const exists = { base: find<HTMLInputElement>("#exists-base"), head: find<HTMLInputElement>("#exists-head") };
  const runLists = { base: find<HTMLOListElement>("#runs-base"), head: find<HTMLOListElement>("#runs-head") };
  const stops = { base: find<HTMLElement>("#stop-base"), head: find<HTMLElement>("#stop-head") };
  const proves = find<HTMLInputElement>("#proves");
  const rules = [...root.querySelectorAll<HTMLLIElement>(".ladder > li")];
  const cause = find<HTMLElement>("#outcome-cause");
  const causeText = find<HTMLElement>("#outcome-cause-text");
  const verdictName = find<HTMLElement>("#outcome-verdict");
  const verdictText = find<HTMLElement>("#outcome-verdict-text");
  if (rules.length !== CAUSE_RULES.length) throw new Error("the explorer's rule list does not match rules.ts");

  let state: ExplorerState = PRESETS[0]!.state;
  let preset: string | null = PRESETS[0]!.id;
  // Keeps keyboard focus on the run the visitor just changed after a re-render.
  let focusAfter: { side: "base" | "head"; index: number } | null = null;

  const render = (): void => {
    const d = decide(state);
    for (const side of ["base", "head"] as const) {
      const runs = side === "base" ? d.baseRuns : d.headRuns;
      exists[side].checked = state[side].exists;
      runLists[side].replaceChildren(
        ...runs.map((outcome, index) => {
          const button = el("button", `run-cell is-${outcome}`);
          button.type = "button";
          button.setAttribute("aria-label", `${side === "base" ? "Base" : "Head"} run ${index + 1}: ${WORD[outcome]}. Press to change.`);
          button.append(el("span", "run-mark", MARK[outcome]), el("span", "run-word", WORD[outcome]));
          button.addEventListener("click", () => {
            state = cycle(state, side, index);
            preset = null;
            focusAfter = { side, index };
            render();
          });
          const item = el("li", "run-slot");
          item.append(button);
          return item;
        }),
      );
      stops[side].textContent = stopReason(runs, side);
    }
    proves.checked = state.provesCriterion;
    rules.forEach((li, i) => {
      li.classList.toggle("is-passed", i < d.rule);
      li.classList.toggle("is-match", i === d.rule);
      li.classList.toggle("is-unreached", i > d.rule);
      if (i === d.rule) li.setAttribute("aria-current", "true");
      else li.removeAttribute("aria-current");
    });
    const c = d.classification.cause;
    cause.textContent = c ?? "No cause";
    cause.dataset.cause = c ?? "none";
    causeText.textContent = CAUSE_TEXT[c ?? "none"];
    verdictName.textContent = d.verdict;
    verdictName.dataset.verdict = d.verdict;
    verdictText.textContent = state.provesCriterion
      ? `${VERDICT_TEXT[d.verdict]}${d.unproven.length > 0 ? " div-zero is unproven." : " div-zero is proven."}`
      : VERDICT_TEXT[d.verdict];
    for (const button of presetButtons) button.setAttribute("aria-pressed", String(button.dataset.preset === preset));
    if (focusAfter !== null) {
      const cells = runLists[focusAfter.side].querySelectorAll<HTMLButtonElement>("button");
      (cells[focusAfter.index] ?? cells[cells.length - 1])?.focus();
      focusAfter = null;
    }
  };

  for (const button of presetButtons) {
    button.addEventListener("click", () => {
      const chosen = PRESETS.find((p) => p.id === button.dataset.preset);
      if (chosen === undefined) return;
      state = chosen.state;
      preset = chosen.id;
      render();
    });
  }
  for (const side of ["base", "head"] as const) {
    exists[side].addEventListener("change", () => {
      state = { ...state, [side]: { ...state[side], exists: exists[side].checked } };
      preset = null;
      render();
    });
  }
  proves.addEventListener("change", () => {
    state = { ...state, provesCriterion: proves.checked };
    preset = null;
    render();
  });

  root.classList.add("is-live");
  render();
}

setUpCopyButtons();
for (const tabs of document.querySelectorAll<HTMLElement>("[data-tabs]")) setUpTabs(tabs);
const explorer = document.getElementById("explorer");
if (explorer !== null) setUpExplorer(explorer);
