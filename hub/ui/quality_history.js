import { $, api, el, showError } from "./dom.js";
import { when } from "./format.js";
import { span } from "./quality_time.js";

// История проверок качества: график WER по прогонам, список прогонов (он же табличный
// вид графика) и сравнение двух прогонов по наборам и группам клипов.
const SVG = "http://www.w3.org/2000/svg";
const HEIGHT = 220;
const PAD = { top: 16, right: 16, bottom: 28, left: 52 };
const TICKS = 4;
// Разница меньше этой (в долях) считается «без изменений».
const SAME = 0.0005;
const rateText = new Intl.NumberFormat("ru-RU", { maximumFractionDigits: 1, minimumFractionDigits: 1 });
const dayText = new Intl.DateTimeFormat("ru-RU", { day: "numeric", month: "short" });

let runs = [];
let focused = null;

function pct(rate) {
  return rate == null ? "нет" : `${rateText.format(rate * 100)} %`;
}

function short(commit) {
  return commit ? commit.slice(0, 7) : "без коммита";
}

// Метрики графика: общий WER по текстам человека, по всем наборам и по каждому набору.
function metrics() {
  const list = [
    { key: "human", label: "WER по текстам человека", pick: (r) => r.totals.human?.wer.rate ?? null },
    { key: "all", label: "WER по всем наборам с текстом", pick: (r) => r.totals.all?.wer.rate ?? null },
  ];
  const seen = new Map();
  for (const run of runs) {
    for (const set of run.datasets) if (set.has_text && !seen.has(set.id)) seen.set(set.id, set.title);
  }
  for (const [id, title] of seen) {
    list.push({ key: `set:${id}`, label: `WER: ${title}`, pick: (r) => r.datasets.find((d) => d.id === id)?.wer.rate ?? null });
  }
  return list;
}

function svg(tag, attrs) {
  const node = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  return node;
}

// Верх оси: ближайшее «круглое» значение над максимумом, чтобы деления читались.
function niceTop(max) {
  if (max <= 0) return 0.01;
  const step = 10 ** Math.floor(Math.log10(max));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * step >= max * 1.1) return m * step;
  return 10 * step;
}

function drawChart() {
  const box = $("history-chart");
  const metric = metrics().find((m) => m.key === $("history-metric").value) ?? metrics()[0];
  $("history-chart-title").textContent = metric.label;
  const tip = $("history-tip");
  tip.hidden = true;
  const ordered = [...runs].reverse();
  const points = ordered.map((run, i) => ({ run, i, value: metric.pick(run) }));
  const known = points.filter((p) => p.value != null);
  box.replaceChildren();
  if (known.length === 0) {
    box.append(el("p", "empty", "Прогонов с такими итогами пока нет"));
    return;
  }
  const width = Math.max(box.clientWidth, 280);
  const plotW = width - PAD.left - PAD.right;
  const plotH = HEIGHT - PAD.top - PAD.bottom;
  const top = niceTop(Math.max(...known.map((p) => p.value)));
  const x = (i) => PAD.left + (ordered.length === 1 ? plotW / 2 : (i * plotW) / (ordered.length - 1));
  const y = (v) => PAD.top + plotH - (v / top) * plotH;
  const chart = svg("svg", { viewBox: `0 0 ${width} ${HEIGHT}`, width, height: HEIGHT, class: "chart-svg", tabindex: 0, role: "img" });
  const last = known.at(-1);
  chart.setAttribute("aria-label", `${metric.label}: ${known.length} прогонов, последний ${pct(last.value)}. Стрелки влево и вправо выбирают прогон.`);
  for (let t = 0; t <= TICKS; t += 1) {
    const value = (top * t) / TICKS;
    chart.append(svg("line", { x1: PAD.left, x2: width - PAD.right, y1: y(value), y2: y(value), class: t === 0 ? "chart-axis" : "chart-grid" }));
    const label = svg("text", { x: PAD.left - 8, y: y(value) + 4, class: "chart-label", "text-anchor": "end" });
    label.textContent = pct(value);
    chart.append(label);
  }
  for (const p of [points[0], points.at(-1)]) {
    const label = svg("text", { x: x(p.i), y: HEIGHT - 8, class: "chart-label", "text-anchor": p.i === 0 && ordered.length > 1 ? "start" : "end" });
    label.textContent = dayText.format(new Date(p.run.finished_at * 1000));
    chart.append(label);
  }
  // Пропущенные значения рвут линию, а не соединяются через пустоту.
  let path = "";
  let prev = null;
  for (const p of points) {
    if (p.value == null) {
      prev = null;
      continue;
    }
    path += `${prev === null ? "M" : "L"}${x(p.i).toFixed(1)},${y(p.value).toFixed(1)}`;
    prev = p;
  }
  chart.append(svg("path", { d: path, class: "chart-line" }));
  for (const p of known) chart.append(svg("circle", { cx: x(p.i), cy: y(p.value), r: 4, class: "chart-dot" }));
  const cross = svg("line", { x1: 0, x2: 0, y1: PAD.top, y2: PAD.top + plotH, class: "chart-cross", visibility: "hidden" });
  const ring = svg("circle", { cx: 0, cy: 0, r: 6, class: "chart-focus", visibility: "hidden" });
  chart.append(cross, ring);

  const show = (p) => {
    focused = p.i;
    cross.setAttribute("x1", x(p.i));
    cross.setAttribute("x2", x(p.i));
    cross.setAttribute("visibility", "visible");
    ring.setAttribute("cx", x(p.i));
    ring.setAttribute("cy", y(p.value));
    ring.setAttribute("visibility", "visible");
    tip.replaceChildren(
      el("strong", null, pct(p.value)),
      el("span", null, `${when(p.run.finished_at)}, ${short(p.run.commit)}`),
    );
    tip.hidden = false;
    const left = Math.min(Math.max(x(p.i) - tip.offsetWidth / 2, 0), width - tip.offsetWidth);
    tip.style.left = `${left}px`;
  };
  const hide = () => {
    cross.setAttribute("visibility", "hidden");
    ring.setAttribute("visibility", "hidden");
    tip.hidden = true;
  };
  const nearest = (px) =>
    known.reduce((best, p) => (Math.abs(x(p.i) - px) < Math.abs(x(best.i) - px) ? p : best), known[0]);
  chart.addEventListener("pointermove", (event) => {
    const rect = chart.getBoundingClientRect();
    show(nearest(((event.clientX - rect.left) * width) / rect.width));
  });
  chart.addEventListener("pointerleave", hide);
  chart.addEventListener("blur", hide);
  chart.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const at = known.findIndex((p) => p.i === focused);
    const next = at === -1 ? known.length - 1 : at + (event.key === "ArrowRight" ? 1 : -1);
    show(known[Math.min(Math.max(next, 0), known.length - 1)]);
  });
  box.append(chart);
}

function runRow(run) {
  const item = el("li", "row");
  const info = el("div");
  const parts = [
    `тексты человека ${pct(run.totals.human?.wer.rate)}`,
    `все наборы ${pct(run.totals.all?.wer.rate)}`,
    `наборов ${run.datasets.length}`,
    `звука ${span(run.audio_sec)}`,
  ];
  info.append(
    el("div", "row-title", `${when(run.finished_at)}, ${short(run.commit)}`),
    el("div", "row-meta", parts.join(", ")),
  );
  item.append(info);
  return item;
}

function option(value, text) {
  const node = el("option", null, text);
  node.value = value;
  return node;
}

// Разница в процентных пунктах со словом: цвет не единственный признак.
function change(before, after) {
  if (before == null || after == null) return { text: "нет данных", tone: "" };
  const diff = after - before;
  if (Math.abs(diff) < SAME) return { text: "без изменений", tone: "" };
  const points = rateText.format(Math.abs(diff) * 100);
  return diff < 0
    ? { text: `−${points} п. п., лучше`, tone: "better" }
    : { text: `+${points} п. п., хуже`, tone: "worse" };
}

function compareRow(name, before, after, sub) {
  const row = el("tr", sub ? "compare-sub" : null);
  const delta = change(before, after);
  const deltaCell = el("td", `num ${delta.tone}`.trim(), delta.text);
  row.append(el("td", null, name), el("td", "num", pct(before)), el("td", "num", pct(after)), deltaCell);
  return row;
}

function drawCompare() {
  const box = $("compare-table");
  const before = runs.find((r) => r.id === $("compare-old").value);
  const after = runs.find((r) => r.id === $("compare-new").value);
  box.replaceChildren();
  if (!before || !after) {
    box.append(el("p", "empty", "Для сравнения нужно хотя бы два прогона"));
    return;
  }
  if (before.models_version !== after.models_version) {
    box.append(el("p", "hint", `Модели разные: было ${before.models_version}, стало ${after.models_version}.`));
  }
  const table = el("table", "table report-table");
  const head = el("tr");
  for (const [text, numeric] of [["Набор и группа", false], ["Было", true], ["Стало", true], ["Разница", true]]) {
    const th = el("th", numeric ? "num" : null, text);
    th.scope = "col";
    head.append(th);
  }
  const thead = el("thead");
  thead.append(head);
  const body = el("tbody");
  body.append(
    compareRow("Тексты человека", before.totals.human?.wer.rate, after.totals.human?.wer.rate, false),
    compareRow("Все наборы с текстом", before.totals.all?.wer.rate, after.totals.all?.wer.rate, false),
  );
  const ids = [...new Set([...after.datasets, ...before.datasets].filter((d) => d.has_text).map((d) => d.id))];
  for (const id of ids) {
    const old = before.datasets.find((d) => d.id === id);
    const now = after.datasets.find((d) => d.id === id);
    body.append(compareRow((now ?? old).title, old?.wer.rate, now?.wer.rate, false));
    for (const group of (now ?? old).groups) {
      const label = group.label;
      const was = old?.groups.find((g) => g.label === label);
      const is = now?.groups.find((g) => g.label === label);
      body.append(compareRow(label, was?.wer.rate, is?.wer.rate, true));
    }
  }
  table.append(thead, body);
  const wrap = el("div", "table-wrap");
  wrap.append(table);
  box.append(wrap);
}

export async function loadHistory() {
  try {
    runs = await api("/api/quality/runs");
  } catch (error) {
    runs = [];
    showError("quality-error", error);
  }
  const metric = $("history-metric");
  const keep = metric.value;
  metric.replaceChildren(...metrics().map((m) => option(m.key, m.label)));
  if ([...metric.options].some((o) => o.value === keep)) metric.value = keep;
  metric.disabled = runs.length === 0;
  const list = $("history-runs");
  list.replaceChildren(...runs.map(runRow));
  if (runs.length === 0) list.append(el("li", "empty", "Прогонов в истории пока нет: они появятся после следующей проверки"));
  for (const [id, pick] of [["compare-old", 1], ["compare-new", 0]]) {
    const select = $(id);
    select.replaceChildren(...runs.map((r) => option(r.id, `${when(r.finished_at)}, ${short(r.commit)}`)));
    select.disabled = runs.length < 2;
    if (runs[pick]) select.value = runs[pick].id;
  }
  drawChart();
  drawCompare();
}

export function initHistory() {
  $("history-metric").addEventListener("change", drawChart);
  $("compare-old").addEventListener("change", drawCompare);
  $("compare-new").addEventListener("change", drawCompare);
  let resize = null;
  window.addEventListener("resize", () => {
    clearTimeout(resize);
    resize = setTimeout(() => runs.length > 0 && drawChart(), 150);
  });
}
