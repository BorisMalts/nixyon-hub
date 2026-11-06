// Длительности на странице проверки: в часах и минутах, секунды читать неудобно.
export function span(seconds) {
  const total = Math.max(0, Math.round(seconds / 60));
  if (seconds < 60) return "меньше минуты";
  const h = Math.floor(total / 60);
  const m = total % 60;
  if (h === 0) return `${m} мин`;
  if (h >= 48) {
    const d = Math.floor(h / 24);
    const rest = h % 24;
    return rest === 0 ? `${d} сут` : `${d} сут ${rest} ч`;
  }
  return m === 0 ? `${h} ч` : `${h} ч ${m} мин`;
}

export function percent(share) {
  return `${(Math.min(Math.max(share, 0), 1) * 100).toFixed(1).replace(".", ",")} %`;
}
