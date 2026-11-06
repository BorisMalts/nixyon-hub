import { $ } from "./dom.js";
import { percent, span } from "./quality_time.js";

// Строку «@progress <звука готово> <звука всего> <клипов готово> <клипов всего> <прошло>»
// печатает программа проверки не чаще раза в секунду; в журнал она не попадает.
const PREFIX = "@progress ";
const TICK_MS = 1000;

let last = null;
let ticker = null;

export function isProgress(line) {
  return line.startsWith(PREFIX);
}

export function parseProgress(line) {
  const [done, total, clips, allClips, elapsed] = line.slice(PREFIX.length).trim().split(/\s+/).map(Number);
  if (![done, total, clips, allClips, elapsed].every(Number.isFinite) || total <= 0) return null;
  return { done, total, clips, allClips, elapsed };
}

function draw() {
  if (!last) return;
  const share = Math.min(last.done / last.total, 1);
  const since = last.running ? (Date.now() - last.seenAt) / 1000 : 0;
  const elapsed = last.elapsed + since;
  $("quality-bar-fill").style.width = `${(share * 100).toFixed(1)}%`;
  $("quality-bar").setAttribute("aria-valuenow", (share * 100).toFixed(1));
  $("quality-percent").textContent = percent(share);
  let eta = "";
  if (share >= 1) {
    eta = `готово за ${span(elapsed)}`;
  } else if (share > 0) {
    const left = Math.max(0, (last.elapsed / share) * (1 - share) - since);
    eta = `${last.clips} из ${last.allClips} клипов, прошло ${span(elapsed)}, осталось около ${span(left)}`;
  } else {
    eta = `прошло ${span(elapsed)}`;
  }
  $("quality-eta").textContent = eta;
}

// Новая строка прогресса. running: проверка идёт, тогда оценка времени обновляется
// каждую секунду и между строками.
export function showProgress(progress, running) {
  last = { ...progress, running, seenAt: Date.now() };
  $("quality-progress").hidden = false;
  draw();
  clearInterval(ticker);
  ticker = running ? setInterval(draw, TICK_MS) : null;
}

export function hideProgress() {
  last = null;
  clearInterval(ticker);
  ticker = null;
  $("quality-progress").hidden = true;
}

export function stopTicking() {
  clearInterval(ticker);
  ticker = null;
}
