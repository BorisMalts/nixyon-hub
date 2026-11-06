import { $ } from "./dom.js";

// Журнал проверки качества без лишнего: подготовка и сборка свёрнуты, ход проверки и
// WER по наборам обновляются в одной строке, предупреждения движка собраны отдельно.
const MAX_LINES = 5000;

// Что за строка. Подготовка кончается строкой cargo о запуске программы проверки или,
// в старых журналах, строкой о числе движков.
export function classify(line) {
  if (/^\s*Running `/.test(line)) return "start";
  if (/^движков \d+/.test(line)) return "engines";
  if (/^[\d,]+\s%\sготово/u.test(line)) return "progress";
  if (line.startsWith("WER по наборам:")) return "sets";
  if (line.includes("sherpa-onnx/csrc/")) return "engine";
  return "text";
}

let preparing = true;
let prepLines = 0;
let engineLines = 0;
const now = { progress: "", sets: "" };

const LOGS = ["quality-log", "quality-prep-log", "quality-engine-log"];

// Куда идёт строка (id журнала или null, если она только обновляет строку хода
// проверки). Счётчики меняются здесь, страница не трогается.
function route(line) {
  const kind = classify(line);
  if (kind === "engine") {
    engineLines += 1;
    return "quality-engine-log";
  }
  if (kind === "progress" || kind === "sets") {
    now[kind] = line;
    return null;
  }
  if (preparing && kind !== "engines") {
    prepLines += 1;
    if (kind === "start") preparing = false;
    return "quality-prep-log";
  }
  preparing = false;
  return "quality-log";
}

function showCounters() {
  $("quality-prep").hidden = prepLines === 0;
  $("quality-prep-count").textContent = String(prepLines);
  $("quality-engine").hidden = engineLines === 0;
  $("quality-engine-count").textContent = String(engineLines);
  $("quality-now").hidden = !now.progress && !now.sets;
  $("quality-now-progress").textContent = now.progress;
  $("quality-now-sets").textContent = now.sets;
}

// Строка идущей проверки: журнал прокручивается вниз, только если он уже был внизу.
function append(id, line) {
  const log = $(id);
  const nearBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  log.append(line + "\n");
  while (log.childNodes.length > MAX_LINES) log.removeChild(log.firstChild);
  if (nearBottom) log.scrollTop = log.scrollHeight;
}

export function resetLog() {
  preparing = true;
  prepLines = 0;
  engineLines = 0;
  now.progress = "";
  now.sets = "";
  for (const id of LOGS) $(id).replaceChildren();
  $("quality-prep").open = false;
  showCounters();
}

export function addLine(line) {
  const id = route(line);
  showCounters();
  if (id) append(id, line);
}

// Весь сохранённый журнал сразу после resetLog: каждый блок вставляется одним текстом,
// без замеров страницы на каждой строке (на тысяче строк она иначе подвисает).
export function addLines(lines) {
  const parts = Object.fromEntries(LOGS.map((id) => [id, []]));
  for (const line of lines) {
    const id = route(line);
    if (id) parts[id].push(line);
  }
  for (const id of LOGS) {
    const kept = parts[id].slice(-MAX_LINES);
    $(id).textContent = kept.length > 0 ? kept.join("\n") + "\n" : "";
  }
  showCounters();
  const log = $("quality-log");
  log.scrollTop = log.scrollHeight;
}

// Конец проверки. Если она упала ещё на подготовке, ошибка там: блок раскрывается.
export function endLog(failed) {
  if (failed && preparing) $("quality-prep").open = true;
}
