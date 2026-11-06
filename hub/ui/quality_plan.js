import { $, api, el, showError } from "./dom.js";
import { PRESETS, presetPlan } from "./quality_presets.js";
import { span } from "./quality_time.js";

// Во сколько раз быстрее реального времени идёт проверка, пока прошлого прогона нет:
// замер на сервере, 33 часа Common Voice за 38,5 минуты.
const DEFAULT_SPEED = 50;
// Агент останавливает проверку через 7 суток.
const TIME_LIMIT_SEC = 7 * 24 * 60 * 60;
// Сборка программы проверки перед прогоном.
const BUILD_SEC = 60;
const KIND = { single: "один голос", dialog: "разговор", mixed: "вперемешку" };
const REFERENCE = { human: "тексты писал человек", auto: "тексты машинные", unknown: "кто писал тексты, неизвестно" };

let rows = [];
let speeds = { overall: null, datasets: {} };
let onChange = () => {};

function speedOf(id) {
  return speeds.datasets?.[id] ?? speeds.overall ?? DEFAULT_SPEED;
}

// Ползунок в минутах, крайнее правое значение значит «весь набор».
function minutesOf(row) {
  const value = Number(row.range.value);
  return value >= Number(row.range.max) ? null : value;
}

function takenSeconds(row) {
  const minutes = minutesOf(row);
  return minutes === null ? row.set.audio_sec : Math.min(minutes * 60, row.set.audio_sec);
}

function describe(row) {
  const taken = takenSeconds(row);
  const amount = minutesOf(row) === null ? `весь набор, ${span(taken)}` : `${span(taken)} из ${span(row.set.audio_sec)}`;
  row.value.textContent = `${amount}, ≈ ${span(taken / speedOf(row.set.id))}`;
}

function render() {
  const chosen = rows.filter((r) => r.check.checked);
  for (const row of rows) {
    row.amount.hidden = !row.check.checked;
    describe(row);
  }
  const audio = chosen.reduce((sum, r) => sum + takenSeconds(r), 0);
  const work = chosen.reduce((sum, r) => sum + takenSeconds(r) / speedOf(r.set.id), 0);
  const total = $("quality-total");
  const meta = $("quality-total-meta");
  const tooLong = work + BUILD_SEC > TIME_LIMIT_SEC;
  if (chosen.length === 0) {
    total.textContent = "Ничего не выбрано";
    meta.textContent = "";
  } else {
    total.textContent = `Займёт около ${span(work + BUILD_SEC)}`;
    meta.textContent = tooLong
      ? `${span(audio)} звука: дольше 7 суток, проверка остановится по пределу. Уменьшите объём.`
      : `${span(audio)} звука из ${chosen.length} наборов, вместе со сборкой программы`;
  }
  $("quality-start").disabled = chosen.length === 0 || tooLong;
  onChange();
}

function datasetRow(set) {
  const item = el("li", "dataset");
  const id = `quality-set-${set.id}`;
  const check = el("input", "dataset-check");
  check.type = "checkbox";
  check.id = id;
  check.checked = set.default_selected !== false;
  const label = el("label", "dataset-label");
  label.htmlFor = id;
  const text = set.has_text ? (REFERENCE[set.reference] ?? REFERENCE.unknown) : "без текста, только голоса";
  const notes = [KIND[set.kind] ?? set.kind, text, `${set.clips.toLocaleString("ru-RU")} клипов`, span(set.audio_sec)];
  if (set.default_selected === false) notes.push("включается только вручную");
  label.append(el("span", "row-title", set.title), el("span", "row-meta", notes.join(", ")));
  const amount = el("div", "dataset-amount");
  const range = el("input", "dataset-range");
  range.type = "range";
  const totalMinutes = Math.max(1, Math.ceil(set.audio_sec / 60));
  range.min = "1";
  range.max = String(totalMinutes);
  range.step = "1";
  range.value = range.max;
  range.setAttribute("aria-label", `Сколько минут звука взять из набора «${set.title}»`);
  const value = el("span", "dataset-value");
  value.setAttribute("aria-live", "polite");
  amount.append(range, value);
  item.append(check, label, amount);
  const row = { set, check, range, value, amount };
  check.addEventListener("change", render);
  range.addEventListener("input", render);
  return { item, row };
}

// Готовая настройка ставит галочки и ползунки; запускает проверку человек.
function applyPreset(key) {
  const plan = presetPlan(key, rows.map((r) => r.set), speedOf);
  for (const row of rows) {
    row.check.checked = plan.has(row.set.id);
    const minutes = plan.get(row.set.id);
    row.range.value = minutes == null ? row.range.max : String(minutes);
  }
  render();
}

function drawPresets() {
  const box = $("quality-presets");
  box.replaceChildren(el("span", "presets-label", "Быстрая настройка"));
  for (const preset of PRESETS) {
    const button = el("button", "button button-secondary", preset.label);
    button.type = "button";
    button.disabled = rows.length === 0;
    button.addEventListener("click", () => applyPreset(preset.key));
    box.append(button);
  }
}

export async function loadPlan(changed) {
  onChange = changed;
  const list = $("quality-datasets");
  try {
    const view = await api("/api/quality/datasets");
    speeds = view.speeds ?? speeds;
    rows = [];
    list.replaceChildren();
    if (view.datasets.length === 0) {
      list.append(el("li", "empty", "На сервере пока нет наборов записей."));
    }
    for (const set of view.datasets) {
      const { item, row } = datasetRow(set);
      rows.push(row);
      list.append(item);
    }
    drawPresets();
    render();
  } catch (error) {
    list.replaceChildren(el("li", "empty", "Список наборов не загрузился"));
    showError("quality-error", error);
  }
}

// План для запуска: выбранные наборы и минуты из каждого (null: весь набор).
export function chosenPlan() {
  return rows.filter((r) => r.check.checked).map((r) => ({ dataset: r.set.id, minutes: minutesOf(r) }));
}
