// Готовые настройки проверки: какие наборы выбрать и сколько минут взять из каждого.
// Здесь только расчёт, без страницы; null в минутах значит «весь набор».

// «Быстро»: около 10 минут вместе со сборкой программы (она идёт около минуты).
const QUICK_WORK_SEC = 9 * 60;

export const PRESETS = [
  { key: "quick", label: "Быстро, около 10 мин" },
  { key: "release", label: "Перед релизом" },
  { key: "all_text", label: "Всё с текстом" },
];

// Тексты человека дают честный WER, а разговоры нужны, чтобы проверить голоса.
function forRelease(set) {
  return set.has_text && (set.reference === "human" || set.kind === "dialog" || set.kind === "mixed");
}

// sets: наборы со страницы; speedOf(id): во сколько раз быстрее реального времени идёт
// набор. Возвращает Map: id набора и минуты (null: весь набор); остальные не выбраны.
export function presetPlan(key, sets, speedOf) {
  const plan = new Map();
  if (key === "all_text") {
    for (const set of sets) if (set.has_text) plan.set(set.id, null);
  } else if (key === "release") {
    for (const set of sets) if (forRelease(set)) plan.set(set.id, null);
  } else if (key === "quick") {
    // Время работы делится между наборами поровну. Набор, который целиком успевает за
    // свою долю, берётся весь, а остаток его доли делится между остальными; из прочих
    // берётся столько звука, сколько они успевают, но не меньше минуты.
    const chosen = sets
      .filter((s) => s.has_text)
      .map((set) => ({ set, work: set.audio_sec / speedOf(set.id) }))
      .sort((a, b) => a.work - b.work);
    const minutes = new Map();
    let left = QUICK_WORK_SEC;
    chosen.forEach(({ set, work }, i) => {
      const share = left / (chosen.length - i);
      const whole = Math.max(1, Math.ceil(set.audio_sec / 60));
      const taken = Math.min(Math.max(1, Math.floor((share * speedOf(set.id)) / 60)), whole);
      if (work <= share || taken >= whole) {
        minutes.set(set.id, null);
        left -= work;
      } else {
        minutes.set(set.id, taken);
        left -= (taken * 60) / speedOf(set.id);
      }
    });
    for (const set of sets) if (minutes.has(set.id)) plan.set(set.id, minutes.get(set.id));
  }
  return plan;
}
