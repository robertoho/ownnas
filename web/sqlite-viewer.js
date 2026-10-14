// Read-only SQLite viewer for the preview panel: tables and views, their rows a page at a
// time (sorted and filtered on the server), their schema, and SELECT queries.

const PAGE = 100;

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === false || value === undefined || value === null) continue;
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : String(child));
  }
  return node;
}

const number = (n) => Number(n).toLocaleString();

function formatBytes(n) {
  if (n < 1024) return `${n} byte${n === 1 ? "" : "s"}`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function mountSqliteViewer(container, { path, api }) {
  let destroyed = false;
  let tables = [];
  let table = null;
  let mode = "data";
  let page = { offset: 0, order: "", desc: false, filter: "" };
  let filterTimer = 0;
  let request = 0;

  const picker = el("select", { class: "sqlite-picker", "aria-label": "Table" });
  const modes = el("div", { class: "sqlite-modes", role: "tablist" });
  const modeButton = (name, label) => el("button", { type: "button", role: "tab", "data-mode": name, onclick: () => setMode(name) }, label);
  modes.append(modeButton("data", "Data"), modeButton("schema", "Schema"), modeButton("sql", "SQL"));
  const status = el("span", { class: "sqlite-status", role: "status" });
  const toolbar = el("div", { class: "sqlite-toolbar" }, picker, modes, status);
  const body = el("div", { class: "sqlite-body" });
  const root = el("section", { class: "sqlite-viewer" }, toolbar, body);
  container.replaceChildren(root);

  // Kept between visits to the SQL tab.
  const editor = el("textarea", { class: "sqlite-sql", spellcheck: "false", rows: "5", "aria-label": "SQL query" });
  const run = el("button", { type: "button", class: "primary", onclick: () => runQuery() }, "Run");
  const results = el("div", { class: "sqlite-results" });
  editor.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      runQuery();
    }
  });

  picker.addEventListener("change", () => {
    table = tables.find((item) => item.name === picker.value) || null;
    page = { offset: 0, order: "", desc: false, filter: "" };
    if (mode === "sql") editor.value = defaultQuery();
    render();
  });

  function defaultQuery() {
    return table ? `SELECT * FROM "${table.name.replace(/"/g, '""')}" LIMIT 100;` : "SELECT sqlite_version();";
  }

  function setMode(next) {
    mode = next;
    if (mode === "sql" && !editor.value) editor.value = defaultQuery();
    render();
  }

  function setStatus(text) {
    status.textContent = text;
  }

  function message(text) {
    body.replaceChildren(el("p", { class: "muted sqlite-message" }, text));
  }

  function valueUrl(rowid, column) {
    const id = typeof rowid === "object" && rowid !== null ? rowid.int : rowid;
    return `/api/sqlite/value?${new URLSearchParams({ path, table: table.name, rowid: String(id), column })}`;
  }

  /// One cell: NULL, a number or text (cut short when long), or a blob's size and first bytes.
  function cellContent(value, rowid, column) {
    if (value === null) return el("span", { class: "sqlite-null" }, "NULL");
    if (typeof value !== "object") return String(value);
    if (value.int !== undefined) return value.int;
    if (value.real !== undefined) return value.real;
    const whole = rowid !== undefined && rowid !== null && column;
    // Long text and blobs lead with their size (a link to the whole value), so it stays in view.
    if (value.text !== undefined) {
      const size = `${number(value.length)} chars`;
      return el("span", {},
        whole
          ? el("a", { href: valueUrl(rowid, column), target: "_blank", rel: "noopener", class: "sqlite-badge sqlite-link", title: "Open the whole text" }, `${size} ↗`)
          : el("span", { class: "sqlite-badge" }, size),
        ` ${value.text}…`);
    }
    if (value.blob !== undefined) {
      const hex = value.hex.slice(0, 16).replace(/(..)/g, "$1 ").trim();
      const label = `BLOB ${formatBytes(value.blob)}`;
      return el("span", { class: "sqlite-blob" },
        whole && value.blob
          ? el("a", { href: valueUrl(rowid, column), download: "", class: "sqlite-badge sqlite-link", title: "Download" }, `${label} ↓`)
          : el("span", { class: "sqlite-badge" }, label),
        hex ? ` ${hex}${value.blob > 8 ? " …" : ""}` : "");
    }
    return JSON.stringify(value);
  }

  function grid(columns, rows, { sortable = false, rowids = false } = {}) {
    const head = el("tr", {}, columns.map((column) => {
      const sorted = sortable && page.order === column.name;
      const label = el("span", {}, column.name, column.pk ? el("span", { class: "sqlite-key", title: "Primary key" }, " 🔑") : null);
      const th = el("th", {
        scope: "col",
        title: [column.type, sortable ? "Click to sort" : ""].filter(Boolean).join(" · ") || null,
        "aria-sort": sorted ? (page.desc ? "descending" : "ascending") : null,
      });
      if (sortable) {
        th.append(el("button", { type: "button", class: "sqlite-sort", onclick: () => sortBy(column.name) }, label, sorted ? (page.desc ? " ▼" : " ▲") : ""));
      } else {
        th.append(label);
      }
      if (column.type) th.append(el("span", { class: "sqlite-type" }, column.type));
      return th;
    }));
    const lines = rows.map((row) => el("tr", {}, row.values.map((value, index) => {
      const content = cellContent(value, rowids ? row.rowid : undefined, columns[index].name);
      const full = typeof value === "string" ? value : value && value.text !== undefined ? `${value.text}…` : null;
      return el("td", { class: typeof value === "number" ? "sqlite-number" : null, title: full && full.length > 30 ? full : null }, content);
    })));
    return el("div", { class: "sqlite-grid-wrap" }, el("table", { class: "sqlite-grid" }, el("thead", {}, head), el("tbody", {}, lines)));
  }

  function sortBy(column) {
    if (page.order !== column) page = { ...page, order: column, desc: false };
    else if (!page.desc) page = { ...page, desc: true };
    else page = { ...page, order: "", desc: false };
    page.offset = 0;
    loadRows();
  }

  async function loadRows() {
    if (!table) return;
    const mine = ++request;
    setStatus("Loading…");
    const params = new URLSearchParams({ path, table: table.name, offset: String(page.offset), limit: String(PAGE) });
    if (page.order) params.set("order", page.order);
    if (page.desc) params.set("desc", "1");
    if (page.filter) params.set("filter", page.filter);
    try {
      const data = await api(`/api/sqlite/rows?${params}`);
      if (destroyed || mine !== request || mode !== "data") return;
      const first = data.rows.length ? page.offset + 1 : 0;
      const last = page.offset + data.rows.length;
      const total = data.total === null || data.total === undefined ? null : data.total;
      const filterBox = el("input", {
        type: "search",
        class: "sqlite-filter",
        placeholder: "Filter rows (text in any column)",
        value: page.filter,
        "aria-label": "Filter rows",
      });
      filterBox.addEventListener("input", () => {
        clearTimeout(filterTimer);
        filterTimer = setTimeout(() => {
          page = { ...page, filter: filterBox.value, offset: 0 };
          loadRows().then(() => {
            // Keep typing where it was after the list redraws.
            const again = body.querySelector(".sqlite-filter");
            if (again) {
              again.focus();
              again.setSelectionRange(again.value.length, again.value.length);
            }
          });
        }, 300);
      });
      const previous = el("button", { type: "button", disabled: page.offset === 0, onclick: () => { page.offset = Math.max(0, page.offset - PAGE); loadRows(); } }, "‹ Previous");
      const more = total === null ? data.rows.length === PAGE : last < total;
      const next = el("button", { type: "button", disabled: !more, onclick: () => { page.offset += PAGE; loadRows(); } }, "Next ›");
      const range = el("span", { class: "sqlite-range" }, data.rows.length
        ? `${number(first)}–${number(last)}${total === null ? "" : ` of ${number(total)}`}`
        : page.filter ? "No rows match" : "No rows");
      const pager = el("div", { class: "sqlite-pager" }, filterBox, range, previous, next);
      body.replaceChildren(pager, data.rows.length ? grid(data.columns, data.rows, { sortable: true, rowids: data.rowid }) : el("p", { class: "muted sqlite-message" }, page.filter ? "Nothing in this table matches the filter." : "This table is empty."));
      setStatus(`${data.elapsedMs} ms`);
    } catch (err) {
      if (destroyed || mine !== request) return;
      setStatus("");
      message(err.message);
    }
  }

  function renderSchema() {
    if (!table) return;
    const columns = el("table", { class: "sqlite-grid sqlite-columns" },
      el("thead", {}, el("tr", {}, ["Column", "Type", "Key", "Required"].map((name) => el("th", { scope: "col" }, name)))),
      el("tbody", {}, table.columns.map((column) => el("tr", {},
        el("td", {}, column.name),
        el("td", {}, column.type || el("span", { class: "sqlite-null" }, "any")),
        el("td", {}, column.pk ? (table.columns.filter((c) => c.pk).length > 1 ? `primary (${column.pk})` : "primary") : ""),
        el("td", {}, column.notNull ? "yes" : "")))));
    body.replaceChildren(
      el("div", { class: "sqlite-grid-wrap" }, columns),
      el("h4", {}, table.type === "view" ? "View definition" : "Definition"),
      el("pre", { class: "sqlite-code" }, table.sql || "(built in)"),
      table.indexes.length ? el("h4", {}, "Indexes") : null,
      table.indexes.length ? el("pre", { class: "sqlite-code" }, table.indexes.join(";\n") + ";") : null,
    );
    setStatus(table.rows === null ? "" : `${number(table.rows)} rows`);
  }

  function renderSql() {
    body.replaceChildren(
      el("div", { class: "sqlite-query" }, editor, el("div", { class: "sqlite-query-actions" },
        el("span", { class: "muted" }, "Read-only: SELECT, WITH, VALUES, or EXPLAIN. Up to 1,000 rows. ⌘↵ runs it."),
        run)),
      results,
    );
    setStatus("");
    requestAnimationFrame(() => editor.focus());
  }

  async function runQuery() {
    const sql = editor.value.trim();
    if (!sql) return;
    run.disabled = true;
    setStatus("Running…");
    try {
      const data = await api("/api/sqlite/query", { method: "POST", json: { path, sql } });
      if (destroyed) return;
      const note = data.truncated ? " (only the first 1,000 rows are shown)" : "";
      setStatus(`${number(data.rows.length)} row${data.rows.length === 1 ? "" : "s"}${note} · ${data.elapsedMs} ms`);
      results.replaceChildren(data.columns.length
        ? grid(data.columns, data.rows)
        : el("p", { class: "muted sqlite-message" }, "The query returned no columns."));
    } catch (err) {
      if (destroyed) return;
      setStatus("");
      results.replaceChildren(el("p", { class: "sqlite-error" }, err.message));
    } finally {
      run.disabled = false;
    }
  }

  function render() {
    for (const button of modes.children) button.setAttribute("aria-selected", button.dataset.mode === mode ? "true" : "false");
    for (const button of modes.children) button.disabled = !table && button.dataset.mode !== "sql";
    if (mode === "sql") return renderSql();
    if (!table) return message("This database has no tables. Use SQL to query it.");
    if (mode === "schema") return renderSchema();
    message("Loading…");
    loadRows();
  }

  (async () => {
    message("Opening database…");
    try {
      const data = await api(`/api/sqlite?path=${encodeURIComponent(path)}`);
      if (destroyed) return;
      tables = data.tables;
      picker.replaceChildren(...tables.map((item) => el("option", { value: item.name },
        `${item.name}${item.type === "view" ? " (view)" : ""}${item.rows === null ? "" : ` · ${number(item.rows)}`}`)));
      picker.hidden = !tables.length;
      // The table with the most rows is usually the interesting one.
      table = tables.filter((item) => item.type === "table").sort((a, b) => (b.rows || 0) - (a.rows || 0))[0] || tables[0] || null;
      if (table) picker.value = table.name;
      if (!table) mode = "sql";
      render();
    } catch (err) {
      if (!destroyed) message(err.message);
    }
  })();

  return {
    destroy() {
      destroyed = true;
      clearTimeout(filterTimer);
      root.remove();
    },
  };
}
