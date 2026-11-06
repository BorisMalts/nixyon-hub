import { $ } from "./dom.js";
import { onShow } from "./tabs.js";

let terminal = null;

// Цвета ярче стандартных xterm: на тёмном фоне текст должен читаться без усилий.
const TERMINAL_PALETTE = {
  black: "#3a3a3c",
  red: "#ff6961",
  green: "#4cd964",
  yellow: "#ffd60a",
  blue: "#4da3ff",
  magenta: "#da8fff",
  cyan: "#70d7ff",
  white: "#f5f5f7",
  brightBlack: "#aeaeb2",
  brightRed: "#ff8a80",
  brightGreen: "#7ee787",
  brightYellow: "#ffe066",
  brightBlue: "#82c0ff",
  brightMagenta: "#e5b0ff",
  brightCyan: "#a0e8ff",
  brightWhite: "#ffffff",
};

function cssToken(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function isExpanded() {
  return document.fullscreenElement === $("terminal-section") || $("terminal-section").classList.contains("is-expanded");
}

function renderExpanded() {
  const expanded = isExpanded();
  const label = expanded ? "Свернуть терминал" : "На весь экран";
  const button = $("terminal-expand");
  button.setAttribute("aria-label", label);
  button.title = label;
  $("icon-expand").hidden = expanded;
  $("icon-collapse").hidden = !expanded;
  // Раскрытие классом не вызывает resize окна, поэтому размер терминала пересчитывается здесь.
  if (terminal) requestAnimationFrame(terminal.resize);
}

function collapseTerminal() {
  const section = $("terminal-section");
  if (document.fullscreenElement === section) {
    document.exitFullscreen().catch(() => null);
  }
  section.classList.remove("is-expanded");
  document.body.classList.remove("no-scroll");
  renderExpanded();
}

function expandInPage() {
  $("terminal-section").classList.add("is-expanded");
  document.body.classList.add("no-scroll");
  renderExpanded();
}

function toggleExpanded() {
  if (isExpanded()) {
    collapseTerminal();
    return;
  }
  const section = $("terminal-section");
  if (document.fullscreenEnabled && section.requestFullscreen) {
    section.requestFullscreen().catch(expandInPage);
  } else {
    expandInPage();
  }
  terminal?.term.focus();
}

function openTerminal() {
  const box = $("terminal");
  box.hidden = false;
  const term = new window.Terminal({
    cursorBlink: true,
    fontFamily: cssToken("--mono"),
    fontSize: 14,
    // Тусклый цветной текст, например серые подсказки оболочки, xterm сам осветляет до этого контраста.
    minimumContrastRatio: 7,
    theme: {
      background: cssToken("--terminal-bg"),
      foreground: cssToken("--terminal-fg"),
      cursor: cssToken("--terminal-fg"),
      ...TERMINAL_PALETTE,
    },
  });
  const fit = new window.FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open($("terminal-screen"));
  fit.fit();
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${scheme}://${location.host}/api/terminal?cols=${term.cols}&rows=${term.rows}`);
  socket.binaryType = "arraybuffer";
  const encoder = new TextEncoder();
  socket.addEventListener("message", (event) => term.write(new Uint8Array(event.data)));
  socket.addEventListener("close", () => term.write("\r\n[терминал закрыт]\r\n"));
  term.onData((data) => {
    if (socket.readyState === WebSocket.OPEN) socket.send(encoder.encode(data));
  });
  const resize = () => {
    fit.fit();
    if (socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
    }
  };
  window.addEventListener("resize", resize);
  term.focus();
  terminal = { term, socket, resize };
  $("terminal-toggle").textContent = "Закрыть терминал";
  $("terminal-expand").hidden = false;
}

function closeTerminal() {
  if (!terminal) return;
  collapseTerminal();
  $("terminal-expand").hidden = true;
  window.removeEventListener("resize", terminal.resize);
  terminal.socket.close();
  terminal.term.dispose();
  terminal = null;
  $("terminal").hidden = true;
  $("terminal-toggle").textContent = "Открыть терминал";
}

export function initTerminal() {
  $("terminal-toggle").addEventListener("click", () => (terminal ? closeTerminal() : openTerminal()));
  $("terminal-expand").addEventListener("click", toggleExpanded);
  document.addEventListener("fullscreenchange", renderExpanded);
  // Пока вкладка скрыта, у терминала нет размера: пересчитываем его при возврате.
  onShow("terminal", () => {
    if (terminal) requestAnimationFrame(terminal.resize);
  });
}
