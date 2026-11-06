const dateFormat = new Intl.DateTimeFormat("ru-RU", { dateStyle: "medium", timeStyle: "short" });
const dayFormat = new Intl.DateTimeFormat("ru-RU", { day: "numeric", month: "short" });
const numberFormat = new Intl.NumberFormat("ru-RU", { maximumFractionDigits: 1 });
const percentFormat = new Intl.NumberFormat("ru-RU", { style: "percent", maximumFractionDigits: 0 });

export function when(unix) {
  return dateFormat.format(new Date(unix * 1000));
}

export function duration(seconds) {
  if (seconds === null || seconds === undefined || seconds < 0) return "";
  const s = Math.round(seconds);
  if (s < 60) return `${s} с`;
  if (s < 3600) return `${Math.floor(s / 60)} мин ${String(s % 60).padStart(2, "0")} с`;
  return `${Math.floor(s / 3600)} ч ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")} мин`;
}

export function size(bytes) {
  return `${numberFormat.format(bytes / 1048576)} МБ`;
}

export function percent(share) {
  return percentFormat.format(share);
}

export function count(n) {
  return numberFormat.format(n);
}

// Сутки режутся в поясе устройства: ключ дня собирается из местной даты.
export function dayKey(date) {
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${m}-${d}`;
}

export function dayLabel(date) {
  return dayFormat.format(date);
}

export function lastDays(n) {
  const days = [];
  const today = new Date();
  today.setHours(12, 0, 0, 0);
  for (let i = n - 1; i >= 0; i -= 1) {
    const day = new Date(today);
    day.setDate(today.getDate() - i);
    days.push(day);
  }
  return days;
}

export function median(values) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}
