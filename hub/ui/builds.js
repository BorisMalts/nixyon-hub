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

// Одна сборка публикует две APK: полную с моделями и лёгкую без них. У лёгкой id полной
// с окончанием «-lite», а заголовок с приставкой «Без моделей: ». Старые сборки без пары
// показываются одной полной.
const LITE_SUFFIX = "-lite";
const LITE_PREFIX = "Без моделей: ";
const WEIGHT = {
  full: ["heavy", "Тяжёлая"],
  lite: ["light", "Лёгкая"],
};

function groupBuilds(builds) {
  const groups = new Map();
  for (const build of builds) {
    const lite = build.id.endsWith(LITE_SUFFIX);
    const key = lite ? build.id.slice(0, -LITE_SUFFIX.length) : build.id;
    const group = groups.get(key) ?? { key, full: null, lite: null };
    group[lite ? "lite" : "full"] = build;
    groups.set(key, group);
  }
  // Номер сборки: по порядку запуска среди показанных, самая ранняя получает 1.
  const started = (g) => (g.full ?? g.lite).created_at;
  const ordered = [...groups.values()].sort((a, b) => started(a) - started(b) || a.key.localeCompare(b.key));
  ordered.forEach((group, index) => {
    group.number = index + 1;
  });
  return ordered.reverse();
}

function weightTag(kind) {
  const [tone, label] = WEIGHT[kind];
  const tag = el("span", "tag", label);
  tag.dataset.weight = tone;
  return tag;
}

function variantRow(kind, build) {
  const row = el("li", "build-variant");
  const info = el("div", "build-variant-info");
  info.append(weightTag(kind), el("span", "row-meta", size(build.size)));
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
  row.append(info, actions);
  return row;
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
    for (const group of groupBuilds(builds)) {
      const main = group.full ?? group.lite;
      const subject = group.full?.subject || group.lite?.subject.replace(LITE_PREFIX, "") || main.commit;
      const item = el("li", "row build-group");
      const head = el("div");
      const title = el("div", "row-title");
      title.append(el("span", "build-number", `№ ${group.number}`), el("span", null, subject));
      head.append(title, el("div", "row-meta", `${when(main.created_at)}, ${main.commit}, ${main.built_by}`));
      const variants = el("ul", "build-variants");
      if (group.full) variants.append(variantRow("full", group.full));
      if (group.lite) variants.append(variantRow("lite", group.lite));
      item.append(head, variants);
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
