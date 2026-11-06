export const $ = (id) => document.getElementById(id);

export function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export async function api(path, options = {}) {
  const response = await fetch(path, { credentials: "same-origin", ...options });
  if (response.status === 401) {
    window.location.replace("/login");
    throw new Error("unauthorized");
  }
  if (!response.ok) {
    const body = await response.json().catch(() => null);
    throw new Error(body?.error?.message ?? "Что-то пошло не так, попробуйте ещё раз");
  }
  return response.status === 204 ? null : response.json();
}

export function showError(id, error) {
  if (error.message !== "unauthorized") $(id).textContent = error.message;
}

// Значки состояний: у каждого своя форма, чтобы состояние читалось не только по цвету.
const ICONS = {
  ok: "M20 6 9 17l-5-5",
  bad: "M18 6 6 18M6 6l12 12",
  running: "M12 3a9 9 0 1 0 9 9",
  waiting: "M12 7v5l3 2M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z",
  stopped: "M6 12h12",
};

export function statusIcon(kind) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.classList.add("icon", "status-icon");
  if (kind === "running") svg.classList.add("spin");
  const path = document.createElementNS(ns, "path");
  path.setAttribute("d", ICONS[kind] ?? ICONS.stopped);
  svg.append(path);
  return svg;
}

export function badge(kind, label) {
  const node = el("span", "badge");
  node.dataset.tone = kind;
  node.append(statusIcon(kind), el("span", null, label));
  return node;
}

export async function showLog(title, path) {
  const dialog = $("log-dialog");
  $("log-title").textContent = title;
  $("log-text").textContent = "Загружаю…";
  $("log-cut").hidden = true;
  dialog.showModal();
  try {
    const log = await api(path);
    $("log-text").textContent = log.text || "Журнал пуст";
    $("log-cut").hidden = !log.cut;
    const text = $("log-text");
    text.scrollTop = text.scrollHeight;
  } catch (error) {
    $("log-text").textContent = error.message === "unauthorized" ? "" : error.message;
  }
}

export function initDialogs() {
  $("log-close").addEventListener("click", () => $("log-dialog").close());
  $("qr-close").addEventListener("click", () => $("qr-dialog").close());
}
