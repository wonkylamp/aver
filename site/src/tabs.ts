// Tabs for the install box. Without JavaScript the tab list stays hidden and
// every panel shows under its own heading; this turns the panels into one
// tab panel at a time, with the arrow-key behaviour of the WAI-ARIA tabs pattern.

/** The tab index a key moves to from `current` among `count` tabs, or null for other keys. */
export function nextTab(key: string, current: number, count: number): number | null {
  switch (key) {
    case "ArrowRight":
      return (current + 1) % count;
    case "ArrowLeft":
      return (current - 1 + count) % count;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

export function setUpTabs(root: HTMLElement): void {
  const list = root.querySelector<HTMLElement>('[role="tablist"]');
  if (list === null) throw new Error("index.html is missing the tab list in a [data-tabs] block");
  const tabs = [...list.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
  const panels = tabs.map((tab) => {
    const panel = document.getElementById(tab.getAttribute("aria-controls") ?? "");
    if (panel === null) throw new Error(`index.html has no panel for the tab ${tab.id}`);
    panel.setAttribute("role", "tabpanel");
    panel.setAttribute("aria-labelledby", tab.id);
    panel.setAttribute("tabindex", "0");
    return panel;
  });

  const select = (index: number, focus: boolean): void => {
    tabs.forEach((tab, i) => {
      const selected = i === index;
      tab.setAttribute("aria-selected", String(selected));
      tab.setAttribute("tabindex", selected ? "0" : "-1");
      panels[i]!.hidden = !selected;
    });
    if (focus) tabs[index]!.focus();
  };

  tabs.forEach((tab, i) => tab.addEventListener("click", () => select(i, false)));
  list.addEventListener("keydown", (event) => {
    const current = tabs.findIndex((tab) => tab.getAttribute("aria-selected") === "true");
    const next = nextTab(event.key, Math.max(current, 0), tabs.length);
    if (next === null) return;
    event.preventDefault();
    select(next, true);
  });

  const initial = tabs.findIndex((tab) => tab.getAttribute("aria-selected") === "true");
  select(Math.max(initial, 0), false);
  list.hidden = false;
  root.classList.add("is-live");
}
