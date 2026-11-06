import { $ } from "./dom.js";

const TABS = ["builds", "tests", "quality", "training", "terminal", "stats"];
const shown = new Map();
const hidden = new Map();
let current = null;

export function onShow(tab, handler) {
  shown.set(tab, handler);
}

export function onHide(tab, handler) {
  hidden.set(tab, handler);
}

export function activeTab() {
  return current;
}

function route() {
  const wanted = location.hash.slice(1);
  const tab = TABS.includes(wanted) ? wanted : "builds";
  if (tab === current) return;
  const previous = current;
  current = tab;
  for (const view of document.querySelectorAll("[data-view]")) {
    view.hidden = view.dataset.view !== tab;
  }
  for (const link of document.querySelectorAll("[data-tab]")) {
    if (link.dataset.tab === tab) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  if (previous) hidden.get(previous)?.();
  shown.get(tab)?.();
  const heading = document.querySelector(`[data-view="${tab}"] h1`);
  if (previous && heading) {
    heading.setAttribute("tabindex", "-1");
    heading.focus({ preventScroll: true });
  }
  window.scrollTo({ top: 0 });
}

export function initTabs() {
  window.addEventListener("hashchange", route);
  route();
}
