import { el } from "./dom.js";

// Отчёт пишет код из репозитория Nixyon: из Markdown берутся только заголовки, абзацы
// и таблицы, и всё ставится на страницу текстом, без разбора HTML.

const NUMBER = /^\d/;

function cells(line) {
  const parts = [];
  let current = "";
  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i];
    if (ch === "\\" && line[i + 1] === "|") {
      current += "|";
      i += 1;
    } else if (ch === "|") {
      parts.push(current.trim());
      current = "";
    } else {
      current += ch;
    }
  }
  parts.push(current.trim());
  return parts.slice(1, -1);
}

function isSeparator(line) {
  return /^\|(\s*:?-+:?\s*\|)+\s*$/.test(line);
}

function cell(tag, text, numeric) {
  const node = el(tag, numeric ? "num" : null, text);
  if (tag === "th") node.scope = "col";
  return node;
}

function table(rows) {
  const wrap = el("div", "table-wrap");
  const [head, ...body] = rows;
  const tableNode = el("table", head.length > 3 ? "table report-table report-wide" : "table report-table");
  // Столбец числовой, если числа во всех его строках: тогда и заголовок справа.
  const numeric = head.map(
    (_, c) => body.length > 0 && body.every((row) => NUMBER.test(row[c] ?? "")),
  );
  const headRow = el("tr");
  headRow.append(...head.map((text, c) => cell("th", text, numeric[c])));
  const thead = el("thead");
  thead.append(headRow);
  const tbody = el("tbody");
  for (const row of body) {
    const tr = el("tr");
    tr.append(...row.map((text, c) => cell("td", text, numeric[c])));
    tbody.append(tr);
  }
  tableNode.append(thead, tbody);
  wrap.append(tableNode);
  return wrap;
}

export function renderReport(markdown) {
  const nodes = [];
  const lines = markdown.split("\n");
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (line.startsWith("|")) {
      const rows = [];
      while (i < lines.length && lines[i].startsWith("|")) {
        if (!isSeparator(lines[i])) rows.push(cells(lines[i]));
        i += 1;
      }
      i -= 1;
      if (rows.length > 0) nodes.push(table(rows));
    } else if (line.startsWith("### ")) {
      nodes.push(el("h4", "report-subheading", line.slice(4)));
    } else if (line.startsWith("## ")) {
      nodes.push(el("h3", "report-heading", line.slice(3)));
    } else if (line.startsWith("# ")) {
      nodes.push(el("h3", "report-title", line.slice(2)));
    } else if (line.trim() !== "") {
      nodes.push(el("p", "hint", line));
    }
  }
  return nodes;
}
