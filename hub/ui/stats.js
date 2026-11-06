import { $, api, el, showError } from "./dom.js";
import { count, dayKey, dayLabel, duration, lastDays, median, percent, size, when } from "./format.js";
import { onShow } from "./tabs.js";

const DAYS = 14;
const WEEK_SECONDS = 7 * 24 * 60 * 60;
const MONTH_SECONDS = 30 * 24 * 60 * 60;
const SVG = "http://www.w3.org/2000/svg";

let stats = null;

function svg(tag, attrs) {
  const node = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  return node;
}

function tile(label, value, note) {
  const node = el("div", "tile card");
  node.append(el("div", "tile-label", label), el("div", "tile-value", value), el("div", "row-meta", note));
  return node;
}

function recent(timings, seconds) {
  const since = Date.now() / 1000 - seconds;
  return timings.filter((t) => t.started_at >= since);
}

function renderTiles() {
  const box = $("tiles");
  box.replaceChildren();
  const builds = recent(stats.builds.filter((b) => b.ok), WEEK_SECONDS).map((b) => b.seconds);
  const tests = recent(stats.test_runs.filter((t) => t.ok), WEEK_SECONDS).map((t) => t.seconds);
  const month = recent(stats.test_runs, MONTH_SECONDS);
  const failed = month.filter((t) => !t.ok).length;
  const apk = stats.apk_sizes.at(-1);
  box.append(
    tile(
      "Сборка APK, медиана",
      builds.length ? duration(median(builds)) : "нет данных",
      builds.length ? `за 7 дней, худшая ${duration(Math.max(...builds))}` : "за 7 дней сборок не было",
    ),
    tile(
      "Прогон тестов, медиана",
      tests.length ? duration(median(tests)) : "нет данных",
      tests.length ? `за 7 дней, худший ${duration(Math.max(...tests))}` : "за 7 дней прогонов не было",
    ),
    tile(
      "Упавшие прогоны",
      month.length ? percent(failed / month.length) : "нет данных",
      month.length ? `${count(failed)} из ${count(month.length)} за 30 дней` : "за 30 дней прогонов не было",
    ),
    tile("Последний APK", apk ? size(apk.size) : "нет данных", apk ? when(apk.created_at) : "сборок ещё не было"),
  );
}

function byDay(timings) {
  const groups = new Map();
  for (const t of timings) {
    const key = dayKey(new Date(t.started_at * 1000));
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(t.seconds);
  }
  return lastDays(DAYS).map((day) => {
    const values = groups.get(dayKey(day)) ?? [];
    return { day, values, median: median(values), worst: values.length ? Math.max(...values) : null };
  });
}

function showTip(target, text) {
  const tip = $("tooltip");
  tip.textContent = text;
  tip.hidden = false;
  const box = target.getBoundingClientRect();
  const width = tip.offsetWidth;
  const left = Math.min(Math.max(8, box.left + box.width / 2 - width / 2), window.innerWidth - width - 8);
  tip.style.left = `${left}px`;
  tip.style.top = `${Math.max(8, box.top - tip.offsetHeight - 8)}px`;
}

function hideTip() {
  $("tooltip").hidden = true;
}

function dayChart(container, timings, noun) {
  container.replaceChildren();
  const days = byDay(timings);
  const width = Math.max(280, container.clientWidth);
  const height = 180;
  const top = 20;
  const bottom = 28;
  const plot = height - top - bottom;
  const maxValue = Math.max(1, ...days.map((d) => d.median ?? 0));
  const band = width / days.length;
  const barWidth = Math.min(28, band * 0.6);
  const chart = svg("svg", { viewBox: `0 0 ${width} ${height}`, width: "100%", height, role: "img" });
  const total = days.reduce((n, d) => n + d.values.length, 0);
  chart.setAttribute("aria-label", total ? `${noun} по дням, медиана за день` : `${noun}: данных пока нет`);
  chart.append(svg("line", { x1: 0, x2: width, y1: top + plot, y2: top + plot, class: "axis" }));
  if (total > 0) {
    const scale = svg("text", { x: 0, y: 12, class: "axis-label" });
    scale.textContent = duration(maxValue);
    chart.append(scale, svg("line", { x1: 0, x2: width, y1: top, y2: top, class: "grid" }));
  }
  const labelEvery = band < 36 ? 2 : 1;
  days.forEach((d, i) => {
    const x = i * band + band / 2;
    if ((days.length - 1 - i) % labelEvery === 0) {
      const label = svg("text", { x, y: height - 8, class: "axis-label", "text-anchor": "middle" });
      label.textContent = dayLabel(d.day);
      chart.append(label);
    }
    if (d.median === null) return;
    const h = Math.max(4, (d.median / maxValue) * plot);
    const y = top + plot - h;
    const r = Math.min(4, barWidth / 2, h);
    const left = x - barWidth / 2;
    const right = x + barWidth / 2;
    const base = top + plot;
    const bar = svg("path", {
      d: `M${left},${base} V${y + r} Q${left},${y} ${left + r},${y} H${right - r} Q${right},${y} ${right},${y + r} V${base} Z`,
      class: "bar",
    });
    const tip = `${dayLabel(d.day)}: медиана ${duration(d.median)}, худшее ${duration(d.worst)}, всего ${count(d.values.length)}`;
    const hit = svg("rect", { x: i * band, y: top, width: band, height: plot, class: "hit", tabindex: 0, role: "img" });
    hit.setAttribute("aria-label", tip);
    for (const [on, fn] of [
      ["mouseenter", () => showTip(hit, tip)],
      ["focus", () => showTip(hit, tip)],
      ["mouseleave", hideTip],
      ["blur", hideTip],
    ]) {
      hit.addEventListener(on, fn);
    }
    chart.append(bar, hit);
  });
  container.append(chart);
  if (total === 0) {
    container.append(el("p", "hint", "Данных пока нет"));
    return;
  }
  const details = el("details", "table-view");
  details.append(el("summary", null, "Таблицей"));
  const wrap = el("div", "table-wrap");
  const table = el("table", "table");
  const head = el("tr");
  for (const [text, num] of [["День", false], ["Медиана", true], ["Худшее", true], ["Всего", true]]) {
    const th = el("th", num ? "num" : null, text);
    th.scope = "col";
    head.append(th);
  }
  const thead = el("thead");
  thead.append(head);
  const tbody = el("tbody");
  for (const d of days.filter((x) => x.values.length)) {
    const row = el("tr");
    row.append(
      el("td", null, dayLabel(d.day)),
      el("td", "num", duration(d.median)),
      el("td", "num", duration(d.worst)),
      el("td", "num", count(d.values.length)),
    );
    tbody.append(row);
  }
  table.append(thead, tbody);
  wrap.append(table);
  details.append(wrap);
  container.append(details);
}

function renderSlowest() {
  const list = $("slowest");
  list.replaceChildren();
  if (stats.slowest_steps.length === 0) {
    list.append(el("li", "empty", "Появится после первых прогонов тестов"));
    return;
  }
  const max = Math.max(1, ...stats.slowest_steps.map((s) => s.median_seconds));
  for (const step of stats.slowest_steps) {
    const item = el("li", "bar-row");
    const track = el("div", "bar-track");
    const fill = el("div", "bar-fill");
    fill.style.width = `${Math.max(2, (step.median_seconds / max) * 100)}%`;
    track.append(fill);
    item.append(
      el("span", "bar-name", step.title),
      track,
      el("span", "bar-value", `${duration(step.median_seconds)}, худшее ${duration(step.max_seconds)}`),
    );
    list.append(item);
  }
}

function renderPeople() {
  const body = $("people").querySelector("tbody");
  body.replaceChildren();
  if (stats.people.length === 0) {
    const row = el("tr");
    const cell = el("td", "empty", "Запусков пока не было");
    cell.colSpan = 5;
    row.append(cell);
    body.append(row);
    return;
  }
  for (const person of stats.people) {
    const row = el("tr");
    row.append(
      el("td", "who-cell", person.who),
      el("td", "num", count(person.builds)),
      el("td", "num", count(person.pulls)),
      el("td", "num", count(person.test_runs)),
      el("td", "num", count(person.quality_runs)),
    );
    body.append(row);
  }
}

function render() {
  if (!stats) return;
  renderTiles();
  dayChart($("chart-builds"), stats.builds.filter((b) => b.ok), "Сборка APK");
  dayChart($("chart-tests"), stats.test_runs, "Прогон тестов");
  renderSlowest();
  renderPeople();
}

async function load() {
  try {
    stats = await api("/api/stats");
    $("stats-error").textContent = "";
    render();
  } catch (error) {
    showError("stats-error", error);
  }
}

export function initStats() {
  onShow("stats", load);
  let pending = null;
  window.addEventListener("resize", () => {
    cancelAnimationFrame(pending);
    pending = requestAnimationFrame(() => {
      if (!$("chart-builds").closest("[data-view]").hidden) render();
    });
  });
}
