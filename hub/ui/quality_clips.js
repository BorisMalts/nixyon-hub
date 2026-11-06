import { $, api, el, showError } from "./dom.js";

// Клипы последней проверки по наборам: отбор по метке и тексту делает хаб, по 50 за раз.
const TAG_LABELS = {
  errors: "с ошибками",
  empty: "пустой ответ",
  failed: "текст не получен",
  cut_start: "обрыв начала",
  cut_end: "обрыв конца",
  digits: "цифры",
};
const SEARCH_DELAY_MS = 350;
const seconds = new Intl.NumberFormat("ru-RU", { maximumFractionDigits: 1 });
const share = new Intl.NumberFormat("ru-RU", { style: "percent", maximumFractionDigits: 1 });
const whole = new Intl.NumberFormat("ru-RU");

let sets = [];
let shown = 0;
let request = 0;
let searchTimer = null;
let playing = null;
// Клипы текущего набора с пометкой «плохой эталон»: следующие проверки их не берут.
let marked = new Set();

function current() {
  return sets.find((s) => s.dataset === $("clips-set").value) ?? null;
}

function stopPlaying() {
  if (!playing) return;
  playing.audio.pause();
  playing.audio.remove();
  URL.revokeObjectURL(playing.url);
  playing = null;
}

// Звук скачивается целиком и играет из памяти: хаб не отдаёт файлы по кускам, а
// Safari без этого не проигрывает <audio> по адресу.
async function play(button, set, number) {
  stopPlaying();
  button.disabled = true;
  try {
    const response = await fetch(`/api/quality/audio/${encodeURIComponent(set)}/${number}`, {
      credentials: "same-origin",
    });
    if (response.status === 401) {
      window.location.replace("/login");
      return;
    }
    if (!response.ok) throw new Error("Звук не загрузился, попробуйте ещё раз");
    const url = URL.createObjectURL(await response.blob());
    const audio = el("audio", "clip-audio");
    audio.controls = true;
    audio.src = url;
    button.after(audio);
    playing = { audio, url };
    await audio.play().catch(() => {});
  } catch (error) {
    showError("quality-error", error);
  } finally {
    button.disabled = false;
  }
}

function diff(edits) {
  const line = el("p", "clip-diff");
  for (const [op, first, second] of edits) {
    if (op === "=") line.append(`${first} `);
    else if (op === "~") line.append(el("del", null, first), " ", el("ins", null, second), " ");
    else if (op === "-") line.append(el("del", null, first), " ");
    else if (op === "+") line.append(el("ins", null, first), " ");
  }
  return line;
}

async function toggleMark(button, set, clip, tag) {
  const on = !marked.has(clip.id);
  button.disabled = true;
  try {
    await api(`/api/quality/marks/${encodeURIComponent(set.dataset)}/${encodeURIComponent(clip.id)}`, {
      method: on ? "POST" : "DELETE",
    });
    if (on) marked.add(clip.id);
    else marked.delete(clip.id);
    showMark(button, tag, on);
  } catch (error) {
    showError("quality-error", error);
  } finally {
    button.disabled = false;
  }
}

function showMark(button, tag, on) {
  button.textContent = on ? "Снять пометку" : "Плохой эталон";
  button.setAttribute("aria-pressed", String(on));
  tag.hidden = !on;
}

function clipRow(clip, set) {
  const item = el("li", "clip");
  const head = el("div", "clip-head");
  const meta = [`${seconds.format(clip.duration_sec)} с`];
  if (set.has_text && clip.words > 0) {
    meta.unshift(`${whole.format(clip.word_errors)} из ${whole.format(clip.words)} слов, ${share.format(clip.word_errors / clip.words)}`);
  }
  if (clip.expected_speakers != null) meta.push(`голосов ${clip.speakers} из ${clip.expected_speakers}`);
  else if (clip.speakers > 1) meta.push(`голосов ${clip.speakers}`);
  head.append(el("span", "row-title", clip.id), el("span", "row-meta", meta.join(", ")));
  item.append(head);
  const tags = clip.tags.filter((t) => t !== "errors" && TAG_LABELS[t]);
  const list = el("div", "clip-tags");
  const markTag = el("span", "clip-tag clip-tag-marked", "плохой эталон, не входит в проверку");
  list.append(markTag);
  for (const tag of tags) list.append(el("span", "clip-tag", TAG_LABELS[tag]));
  item.append(list);
  if (clip.edits && clip.edits.length > 0) {
    item.append(diff(clip.edits));
  } else if (set.has_text) {
    item.append(el("p", "clip-text", `Эталон: ${clip.reference}`));
  }
  if (clip.hypothesis.trim() === "") {
    item.append(el("p", "clip-text", "Распознано: ничего"));
  } else if (!set.has_text) {
    item.append(el("p", "clip-text", `Распознано: ${clip.hypothesis}`));
  }
  if (clip.failure) item.append(el("p", "clip-failure", `Ошибка: ${clip.failure}`));
  const actions = el("div", "clip-actions");
  if (clip.audio != null) {
    const button = el("button", "button button-secondary", "Слушать");
    button.type = "button";
    button.setAttribute("aria-label", `Слушать клип ${clip.id}`);
    button.addEventListener("click", () => play(button, set.dataset, clip.audio));
    actions.append(button);
  }
  if (set.has_text) {
    const button = el("button", "button button-plain");
    button.type = "button";
    button.title = "Ошибка в эталоне набора, а не в распознавании: клип не войдёт в следующие проверки";
    showMark(button, markTag, marked.has(clip.id));
    button.addEventListener("click", () => toggleMark(button, set, clip, markTag));
    actions.append(button);
  } else {
    markTag.hidden = true;
  }
  if (actions.childElementCount > 0) item.append(actions);
  list.hidden = list.querySelectorAll(".clip-tag:not([hidden])").length === 0;
  return item;
}

async function load(more) {
  const set = current();
  const list = $("clips-list");
  const count = $("clips-count");
  const moreButton = $("clips-more");
  if (!set) {
    list.replaceChildren();
    count.textContent = "Клипов пока нет: появятся после следующей проверки";
    moreButton.hidden = true;
    $("clips-download").hidden = true;
    return;
  }
  $("clips-download").hidden = false;
  $("clips-download").href = `/api/quality/clips/${encodeURIComponent(set.dataset)}/file`;
  $("clips-legend").hidden = !set.has_text;
  if (!more) {
    stopPlaying();
    shown = 0;
    list.replaceChildren();
    count.textContent = "Ищу…";
    try {
      const marks = await api(`/api/quality/marks/${encodeURIComponent(set.dataset)}`);
      marked = new Set(marks.map((m) => m.clip));
    } catch (error) {
      marked = new Set();
      showError("quality-error", error);
    }
  }
  const params = new URLSearchParams({ offset: String(shown) });
  const tag = $("clips-tag").value;
  const text = $("clips-search").value.trim();
  if (tag) params.set("tag", tag);
  if (text) params.set("q", text);
  const mine = ++request;
  moreButton.disabled = true;
  try {
    const page = await api(`/api/quality/clips/${encodeURIComponent(set.dataset)}?${params}`);
    if (mine !== request) return;
    for (const clip of page.clips) list.append(clipRow(clip, set));
    shown += page.clips.length;
    count.textContent =
      page.total === 0 ? "Таких клипов нет" : `Показано ${whole.format(shown)} из ${whole.format(page.total)}`;
    moreButton.hidden = shown >= page.total;
  } catch (error) {
    if (mine === request) {
      count.textContent = "Клипы не загрузились";
      showError("quality-error", error);
    }
  } finally {
    moreButton.disabled = false;
  }
}

// Список наборов с клипами последнего отчёта; выбранный набор сохраняется, если он есть.
export async function loadClips() {
  const select = $("clips-set");
  const before = select.value;
  try {
    sets = await api("/api/quality/clip-sets");
  } catch (error) {
    sets = [];
    showError("quality-error", error);
  }
  select.replaceChildren(...sets.map((s) => {
    const option = el("option", null, s.title);
    option.value = s.dataset;
    return option;
  }));
  select.disabled = sets.length === 0;
  if (sets.some((s) => s.dataset === before)) select.value = before;
  await load(false);
}

export function initClips() {
  $("clips-set").addEventListener("change", () => load(false));
  $("clips-tag").addEventListener("change", () => load(false));
  $("clips-search").addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => load(false), SEARCH_DELAY_MS);
  });
  $("clips-more").addEventListener("click", () => load(true));
}

export function hideClips() {
  stopPlaying();
}
