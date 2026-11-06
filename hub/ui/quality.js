import { $, api, badge, el, showError } from "./dom.js";
import { duration, size, when } from "./format.js";
import { hideClips, initClips, loadClips } from "./quality_clips.js";
import { initHistory, loadHistory } from "./quality_history.js";
import { addLine, addLines, endLog, resetLog } from "./quality_log.js";
import { chosenPlan, loadPlan } from "./quality_plan.js";
import { hideProgress, isProgress, parseProgress, showProgress, stopTicking } from "./quality_progress.js";
import { renderReport } from "./report_view.js";
import { onHide, onShow } from "./tabs.js";

const FILES = {
  "report.md": "Отчёт с таблицами",
  "summary.json": "Итоги в JSON",
  "clips.json": "Каждый клип в JSON",
};
const STATE = {
  queued: ["waiting", "в очереди"],
  running: ["running", "идёт"],
  succeeded: ["ok", "готово"],
  failed: ["bad", "ошибка"],
};
const POLL_EVERY_MS = 5000;

let socket = null;
let followingId = null;
let timer = null;
// Какая законченная проверка уже нарисована: без изменений журнал заново не строится.
let shownKey = null;

function renderStatus(job) {
  const box = $("quality-status");
  box.replaceChildren();
  if (!job) {
    box.textContent = "Проверок ещё не было";
    return;
  }
  const [tone, label] = STATE[job.state] ?? ["stopped", job.state];
  const parts = [
    el("strong", null, "Проверка качества"),
    badge(tone, label),
    el("span", null, `запустил ${job.started_by}`),
    el("span", null, when(job.started_at)),
  ];
  if (job.finished_at) parts.push(el("span", null, duration(job.finished_at - job.started_at)));
  box.append(...parts);
}

// Строки прогресса рисуют полосу, а в журнал не идут.
function appendLog(line, running) {
  if (isProgress(line)) {
    const progress = parseProgress(line);
    if (progress) showProgress(progress, running);
    return;
  }
  addLine(line);
}

function stopFollowing() {
  if (socket) socket.close();
  socket = null;
  followingId = null;
  stopTicking();
}

// Агент отдаёт журнал только текущего задания; страница слушает его, пока идёт проверка
// качества, а после конца берёт сохранённый журнал из истории.
function follow(jobId) {
  stopFollowing();
  shownKey = null;
  resetLog();
  hideProgress();
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${scheme}://${location.host}/api/jobs/follow`);
  socket = ws;
  followingId = jobId;
  ws.addEventListener("message", (event) => {
    const frame = JSON.parse(event.data);
    if (frame.type === "status" && frame.job?.kind === "quality") renderStatus(frame.job);
    if (frame.type === "log" && followingId === jobId) appendLog(frame.line, true);
    if (frame.type === "finished") {
      if (frame.job?.kind === "quality") {
        renderStatus(frame.job);
        endLog(frame.job.state === "failed");
      }
      stopFollowing();
      loadReport();
      loadClips();
      loadHistory();
    }
  });
  ws.addEventListener("close", () => {
    if (socket === ws) stopFollowing();
  });
}

async function showLastFinished() {
  const jobs = await api("/api/jobs/history");
  const last = jobs.find((job) => job.kind === "quality");
  const key = last ? `${last.id}:${last.state}:${last.finished_at}` : "none";
  if (key === shownKey) return;
  shownKey = key;
  renderStatus(last ?? null);
  hideProgress();
  resetLog();
  if (!last) return;
  const saved = await api(`/api/jobs/${last.id}/log`);
  const lines = (saved.text || "").split("\n");
  const progress = lines.filter(isProgress).map(parseProgress).filter(Boolean).at(-1);
  if (progress) showProgress(progress, false);
  const text = lines.filter((line) => !isProgress(line) && line !== "");
  addLines(text.length > 0 ? text : ["Журнал пуст"]);
  endLog(last.state === "failed");
}

async function refresh() {
  try {
    const status = await api("/api/jobs/status");
    const running = status.job?.kind === "quality" && status.job.state === "running";
    const queued = (status.queue ?? []).find((job) => job.kind === "quality");
    if (running) {
      if (followingId !== status.job.id || !socket) follow(status.job.id);
    } else if (queued) {
      renderStatus(queued);
    } else if (!socket) {
      await showLastFinished();
    }
  } catch (error) {
    showError("quality-error", error);
  }
}

async function loadFiles() {
  const list = $("quality-files");
  const files = await api("/api/quality");
  list.replaceChildren();
  if (files.length === 0) {
    list.append(el("li", "empty", "Отчёта пока нет. Нажмите «Проверить качество»."));
    return files;
  }
  for (const file of files) {
    const item = el("li", "row");
    const info = el("div");
    info.append(
      el("div", "row-title", FILES[file.name] ?? file.name),
      el("div", "row-meta", `${when(file.modified_at)}, ${size(file.size)}, ${file.name}`),
    );
    const download = el("a", "button button-primary", "Скачать");
    download.href = `/api/quality/${encodeURIComponent(file.name)}`;
    download.setAttribute("download", `nixyon-quality-${file.name}`);
    const actions = el("div", "row-actions");
    actions.append(download);
    item.append(info, actions);
    list.append(item);
  }
  return files;
}

async function loadReport() {
  const box = $("quality-report");
  try {
    const files = await loadFiles();
    if (!files.some((file) => file.name === "report.md")) {
      box.replaceChildren(el("p", "empty", "Здесь появится отчёт последней проверки."));
      return;
    }
    const response = await fetch("/api/quality/report.md", { credentials: "same-origin" });
    if (response.status === 401) {
      window.location.replace("/login");
      return;
    }
    if (!response.ok) {
      box.replaceChildren(el("p", "empty", "Отчёт не загрузился, обновите страницу"));
      return;
    }
    box.replaceChildren(...renderReport(await response.text()));
  } catch (error) {
    showError("quality-error", error);
  }
}

async function start() {
  $("quality-error").textContent = "";
  try {
    const job = await api("/api/jobs/quality", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ plan: chosenPlan() }),
    });
    if (job.state === "running") follow(job.id);
    else renderStatus(job);
  } catch (error) {
    showError("quality-error", error);
  }
}

function poll() {
  if (document.visibilityState === "visible") refresh();
  timer = setTimeout(poll, POLL_EVERY_MS);
}

export function initQuality() {
  $("quality-start").addEventListener("click", start);
  initClips();
  initHistory();
  onShow("quality", () => {
    refresh();
    loadPlan(() => {});
    loadReport();
    loadClips();
    loadHistory();
    timer = setTimeout(poll, POLL_EVERY_MS);
  });
  onHide("quality", () => {
    clearTimeout(timer);
    timer = null;
    stopFollowing();
    hideClips();
  });
}
