import { $, api, badge, el, showError } from "./dom.js";
import { duration, size, when } from "./format.js";
import { isProgress, parseProgress } from "./quality_progress.js";
import { percent, span } from "./quality_time.js";
import { renderReport } from "./report_view.js";
import { onHide, onShow } from "./tabs.js";

// Наборы, которые знает агент. Свободные он качает сам, остальные владелец кладёт на
// сервер вручную; без speechocean762 обучать не на чем.
const SETS = [
  { id: "speechocean762", title: "speechocean762", note: "дети и взрослые, английский, качается сам", required: true },
  { id: "samromur_children", title: "Samrómur Children", note: "дети 4–17 лет, исландский, качается сам" },
  { id: "samromur", title: "Samrómur", note: "взрослые той же платформы, качается сам" },
  { id: "childmandarin", title: "ChildMandarin", note: "дети 3–5 лет, только проверка, кладётся на сервер вручную" },
  { id: "sps_ru", title: "Common Voice RU", note: "взрослые, только проверка, кладётся на сервер вручную" },
];
const FILES = {
  "report.md": "Отчёт с таблицами",
  "summary.json": "Итоги в JSON",
  "child_adult_redimnet2.json": "Веса на ReDimNet2",
  "child_adult_age_gender_hidden.json": "Веса на audEERING",
};
const STATE = {
  queued: ["waiting", "в очереди"],
  running: ["running", "идёт"],
  succeeded: ["ok", "готово"],
  failed: ["bad", "ошибка"],
};
const POLL_EVERY_MS = 5000;
const MAX_LOG_LINES = 2000;
const SHOWN_RUNS = 30;

let socket = null;
let followingId = null;
let timer = null;
let shownKey = null;
let shownRun = null;

function renderStatus(job) {
  const box = $("training-status");
  box.replaceChildren();
  if (!job) {
    box.textContent = "Обучений ещё не было";
    return;
  }
  const [tone, label] = STATE[job.state] ?? ["stopped", job.state];
  const parts = [
    el("strong", null, `Обучение ${job.id}`),
    badge(tone, label),
    el("span", null, `запустил ${job.started_by}`),
    el("span", null, when(job.started_at)),
  ];
  if (job.finished_at) parts.push(el("span", null, duration(job.finished_at - job.started_at)));
  box.append(...parts);
}

// Строка хода от программы обучения: клипов готово, клипов всего, прошло секунд.
function showProgress(progress) {
  const share = Math.min(progress.clips / Math.max(progress.allClips, 1), 1);
  $("training-progress").hidden = false;
  $("training-bar-fill").style.width = `${(share * 100).toFixed(1)}%`;
  $("training-bar").setAttribute("aria-valuenow", (share * 100).toFixed(1));
  $("training-percent").textContent = percent(share);
  const left = share > 0 && share < 1 ? `, осталось около ${span((progress.elapsed / share) * (1 - share))}` : "";
  $("training-eta").textContent = `признаки: ${progress.clips} из ${progress.allClips} клипов, прошло ${span(progress.elapsed)}${left}`;
}

function resetLog() {
  $("training-log").textContent = "";
  $("training-progress").hidden = true;
}

function addLine(line) {
  if (isProgress(line)) {
    const progress = parseProgress(line);
    if (progress) showProgress(progress);
    return;
  }
  const log = $("training-log");
  const lines = log.textContent === "" ? [] : log.textContent.split("\n");
  lines.push(line);
  log.textContent = lines.slice(-MAX_LOG_LINES).join("\n");
  log.scrollTop = log.scrollHeight;
}

function stopFollowing() {
  if (socket) socket.close();
  socket = null;
  followingId = null;
}

function follow(jobId) {
  stopFollowing();
  shownKey = null;
  resetLog();
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${scheme}://${location.host}/api/jobs/follow`);
  socket = ws;
  followingId = jobId;
  ws.addEventListener("message", (event) => {
    const frame = JSON.parse(event.data);
    if (frame.type === "status" && frame.job?.kind === "training") renderStatus(frame.job);
    if (frame.type === "log" && followingId === jobId) addLine(frame.line);
    if (frame.type === "finished") {
      if (frame.job?.kind === "training") renderStatus(frame.job);
      stopFollowing();
      loadRuns();
    }
  });
  ws.addEventListener("close", () => {
    if (socket === ws) stopFollowing();
  });
}

async function showLastFinished() {
  const jobs = await api("/api/jobs/history");
  const last = jobs.find((job) => job.kind === "training");
  const key = last ? `${last.id}:${last.state}:${last.finished_at}` : "none";
  if (key === shownKey) return;
  shownKey = key;
  renderStatus(last ?? null);
  resetLog();
  if (!last) return;
  const saved = await api(`/api/jobs/${last.id}/log`);
  const lines = (saved.text || "").split("\n").filter((line) => line !== "");
  for (const line of lines.length > 0 ? lines : ["Журнал пуст"]) addLine(line);
}

async function refresh() {
  try {
    const status = await api("/api/jobs/status");
    const running = status.job?.kind === "training" && status.job.state === "running";
    const queued = (status.queue ?? []).find((job) => job.kind === "training");
    if (running) {
      if (followingId !== status.job.id || !socket) follow(status.job.id);
    } else if (queued) {
      renderStatus(queued);
    } else if (!socket) {
      await showLastFinished();
    }
  } catch (error) {
    showError("training-error", error);
  }
}

function fileRow(id, file) {
  const item = el("li", "row");
  const info = el("div");
  info.append(el("div", "row-title", FILES[file.name] ?? file.name), el("div", "row-meta", `${size(file.size)}, ${file.name}`));
  const download = el("a", "button button-secondary", "Скачать");
  download.href = `/api/training/${id}/${encodeURIComponent(file.name)}`;
  download.setAttribute("download", `nixyon-training-${id}-${file.name}`);
  const actions = el("div", "row-actions");
  actions.append(download);
  item.append(info, actions);
  return item;
}

async function showReport(id, files) {
  const box = $("training-report");
  if (!files.some((file) => file.name === "report.md")) {
    box.replaceChildren(el("p", "empty", "У этого обучения отчёта нет: оно не дошло до конца."));
    return;
  }
  const response = await fetch(`/api/training/${id}/report.md`, { credentials: "same-origin" });
  if (response.status === 401) {
    window.location.replace("/login");
    return;
  }
  if (!response.ok) {
    box.replaceChildren(el("p", "empty", "Отчёт не загрузился, обновите страницу"));
    return;
  }
  box.replaceChildren(...renderReport(await response.text()));
}

async function loadRuns() {
  const list = $("training-runs");
  try {
    const ids = (await api("/api/training")).slice(0, SHOWN_RUNS);
    list.replaceChildren();
    if (ids.length === 0) {
      list.append(el("li", "empty", "Обучений пока нет. Выберите наборы и нажмите «Обучить»."));
      $("training-report").replaceChildren(el("p", "empty", "Здесь появится отчёт последнего обучения."));
      return;
    }
    for (const [i, id] of ids.entries()) {
      const files = await api(`/api/training/${id}`);
      if (i === 0 && shownRun !== `${id}:${files.length}`) {
        shownRun = `${id}:${files.length}`;
        await showReport(id, files);
      }
      const group = el("li", "row training-run");
      group.append(el("div", "row-title", `Обучение ${id}`));
      const rows = el("ul", "rows");
      if (files.length === 0) rows.append(el("li", "empty", "Файлов нет"));
      for (const file of files) rows.append(fileRow(id, file));
      group.append(rows);
      list.append(group);
    }
  } catch (error) {
    showError("training-error", error);
  }
}

function chosenSets() {
  return SETS.filter((set) => $(`training-set-${set.id}`).checked).map((set) => set.id);
}

function renderSets() {
  const list = $("training-sets");
  list.replaceChildren();
  for (const set of SETS) {
    const item = el("li", "dataset");
    const check = el("input", "dataset-check");
    check.type = "checkbox";
    check.id = `training-set-${set.id}`;
    check.checked = set.required || set.id.startsWith("samromur");
    check.disabled = Boolean(set.required);
    const label = el("label", "dataset-label");
    label.htmlFor = check.id;
    const note = set.required ? `${set.note}, нужен всегда` : set.note;
    label.append(el("span", "row-title", set.title), el("span", "row-meta", note));
    item.append(check, label);
    list.append(item);
  }
}

async function start() {
  $("training-error").textContent = "";
  try {
    const job = await api("/api/jobs/training", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ sets: chosenSets() }),
    });
    if (job.state === "running") follow(job.id);
    else renderStatus(job);
  } catch (error) {
    showError("training-error", error);
  }
}

function poll() {
  if (document.visibilityState === "visible") refresh();
  timer = setTimeout(poll, POLL_EVERY_MS);
}

export function initTraining() {
  renderSets();
  $("training-start").addEventListener("click", start);
  onShow("training", () => {
    refresh();
    loadRuns();
    timer = setTimeout(poll, POLL_EVERY_MS);
  });
  onHide("training", () => {
    clearTimeout(timer);
    timer = null;
    stopFollowing();
  });
}
