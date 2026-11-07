import { $, api, badge, el, showError } from "./dom.js";
import { duration, when } from "./format.js";
import { onHide, onShow } from "./tabs.js";

const RUN = {
  running: ["running", "идёт"],
  success: ["ok", "прошли"],
  failure: ["bad", "упали"],
  cancelled: ["stopped", "отменены"],
  lost: ["stopped", "прервались"],
};
const STEP = {
  running: ["running", "идёт"],
  success: ["ok", "прошёл"],
  failure: ["bad", "упал"],
  lost: ["stopped", "прервался"],
};
const REFRESH_RUNNING_MS = 5000;
const REFRESH_IDLE_MS = 30000;

let runs = [];
let filter = "all";
let timer = null;
const openRuns = new Set();
const openSteps = new Map();

function runTook(run) {
  const end = run.finished_at ?? (run.status === "running" ? Math.floor(Date.now() / 1000) : null);
  return end ? duration(end - run.started_at) : "";
}

function stepTook(step) {
  const end = step.finished_at ?? (step.status === "running" ? Math.floor(Date.now() / 1000) : null);
  return end ? duration(end - step.started_at) : "";
}

async function loadStepLog(run, step, target) {
  target.textContent = "Загружаю…";
  try {
    const log = await api(`/api/ci/runs/${encodeURIComponent(run.id)}/steps/${step.n}/log`);
    target.textContent = (log.cut ? "Журнал длинный, показан его конец.\n\n" : "") + (log.text || "Журнал пуст");
    target.scrollTop = target.scrollHeight;
  } catch (error) {
    target.textContent = error.message === "unauthorized" ? "" : error.message;
  }
}

function renderSteps(run) {
  const list = el("ol", "steps");
  const opened = openSteps.get(run.id) ?? new Set();
  for (const step of run.steps) {
    const item = el("li", "step");
    const [tone, label] = STEP[step.status] ?? ["stopped", step.status];
    const head = el("button", "step-head");
    head.type = "button";
    head.setAttribute("aria-expanded", String(opened.has(step.n)));
    head.append(badge(tone, label), el("span", "step-title", step.title), el("span", "row-meta", stepTook(step)));
    const log = el("pre", "log step-log");
    log.hidden = !opened.has(step.n);
    if (!log.hidden) loadStepLog(run, step, log);
    head.addEventListener("click", () => {
      const open = log.hidden;
      log.hidden = !open;
      head.setAttribute("aria-expanded", String(open));
      if (open) {
        opened.add(step.n);
        loadStepLog(run, step, log);
      } else {
        opened.delete(step.n);
      }
      openSteps.set(run.id, opened);
    });
    item.append(head, log);
    list.append(item);
  }
  if (run.steps.length === 0) list.append(el("li", "empty", "Шаги ещё не начались"));
  return list;
}

// Остановить идущий прогон или запустить остановленный заново. Команду в GitHub отдаёт
// агент сборки; заново значит новая попытка того же коммита, а не продолжение.
const CONTROL = {
  running: ["cancel", "Остановить прогон", "button button-danger", "Останавливаю…", "Прогон остановлен"],
  cancelled: ["rerun", "Возобновить", "button button-secondary", "Запускаю заново…", "Прогон запущен заново"],
  lost: ["rerun", "Возобновить", "button button-secondary", "Запускаю заново…", "Прогон запущен заново"],
};

function renderControl(run) {
  const control = CONTROL[run.status];
  if (!control || !run.run_id) return null;
  const [action, label, style, pending, done] = control;
  const box = el("div", "run-control");
  const button = el("button", style, label);
  button.type = "button";
  const note = el("span", "row-meta");
  note.setAttribute("role", "status");
  button.addEventListener("click", async () => {
    button.disabled = true;
    note.textContent = pending;
    try {
      await api(`/api/ci/github/${encodeURIComponent(run.run_id)}/${action}`, { method: "POST" });
      note.textContent = done;
      setTimeout(load, 3000);
    } catch (error) {
      note.textContent = "";
      button.disabled = false;
      showError("tests-error", error);
    }
  });
  box.append(button, note);
  return box;
}

function renderRun(run) {
  const item = el("li", "run");
  const [tone, label] = RUN[run.status] ?? ["stopped", run.status];
  const head = el("button", "run-head");
  head.type = "button";
  const expanded = openRuns.has(run.id);
  head.setAttribute("aria-expanded", String(expanded));
  const text = el("div", "run-text");
  const meta = [run.branch, run.sha.slice(0, 7), run.actor, when(run.started_at), runTook(run)].filter(Boolean);
  text.append(el("div", "row-title", run.subject || run.sha.slice(0, 7)), el("div", "row-meta", meta.join(", ")));
  if (run.failed_step) text.append(el("div", "run-failed", `Упал шаг «${run.failed_step}»`));
  head.append(badge(tone, label), text);
  item.append(head);
  const control = renderControl(run);
  if (control) item.append(control);
  if (expanded) {
    const detail = el("div", "run-detail");
    detail.append(renderSteps(run));
    if (run.url) {
      const link = el("a", "button button-plain", "Открыть в GitHub");
      link.href = run.url;
      link.target = "_blank";
      link.rel = "noopener noreferrer";
      detail.append(link);
    }
    item.append(detail);
  }
  head.addEventListener("click", () => {
    if (openRuns.has(run.id)) openRuns.delete(run.id);
    else {
      openRuns.add(run.id);
      // Упавший шаг раскрывается сразу: ради его журнала прогон и открывают.
      const failed = run.steps.find((s) => s.status === "failure");
      if (failed && !openSteps.has(run.id)) openSteps.set(run.id, new Set([failed.n]));
    }
    render();
    item.querySelector(".run-head")?.focus();
  });
  return item;
}

function render() {
  const list = $("runs");
  const branch = $("branch").value;
  const visible = runs.filter(
    (r) => (filter === "all" || r.status === "failure") && (!branch || r.branch === branch),
  );
  const focusedId = document.activeElement?.closest?.(".run")?.dataset.id;
  list.replaceChildren();
  if (runs.length === 0) {
    list.append(
      el("li", "empty", "Прогонов пока нет. Они появятся после пуша в репозиторий, когда раннер на сервере зарегистрирован."),
    );
    return;
  }
  if (visible.length === 0) {
    list.append(el("li", "empty", "Под этот отбор прогонов нет"));
    return;
  }
  for (const run of visible) {
    const item = renderRun(run);
    item.dataset.id = run.id;
    list.append(item);
  }
  if (focusedId) list.querySelector(`[data-id="${CSS.escape(focusedId)}"] .run-head`)?.focus();
}

function fillBranches() {
  const select = $("branch");
  const chosen = select.value;
  const branches = [...new Set(runs.map((r) => r.branch).filter(Boolean))].sort();
  select.replaceChildren(el("option", null, "Все ветки"));
  select.firstChild.value = "";
  for (const name of branches) {
    const option = el("option", null, name);
    option.value = name;
    select.append(option);
  }
  select.value = branches.includes(chosen) ? chosen : "";
}

async function load() {
  try {
    runs = await api("/api/ci/runs");
    $("tests-error").textContent = "";
    fillBranches();
    render();
  } catch (error) {
    showError("tests-error", error);
  }
  schedule();
}

function schedule() {
  clearTimeout(timer);
  const busy = runs.some((r) => r.status === "running");
  timer = setTimeout(load, busy ? REFRESH_RUNNING_MS : REFRESH_IDLE_MS);
}

export function initTests() {
  for (const button of document.querySelectorAll("[data-filter]")) {
    button.addEventListener("click", () => {
      filter = button.dataset.filter;
      for (const b of document.querySelectorAll("[data-filter]")) {
        b.setAttribute("aria-pressed", String(b === button));
      }
      render();
    });
  }
  $("branch").addEventListener("change", render);
  onShow("tests", load);
  onHide("tests", () => clearTimeout(timer));
}
