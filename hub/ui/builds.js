import { $, api, badge, el, showError, showLog } from "./dom.js";
import { duration, size, when } from "./format.js";
import { onShow } from "./tabs.js";

const KIND = {
  pull: "Обновление кода",
  build: "Сборка APK",
  quality: "Проверка качества",
  live_eval: "Прогон живых записей",
  training: "Обучение",
};
const STATE = {
  queued: ["waiting", "в очереди"],
  running: ["running", "идёт"],
  succeeded: ["ok", "готово"],
  failed: ["bad", "ошибка"],
};
const MAX_LOG_LINES = 5000;
const HISTORY_PAGE = 20;
const POLL_EVERY_MS = 5000;

let follower = null;
let following = null;
let historyShown = HISTORY_PAGE;

function stateBadge(state) {
  const [tone, label] = STATE[state] ?? ["stopped", state];
  return badge(tone, label);
}

function renderStatus(job, queue) {
  const box = $("status");
  box.replaceChildren();
  if (!job) {
    box.textContent = "Заданий ещё не было";
  } else {
    const parts = [
      el("strong", null, KIND[job.kind] ?? job.kind),
      stateBadge(job.state),
      el("span", null, `запустил ${job.started_by}`),
      el("span", null, when(job.started_at)),
    ];
    if (job.finished_at) parts.push(el("span", null, duration(job.finished_at - job.started_at)));
    box.append(...parts);
  }
  if (queue) {
    const line = $("queue");
    line.hidden = queue.length === 0;
    line.textContent = queue.length
      ? `В очереди: ${queue.map((q) => `${KIND[q.kind] ?? q.kind} (${q.started_by})`).join(", ")}`
      : "";
  }
}

function appendLog(line) {
  const log = $("log");
  const nearBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  log.append(line + "\n");
  while (log.childNodes.length > MAX_LOG_LINES) log.removeChild(log.firstChild);
  if (nearBottom) log.scrollTop = log.scrollHeight;
}

// Агент закрывает поток, когда задание закончилось. Следующее из очереди начнётся само,
// поэтому страница опрашивает состояние и снова подключается к идущему заданию.
function follow() {
  if (follower) follower.close();
  $("log").replaceChildren();
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${scheme}://${location.host}/api/jobs/follow`);
  follower = socket;
  socket.addEventListener("message", (event) => {
    const frame = JSON.parse(event.data);
    if (frame.type === "status") {
      following = frame.job?.state === "running" ? frame.job.id : null;
      renderStatus(frame.job, frame.queue);
    }
    if (frame.type === "log") appendLog(frame.line);
    if (frame.type === "finished") {
      following = null;
      renderStatus(frame.job);
      loadBuilds();
      loadHistory();
    }
  });
  socket.addEventListener("close", () => {
    if (follower === socket) follower = null;
  });
}

async function poll() {
  if (document.visibilityState === "visible") {
    try {
      const status = await api("/api/jobs/status");
      const running = status.job?.state === "running";
      if (running && (following !== status.job.id || !follower)) follow();
      else renderStatus(status.job, status.queue);
    } catch (error) {
      showError("action-error", error);
    }
  }
  setTimeout(poll, POLL_EVERY_MS);
}

async function loadBuilds() {
  const list = $("builds");
  try {
    const builds = await api("/api/builds");
    list.replaceChildren();
    if (builds.length === 0) {
      list.append(el("li", "empty", "Готовых APK пока нет. Нажмите «Собрать APK»."));
      return;
    }
    for (const build of builds) {
      const item = el("li", "row");
      const info = el("div");
      info.append(
        el("div", "row-title", build.subject || build.commit),
        el("div", "row-meta", `${when(build.created_at)}, ${build.commit}, ${size(build.size)}, ${build.built_by}`),
      );
      const actions = el("div", "row-actions");
      const download = el("a", "button button-primary", "Скачать");
      download.href = `/api/builds/${encodeURIComponent(build.id)}/apk`;
      download.setAttribute("download", build.file);
      const qr = el("button", "button button-secondary", "QR-код");
      qr.type = "button";
      qr.addEventListener("click", () => {
        $("qr-image").src = `/api/builds/${encodeURIComponent(build.id)}/qr.svg`;
        $("qr-dialog").showModal();
      });
      actions.append(download, qr);
      item.append(info, actions);
      list.append(item);
    }
  } catch (error) {
    showError("action-error", error);
  }
}

async function loadHistory() {
  const list = $("history");
  try {
    const jobs = await api("/api/jobs/history");
    list.replaceChildren();
    if (jobs.length === 0) {
      list.append(el("li", "empty", "Здесь появятся задания с их журналами."));
      return;
    }
    for (const job of jobs.slice(0, historyShown)) {
      const item = el("li", "row");
      const info = el("div");
      const title = el("div", "row-title");
      title.append(el("span", null, `${KIND[job.kind] ?? job.kind} `), stateBadge(job.state));
      const took = job.finished_at ? `, ${duration(job.finished_at - job.started_at)}` : "";
      info.append(title, el("div", "row-meta", `${when(job.started_at)}${took}, ${job.started_by}`));
      const open = el("button", "button button-secondary", "Журнал");
      open.type = "button";
      open.addEventListener("click", () =>
        showLog(`${KIND[job.kind] ?? job.kind}, ${when(job.started_at)}`, `/api/jobs/${job.id}/log`),
      );
      const actions = el("div", "row-actions");
      actions.append(open);
      item.append(info, actions);
      list.append(item);
    }
    if (jobs.length > historyShown) {
      const more = el("button", "button button-plain more", "Показать ещё");
      more.type = "button";
      more.addEventListener("click", () => {
        historyShown += HISTORY_PAGE;
        loadHistory();
      });
      const item = el("li", "row row-more");
      item.append(more);
      list.append(item);
    }
  } catch (error) {
    showError("action-error", error);
  }
}

async function start(kind) {
  $("action-error").textContent = "";
  try {
    const job = await api(`/api/jobs/${kind}`, { method: "POST" });
    if (job.state === "running") {
      follow();
      return;
    }
    const status = await api("/api/jobs/status");
    renderStatus(status.job, status.queue);
  } catch (error) {
    showError("action-error", error);
  }
}

export function initBuilds() {
  $("build").addEventListener("click", () => start("build"));
  $("pull").addEventListener("click", () => start("pull"));
  onShow("builds", () => {
    loadBuilds();
    loadHistory();
  });
  follow();
  setTimeout(poll, POLL_EVERY_MS);
}
