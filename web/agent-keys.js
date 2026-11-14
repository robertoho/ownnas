// Settings → AI agents: the keys that let an AI agent use the library over MCP, what each one
// can reach (the whole library or chosen folders) and do there (read, or also organise), what
// each one has done, and the changes organising keys made or are waiting to have approved, with
// Undo. Also the file explorer's "Share with AI agent…" dialog (who has access, and a key for
// just the items chosen) and "AI agent access…" dialog (allow or exclude for every key or some).
// Loaded on demand by app.js.

let ctx = null;
let section = null;

export function init(context) {
  ctx = context;
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === false || value === undefined || value === null) continue;
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : String(child));
  }
  return node;
}

const WHOLE_LIBRARY = "";
const DAY = 86400;
const EXPIRY_CHOICES = [["", "Never"], ["30", "In 30 days"], ["90", "In 90 days"], ["365", "In a year"]];
const OUTCOMES = { ok: "Allowed", refused: "Refused", failed: "Failed" };

const date = (seconds) => new Date(seconds * 1000).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
const dateTime = (seconds) => new Date(seconds * 1000).toLocaleString([], { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", second: "2-digit" });
const endpoint = () => `${location.origin}/mcp`;

// What the form holds survives a refresh of the list.
const draft = { name: "", scope: "folders", folders: [], expires: "", level: "read", approve: true };
const STATUSES = { pending: "Waiting for you", done: "Made", declined: "Declined", failed: "Failed", undone: "Undone" };
const LEVELS = { read: "Read only", organise: "Read and organise", write: "Read, organise and write" };

async function copy(text, done) {
  try {
    await navigator.clipboard.writeText(text);
    ctx.toast(done);
  } catch {
    ctx.toast("Could not copy: select the text and copy it yourself", true);
  }
}

/// The one time a key's secret is shown, with what an agent needs to connect.
function secretCard(name, token) {
  const config = JSON.stringify({ mcpServers: { ownnas: { type: "http", url: endpoint(), headers: { Authorization: `Bearer ${token}` } } } }, null, 2);
  const card = el("div", { class: "backup-card agent-secret", role: "status" },
    el("p", { class: "backup-headline" }, `Key for ${name}`),
    el("p", {}, "Copy it now. It is shown only once and OwnNAS keeps no way to read it back."),
    el("code", { class: "agent-token" }, token),
    el("div", { class: "agent-actions" },
      el("button", { type: "button", onclick: () => copy(token, "Key copied") }, "Copy key"),
      el("button", { type: "button", onclick: () => copy(config, "Settings copied") }, "Copy MCP settings"),
      el("button", { type: "button", onclick: () => card.remove() }, "Done")));
  return card;
}

function folderLabel(path) {
  return path === WHOLE_LIBRARY ? "Whole library" : path;
}

function createForm() {
  const name = el("input", { type: "text", maxlength: "80", required: true, value: draft.name, placeholder: "For example: Claude on my laptop", "aria-label": "Key name" });
  name.addEventListener("input", () => { draft.name = name.value; });

  const expires = el("select", { "aria-label": "Key expires" }, EXPIRY_CHOICES.map(([value, label]) => el("option", { value, selected: value === draft.expires }, label)));
  expires.addEventListener("change", () => { draft.expires = expires.value; });

  const chosen = el("div", { class: "agent-folders" });
  const wholeNote = el("p", { class: "muted" }, "The agent can use every file in the library, including folders added later.");
  const add = el("button", { type: "button" }, "Add folder…");
  const foldersBlock = el("div", { class: "agent-scope-folders" }, chosen, add);

  const paint = () => {
    foldersBlock.hidden = draft.scope !== "folders";
    wholeNote.hidden = draft.scope !== "whole";
    chosen.replaceChildren(...draft.folders.map((folder) => el("span", { class: "tag-chip agent-chip" },
      folder,
      el("button", { type: "button", class: "agent-chip-remove", "aria-label": `Remove ${folder}`, title: "Remove", onclick: () => { draft.folders = draft.folders.filter((item) => item !== folder); paint(); } }, "×"))));
    if (!draft.folders.length) chosen.append(el("p", { class: "muted" }, "No folders chosen yet."));
  };

  add.addEventListener("click", async () => {
    const folder = await ctx.pickFolder({ title: "Folder the agent can read", ok: "Add this folder" });
    if (folder === null || folder === undefined) return;
    if (folder === WHOLE_LIBRARY) {
      ctx.toast('That is the whole library: choose "Whole library" above to give all of it', true);
      return;
    }
    if (!draft.folders.includes(folder)) draft.folders.push(folder);
    paint();
  });

  const scopeChoice = (value, label) => {
    const input = el("input", { type: "radio", name: "agent-scope", value, checked: draft.scope === value });
    input.addEventListener("change", () => { draft.scope = value; paint(); });
    return el("label", { class: "agent-scope-choice" }, input, el("span", {}, label));
  };

  const approveBox = el("input", { type: "checkbox", checked: draft.approve });
  approveBox.addEventListener("change", () => { draft.approve = approveBox.checked; });
  const approveRow = el("label", { class: "agent-scope-choice" }, approveBox,
    el("span", {}, "Ask me before each change", el("span", { class: "muted" }, " — changes wait here until you approve them")));
  approveRow.hidden = draft.level === "read";
  const levelChoice = (value, label, about) => {
    const input = el("input", { type: "radio", name: "agent-level", value, checked: draft.level === value });
    input.addEventListener("change", () => { draft.level = value; approveRow.hidden = value === "read"; });
    return el("label", { class: "agent-scope-choice" }, input, el("span", {}, label, el("span", { class: "muted" }, ` — ${about}`)));
  };

  const submit = el("button", { type: "submit", class: "primary" }, "Create key");
  const form = el("form", { class: "backup-card agent-form" },
    el("label", { class: "agent-field" }, el("span", {}, "Name"), name),
    el("fieldset", { class: "agent-scope" },
      el("legend", {}, "Which folders"),
      scopeChoice("folders", "Only these folders"),
      scopeChoice("whole", "Whole library"),
      foldersBlock,
      wholeNote),
    el("fieldset", { class: "agent-scope" },
      el("legend", {}, "What it can do"),
      levelChoice("read", "Read", "list, search, read and look at files"),
      levelChoice("organise", "Read and organise", "also tag, make folders, rename, move, move to the Trash, and arrange albums. Nothing is deleted for good, and protected files are left alone. Every change can be undone here."),
      levelChoice("write", "Read, organise and write", "also make text files, change passages, add rows to tables, and copy. What a file held is kept in its history first, so this can be undone too."),
      approveRow),
    el("label", { class: "agent-field" }, el("span", {}, "Expires"), expires),
    el("div", { class: "agent-actions" }, submit));

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const folders = draft.scope === "whole" ? [WHOLE_LIBRARY] : draft.folders;
    if (!draft.name.trim()) return ctx.toast("Give the key a name", true);
    if (!folders.length) return ctx.toast("Add at least one folder, or choose the whole library", true);
    const verb = { read: "read", organise: "read and organise", write: "read, organise and change" }[draft.level];
    if (draft.scope === "whole" && !(await ctx.askConfirm(`This key will be able to ${verb} every file in the library, apart from hidden files and the Trash. Create it?`, "Create key"))) return;
    submit.disabled = true;
    try {
      const body = { name: draft.name.trim(), level: draft.level, approve: draft.level !== "read" && draft.approve, folders };
      if (draft.expires) body.expiresAt = Math.floor(Date.now() / 1000) + Number(draft.expires) * DAY;
      const made = await ctx.api("/api/agent-keys", { method: "POST", json: body });
      const label = body.name;
      Object.assign(draft, { name: "", scope: "folders", folders: [], expires: "", level: "read", approve: true });
      await render();
      section.querySelector("[data-agent-secret]").append(secretCard(label, made.token));
    } catch (err) {
      ctx.toast(err.message, true);
    } finally {
      submit.disabled = false;
    }
  });

  paint();
  return form;
}

function keyStatus(key, now) {
  if (key.revokedAt) return `Revoked ${date(key.revokedAt)}`;
  if (key.expiresAt && key.expiresAt <= now) return `Expired ${date(key.expiresAt)}`;
  return key.expiresAt ? `Expires ${date(key.expiresAt)}` : "Does not expire";
}

/// A key's changes: what it asked for and what became of it, with Approve, Decline and Undo.
async function paintChanges(key, host) {
  const { changes } = await ctx.api(`/api/agent-keys/changes?id=${key.id}&limit=200`);
  const act = async (url, json) => {
    try {
      await ctx.api(url, { method: "POST", json });
      await render({ open: key.id });
    } catch (err) {
      ctx.toast(err.message, true);
      await paintChanges(key, host);
    }
  };
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const since = Math.floor(today.getTime() / 1000);
  const undoableToday = changes.filter((change) => change.canUndo && change.at >= since).length;
  const pending = changes.filter((change) => change.status === "pending");
  const bulk = el("div", { class: "agent-actions" },
    pending.length > 1 ? el("button", { type: "button", onclick: async () => {
      for (const change of pending) {
        try { await ctx.api("/api/agent-keys/changes/decide", { method: "POST", json: { id: change.id, approve: true } }); } catch (err) { ctx.toast(err.message, true); }
      }
      await render({ open: key.id });
    } }, `Approve all ${pending.length}`) : null,
    undoableToday ? el("button", { type: "button", onclick: async () => {
      if (!(await ctx.askConfirm(`Undo the ${undoableToday} change${undoableToday === 1 ? "" : "s"} "${key.name}" made today, newest first?`, "Undo"))) return;
      try {
        const result = await ctx.api("/api/agent-keys/changes/undo-since", { method: "POST", json: { keyId: key.id, since } });
        ctx.toast(result.skipped.length ? `Undid ${result.undone}. ${result.skipped.length} could not be undone: ${result.skipped[0].why}` : `Undid ${result.undone} change${result.undone === 1 ? "" : "s"}`, result.skipped.length > 0);
      } catch (err) {
        ctx.toast(err.message, true);
      }
      await render({ open: key.id });
    } }, "Undo today's changes") : null);
  host.replaceChildren(
    changes.length
      ? el("ul", { class: "agent-log-list agent-changes" }, changes.map((change) => el("li", { class: `agent-change agent-change-${change.status}` },
        el("span", { class: "muted" }, dateTime(change.at)), " ",
        el("strong", {}, `${STATUSES[change.status] || change.status}:`), el("span", {}, change.summary),
        change.status === "failed" && change.result ? el("span", { class: "muted" }, ` — ${change.result}`) : null,
        change.decidedBy && change.status !== "done" ? el("span", { class: "muted" }, ` (${change.decidedBy})`) : null,
        change.status === "pending" ? el("span", { class: "agent-change-actions" },
          el("button", { type: "button", class: "primary", onclick: () => act("/api/agent-keys/changes/decide", { id: change.id, approve: true }) }, "Approve"),
          el("button", { type: "button", onclick: () => act("/api/agent-keys/changes/decide", { id: change.id, approve: false }) }, "Decline")) : null,
        change.canUndo ? el("span", { class: "agent-change-actions" },
          el("button", { type: "button", onclick: () => act("/api/agent-keys/changes/undo", { id: change.id }) }, "Undo")) : null)))
      : el("p", { class: "muted" }, "It has not changed anything yet."),
    bulk);
}

/// A key's tools, limits, and folders it may only read.
async function settingsDialog(key) {
  let catalog;
  try {
    catalog = await ctx.api("/api/agent-keys/tools");
  } catch (err) {
    ctx.toast(err.message, true);
    return;
  }
  const levels = key.level === "write" ? ["read", "organise", "write"] : key.level === "organise" ? ["read", "organise"] : ["read"];
  const titles = { read: "Reading", organise: "Organising", write: "Writing" };
  const boxes = [];
  const groups = levels.map((level) => el("fieldset", { class: "agent-scope agent-tools" },
    el("legend", {}, titles[level]),
    catalog.tools[level].map((tool) => {
      const box = el("input", { type: "checkbox", value: tool.name, checked: !key.tools || key.tools.includes(tool.name) });
      boxes.push(box);
      return el("label", { class: "agent-scope-choice" }, box, el("span", {}, tool.title, el("code", { class: "muted" }, ` ${tool.name}`)));
    })));
  const readOnly = key.level === "read" ? [] : key.folders.map((folder) => {
    const box = el("input", { type: "checkbox", value: folder, checked: (key.readOnly || []).includes(folder) });
    return { box, node: el("label", { class: "agent-scope-choice" }, box, el("span", {}, "Only read ", el("strong", {}, folderLabel(folder)))) };
  });
  const extensions = el("input", { type: "text", spellcheck: "false", autocomplete: "off", value: (key.extensions || []).join(", "), placeholder: "Every file", "aria-label": "Only these file extensions" });
  const number = (value, fallback) => el("input", { type: "number", min: "0", step: "1", value: value ?? "", placeholder: String(fallback) });
  const calls = number(key.callsPerMinute, catalog.defaults.callsPerMinute);
  const changes = number(key.dailyChanges, catalog.defaults.dailyChanges);
  const save = el("button", { type: "button", class: "primary" }, "Save");
  const dialog = el("dialog", { class: "workspace-editor agent-settings" },
    el("h3", {}, `Limits & tools: ${key.name}`),
    el("p", { class: "muted" }, "The tools this key may use. Untick the ones it doesn't need."),
    groups,
    readOnly.length ? el("fieldset", { class: "agent-scope" }, el("legend", {}, "Folders it may only read"),
      el("p", { class: "muted" }, "It can still look in them, but not change anything there. A folder of its own inside one stays writable."),
      readOnly.map((item) => item.node)) : null,
    el("label", { class: "agent-field" }, el("span", {}, "Only these file extensions"), extensions),
    el("p", { class: "muted" }, "Such as md, txt, pdf. It then sees and changes only files ending in them (folders can still be browsed), makes only such files, and leaves alone folders holding other kinds. Empty: every file."),
    el("label", { class: "agent-field" }, el("span", {}, "Requests a minute"), calls),
    key.level === "read" ? null : el("label", { class: "agent-field" }, el("span", {}, "Changes a day before it asks"), changes),
    el("div", { class: "dialog-actions" }, el("span", { class: "spacer" }), el("button", { type: "button", onclick: () => dialog.close() }, "Cancel"), save));
  dialog.addEventListener("close", () => dialog.remove());
  save.addEventListener("click", async () => {
    const chosen = boxes.filter((box) => box.checked).map((box) => box.value);
    if (!chosen.length) return ctx.toast("Leave the key at least one tool", true);
    const body = {
      id: key.id,
      tools: chosen.length === boxes.length ? null : chosen,
      callsPerMinute: calls.value === "" ? null : Number(calls.value),
      dailyChanges: changes.value === "" ? null : Number(changes.value),
      readOnly: readOnly.filter((item) => item.box.checked).map((item) => item.box.value),
      extensions: extensions.value.split(/[\s,;]+/).map((ext) => ext.trim()).filter(Boolean),
    };
    save.disabled = true;
    try {
      await ctx.api("/api/agent-keys/settings", { method: "POST", json: body });
      dialog.close();
      ctx.toast("Saved. It applies from the agent's next request.");
      await render({ open: key.id });
    } catch (err) {
      ctx.toast(err.message, true);
    } finally {
      save.disabled = false;
    }
  });
  document.body.append(dialog);
  dialog.showModal();
}

/// The folders (or single files) a key may use, changed without a new secret.
function scopeDialog(key) {
  let whole = key.folders.includes(WHOLE_LIBRARY);
  let chosen = whole ? [] : key.folders.slice();
  const list = el("div", { class: "agent-folders" });
  const addFolder = el("button", { type: "button" }, "Add folder…");
  const addFile = el("button", { type: "button" }, "Add file…");
  const wholeBox = el("input", { type: "checkbox", checked: whole });
  const save = el("button", { type: "button", class: "primary" }, "Save");
  const paint = () => {
    list.replaceChildren(...chosen.map((path) => el("span", { class: "tag-chip agent-chip" },
      path,
      el("button", { type: "button", class: "agent-chip-remove", "aria-label": `Remove ${path}`, title: "Remove", onclick: () => { chosen = chosen.filter((item) => item !== path); paint(); } }, "×"))));
    if (!chosen.length && !whole) list.append(el("p", { class: "muted" }, "Nothing chosen yet."));
    list.hidden = whole;
    addFolder.hidden = whole;
    addFile.hidden = whole;
  };
  const add = async (files) => {
    const path = await ctx.pickFolder({ title: files ? "Folder or file the agent can use" : "Folder the agent can use", ok: "Add this folder", files });
    if (path === null || path === undefined) return;
    if (path === WHOLE_LIBRARY) return ctx.toast('That is the whole library: tick "The whole library" to give all of it', true);
    if (!chosen.includes(path)) chosen.push(path);
    paint();
  };
  addFolder.addEventListener("click", () => add(false));
  addFile.addEventListener("click", () => add(true));
  wholeBox.addEventListener("change", () => { whole = wholeBox.checked; paint(); });
  const dialog = el("dialog", { class: "workspace-editor agent-settings" },
    el("h3", {}, `Folders & files: ${key.name}`),
    el("p", { class: "muted" }, "Where this key may look. The agent keeps using the same key; the change applies from its next request."),
    el("label", { class: "agent-scope-choice" }, wholeBox, el("span", {}, "The whole library", el("span", { class: "muted" }, " — every file, including folders added later"))),
    list,
    el("div", { class: "agent-actions" }, addFolder, addFile),
    el("div", { class: "dialog-actions" }, el("span", { class: "spacer" }), el("button", { type: "button", onclick: () => dialog.close() }, "Cancel"), save));
  dialog.addEventListener("close", () => dialog.remove());
  save.addEventListener("click", async () => {
    const folders = whole ? [WHOLE_LIBRARY] : chosen;
    if (!folders.length) return ctx.toast("Add at least one folder or file, or choose the whole library", true);
    save.disabled = true;
    try {
      await ctx.api("/api/agent-keys/folders", { method: "POST", json: { id: key.id, folders } });
      dialog.close();
      ctx.toast("Saved. It applies from the agent's next request.");
      await render({ open: key.id });
    } catch (err) {
      ctx.toast(err.message, true);
    } finally {
      save.disabled = false;
    }
  });
  paint();
  document.body.append(dialog);
  dialog.showModal();
}

/// What a key did over the last 30 days: calls a day, by tool, and its changes.
async function paintUsage(key, host) {
  const usage = await ctx.api(`/api/agent-keys/usage?id=${key.id}&days=30`);
  const total = usage.days.reduce((sum, day) => sum + day.calls, 0);
  const refused = usage.days.reduce((sum, day) => sum + day.refused, 0);
  const most = Math.max(1, ...usage.days.map((day) => day.calls));
  const byDay = new Map(usage.days.map((day) => [day.day, day]));
  const today = Math.floor(Date.now() / 1000 / DAY) * DAY;
  const bars = el("div", { class: "agent-usage-bars", role: "img", "aria-label": `${total} calls in 30 days` },
    Array.from({ length: 30 }, (_, index) => {
      const day = today - (29 - index) * DAY;
      const found = byDay.get(day) || { calls: 0, refused: 0 };
      const bar = el("span", {
        class: `agent-usage-bar${found.refused ? " has-refused" : ""}`,
        title: `${date(day)}: ${found.calls} calls${found.refused ? `, ${found.refused} refused` : ""}`,
      });
      // Set through the style object: the page's policy refuses style attributes.
      bar.style.height = `${Math.max(2, Math.round((found.calls / most) * 100))}%`;
      return bar;
    }));
  const changes = Object.entries(usage.changes).map(([status, count]) => `${count} ${(STATUSES[status] || status).toLowerCase()}`).join(", ");
  const rows = usage.tools.sort((a, b) => Object.values(b.outcomes).reduce((x, y) => x + y, 0) - Object.values(a.outcomes).reduce((x, y) => x + y, 0));
  host.replaceChildren(
    el("p", {}, `${total} calls in the last 30 days${refused ? `, ${refused} refused` : ""}.${changes ? ` Changes: ${changes}.` : ""}`),
    bars,
    rows.length ? el("table", { class: "agent-usage-table" },
      el("thead", {}, el("tr", {}, el("th", {}, "Tool"), el("th", {}, "Allowed"), el("th", {}, "Refused"), el("th", {}, "Failed"))),
      el("tbody", {}, rows.map((row) => el("tr", {},
        el("td", {}, el("code", {}, row.tool)),
        el("td", {}, row.outcomes.ok || 0),
        el("td", { class: row.outcomes.refused ? "agent-usage-refused" : "" }, row.outcomes.refused || 0),
        el("td", {}, (row.outcomes.failed || 0) + (row.outcomes.pending || 0)))))) : null);
}

function keyCard(key, now, waiting, open, probing) {
  const active = !key.revokedAt && !(key.expiresAt && key.expiresAt <= now);
  const logHost = el("div", { class: "agent-log", hidden: true });
  const scope = key.folders.length
    ? key.folders.map((folder) => el("span", { class: `tag-chip mini agent-chip${folder === WHOLE_LIBRARY ? " agent-chip-whole" : ""}` }, folderLabel(folder), (key.readOnly || []).includes(folder) ? " (read only)" : ""))
    : el("span", { class: "muted" }, "Its folders are gone, so it can read nothing.");
  const excluded = (key.excluded || []).length
    ? el("div", { class: "agent-folders agent-excluded" }, el("span", { class: "muted" }, "Excluded:"),
      key.excluded.map((path) => el("span", { class: "tag-chip mini agent-chip agent-chip-excluded" }, folderLabel(path),
        active && el("button", { type: "button", class: "agent-chip-remove", "aria-label": `Stop excluding ${path}`, title: "Stop excluding", onclick: () => unexclude(path, [key.id]) }, "×"))))
    : null;
  const showLog = el("button", { type: "button" }, "Activity");
  showLog.addEventListener("click", async () => {
    if (!logHost.hidden) {
      logHost.hidden = true;
      return;
    }
    try {
      const { entries } = await ctx.api(`/api/agent-keys/log?id=${key.id}&limit=100`);
      logHost.replaceChildren(entries.length
        ? el("ul", { class: "agent-log-list" }, entries.map((entry) => el("li", {},
          el("span", { class: "muted" }, dateTime(entry.at)), " ",
          `${OUTCOMES[entry.outcome] || entry.outcome}: ${entry.tool}`,
          entry.path ? el("code", {}, ` ${entry.path}`) : null,
          entry.detail ? el("span", { class: "muted" }, ` ${entry.detail}`) : null)))
        : el("p", { class: "muted" }, "It has not been used yet."));
      logHost.hidden = false;
    } catch (err) {
      ctx.toast(err.message, true);
    }
  });
  const organises = key.level === "organise" || key.level === "write";
  const usageHost = el("div", { class: "agent-log", hidden: true });
  const showUsage = el("button", { type: "button", onclick: async () => {
    if (!usageHost.hidden) {
      usageHost.hidden = true;
      return;
    }
    try {
      await paintUsage(key, usageHost);
      usageHost.hidden = false;
    } catch (err) {
      ctx.toast(err.message, true);
    }
  } }, "Usage");
  const changesHost = el("div", { class: "agent-log", hidden: true });
  const showChanges = organises && el("button", { type: "button" }, waiting ? `Changes (${waiting} waiting)` : "Changes");
  const toggleChanges = async () => {
    if (!changesHost.hidden) {
      changesHost.hidden = true;
      return;
    }
    try {
      await paintChanges(key, changesHost);
      changesHost.hidden = false;
    } catch (err) {
      ctx.toast(err.message, true);
    }
  };
  if (showChanges) showChanges.addEventListener("click", toggleChanges);
  if (organises && open) toggleChanges();
  const approval = organises && active && el("button", { type: "button", onclick: async () => {
    try {
      await ctx.api("/api/agent-keys/approval", { method: "POST", json: { id: key.id, approve: !key.approve } });
      await render({ open: key.id });
    } catch (err) {
      ctx.toast(err.message, true);
    }
  } }, key.approve ? "Stop asking before changes" : "Ask before changes");
  const revoke = active && el("button", { type: "button", class: "danger", onclick: async () => {
    if (!(await ctx.askConfirm(`Revoke "${key.name}"? Agents using it lose access at once, and this can't be undone.`, "Revoke"))) return;
    try {
      await ctx.api("/api/agent-keys/revoke", { method: "POST", json: { id: key.id } });
      await render();
    } catch (err) {
      ctx.toast(err.message, true);
    }
  } }, "Revoke");
  return el("div", { class: `backup-card agent-key${active ? "" : " agent-key-inactive"}` },
    el("p", { class: "backup-headline agent-key-name" }, key.name),
    el("div", { class: "agent-folders" }, scope),
    excluded,
    el("p", { class: "muted" }, `${LEVELS[key.level] || key.level}${organises ? (key.approve ? ", asks first" : ", without asking") : ""}${key.tools ? ` · ${key.tools.length} chosen tools` : ""} · ${keyStatus(key, now)} · Made by ${key.createdBy} on ${date(key.createdAt)} · ${key.lastUsedAt ? `Last used ${dateTime(key.lastUsedAt)}` : "Never used"}`),
    probing ? el("p", { class: "backup-card agent-waiting agent-probing", role: "status" }, `Refused ${probing.refused} times in the last hour: it may be trying to reach files it wasn't given. Look at its Activity, and revoke it if that's not expected.`) : null,
    key.extensions ? el("p", { class: "muted" }, `Only ${key.extensions.map((ext) => `.${ext}`).join(", ")} files`) : null,
    el("div", { class: "agent-actions" }, showLog, showUsage, showChanges, approval,
      active && el("button", { type: "button", onclick: () => scopeDialog(key) }, "Folders & files…"),
      active && el("button", { type: "button", onclick: () => settingsDialog(key) }, "Limits & tools…"), revoke),
    logHost,
    usageHost,
    changesHost);
}

/// Lifts an exclusion set on exactly `path`: for the keys given, or (`null`) the one for every key.
async function unexclude(path, keys) {
  try {
    await ctx.api("/api/agent-keys/rules", { method: "POST", json: { paths: [path], keys, rule: "unexclude" } });
    ctx.toast(`No longer excluded: ${folderLabel(path)}`);
    if (ctx.onAgentAccessChange) ctx.onAgentAccessChange();
    await render({ open: keys && keys[0] });
  } catch (err) {
    ctx.toast(err.message, true);
  }
}

async function render({ open } = {}) {
  if (!section || !section.isConnected) return;
  const [{ keys, pending = {}, excludedForEveryKey = [] }, attention] = await Promise.all([ctx.api("/api/agent-keys"), ctx.api("/api/agent-keys/attention").catch(() => ({ probing: [] }))]);
  const waiting = Object.values(pending).reduce((sum, count) => sum + count, 0);
  const now = Math.floor(Date.now() / 1000);
  // replaceChildren would show a null as the text "null".
  section.replaceChildren(...[
    el("h3", {}, "AI agents"),
    el("p", { class: "muted" }, "A key lets an AI agent search and read the library over MCP, and, if you allow it, organise it. You choose what each key can reach. Agents never delete anything for good, and never see hidden files or the Trash."),
    waiting ? el("p", { class: "backup-card agent-waiting", role: "status" }, `${waiting} change${waiting === 1 ? "" : "s"} waiting for your approval: open Changes on the key below.`) : null,
    el("p", {}, "Address for agents: ", el("code", {}, endpoint()), " ", el("button", { type: "button", class: "agent-copy", onclick: () => copy(endpoint(), "Address copied") }, "Copy")),
    el("div", { "data-agent-secret": true }),
    el("h3", {}, "New key"),
    createForm(),
    el("h3", {}, "Keys"),
    excludedForEveryKey.length ? el("div", { class: "backup-card agent-every-excluded" },
      el("p", { class: "backup-headline" }, "Excluded for every key"),
      el("p", { class: "muted" }, "No key reaches these, even inside its folders, including keys made later. Set from the file explorer: right-click → AI agent access…"),
      el("div", { class: "agent-folders" }, excludedForEveryKey.map((path) => el("span", { class: "tag-chip mini agent-chip agent-chip-excluded" }, folderLabel(path),
        el("button", { type: "button", class: "agent-chip-remove", "aria-label": `Stop excluding ${path}`, title: "Stop excluding", onclick: () => unexclude(path, null) }, "×"))))) : null,
    ...(keys.length ? keys.map((key) => keyCard(key, now, pending[key.id] || 0, open === key.id || (pending[key.id] || 0) > 0, attention.probing.find((entry) => entry.id === key.id))) : [el("p", { class: "muted" }, "No keys yet, so no agent can connect.")])].filter(Boolean));
}

export async function renderSettings(host) {
  section = host;
  await render();
}

const REACH = {
  allowed: "Shared",
  inside: "Shared",
  excluded: "Excluded",
  excludedInside: "Excluded",
  none: "Not shared",
};

/// One key's reach of one item, in words: "Shared (in Docs)", "Excluded for every key"…
function reachText(reach) {
  const base = REACH[reach.state] || reach.state;
  if (reach.state === "inside") return `${base} (in ${folderLabel(reach.by)})`;
  if (reach.state === "allowed") return `${base} (given)`;
  if (reach.state === "excluded" || reach.state === "excludedInside") {
    const where = reach.state === "excludedInside" ? ` (in ${folderLabel(reach.by)})` : "";
    return `${base}${reach.everyKey ? " for every key" : ""}${where}`;
  }
  return base;
}

const shared = (state) => state === "allowed" || state === "inside";
const keptOut = (state) => state === "excluded" || state === "excludedInside";

/// The file explorer's dialog: how each key reaches the chosen folders and files, and buttons
/// to allow or exclude them for every key or the ones ticked.
export async function accessDialog(paths, title) {
  if (!paths.length) return;
  let keys = [];
  let items = [];
  const whoEvery = el("input", { type: "radio", name: "agent-access-who", value: "every", checked: true });
  const whoChosen = el("input", { type: "radio", name: "agent-access-who", value: "chosen" });
  const ticked = new Set();
  const table = el("div", { class: "agent-access-keys" }, el("p", { class: "muted" }, "Loading…"));
  const summary = el("div", { class: "agent-access-summary" });
  const notes = el("div", { class: "agent-access-notes", role: "status" });
  const allow = el("button", { type: "button", class: "primary", title: "Give these to the keys, and lift an exclusion of exactly these" }, "Allow");
  const exclude = el("button", { type: "button", class: "danger", title: "Keep these out of the keys' reach, even inside folders they were given" }, "Exclude");
  const clear = el("button", { type: "button", title: "Take away what was set on exactly these items, either way" }, "Remove rules");
  const count = paths.length === 1 ? "this item" : `these ${paths.length} items`;

  const paint = () => {
    const chosenMode = whoChosen.checked;
    const everyOut = items.filter((item) => item.excludedForEveryKey).length;
    summary.replaceChildren(...[
      everyOut ? el("p", { class: "agent-access-warning" }, items.length === 1 ? "Excluded for every key, including keys made later." : `${everyOut} of ${items.length} excluded for every key, including keys made later.`) : null,
      items.length < paths.length ? el("p", { class: "muted" }, `${paths.length - items.length} of the items can never be given to an agent (hidden, or in the Trash).`) : null,
    ].filter(Boolean));
    if (!keys.length) {
      table.replaceChildren(el("p", { class: "muted" }, "No keys work right now. Excluding for every key still holds for keys made later. Make keys in Settings → AI agents."));
      return;
    }
    table.replaceChildren(...keys.map((key) => {
      const reaches = items.map((item) => ({ path: item.path, reach: item.keys.find((reach) => reach.keyId === key.id) })).filter((entry) => entry.reach);
      const sharedCount = reaches.filter((entry) => shared(entry.reach.state)).length;
      const outCount = reaches.filter((entry) => keptOut(entry.reach.state)).length;
      let status;
      let tone = "none";
      if (reaches.length === 1) {
        status = reachText(reaches[0].reach);
        tone = shared(reaches[0].reach.state) ? "shared" : keptOut(reaches[0].reach.state) ? "excluded" : "none";
      } else if (sharedCount === reaches.length) {
        status = `All ${reaches.length} shared`;
        tone = "shared";
      } else if (outCount === reaches.length) {
        status = `All ${reaches.length} excluded`;
        tone = "excluded";
      } else if (!sharedCount && !outCount) {
        status = "Not shared";
      } else {
        status = [sharedCount && `${sharedCount} shared`, outCount && `${outCount} excluded`, reaches.length - sharedCount - outCount && `${reaches.length - sharedCount - outCount} not shared`].filter(Boolean).join(", ");
        tone = "mixed";
      }
      const box = el("input", { type: "checkbox", checked: ticked.has(key.id), disabled: !chosenMode, "aria-label": `Choose ${key.name}` });
      box.addEventListener("change", () => { if (box.checked) ticked.add(key.id); else ticked.delete(key.id); });
      const details = reaches.length > 1 ? el("details", { class: "agent-access-details" }, el("summary", {}, "Each item"),
        el("ul", {}, reaches.map((entry) => el("li", {}, el("code", {}, folderLabel(entry.path)), " ", reachText(entry.reach))))) : null;
      return el("div", { class: "agent-access-key" },
        el("label", { class: "agent-scope-choice" }, box, el("span", {}, el("strong", {}, key.name), el("span", { class: "muted" }, ` · ${LEVELS[key.level] || key.level}`))),
        el("span", { class: `agent-access-state agent-access-${tone}` }, status),
        details);
    }));
  };

  const load = async () => {
    const [listed, access] = await Promise.all([ctx.api("/api/agent-keys"), ctx.api("/api/agent-keys/access", { method: "POST", json: { paths } })]);
    const now = Math.floor(Date.now() / 1000);
    keys = listed.keys.filter((key) => !key.revokedAt && !(key.expiresAt && key.expiresAt <= now));
    items = access.items;
    for (const id of [...ticked]) if (!keys.some((key) => key.id === id)) ticked.delete(id);
    paint();
  };

  const act = async (rule, button) => {
    const every = whoEvery.checked;
    if (!every && !ticked.size) return ctx.toast("Tick at least one key, or choose every key", true);
    if (rule === "exclude" && every && !(await ctx.askConfirm(`Exclude ${count} for every AI agent key, including keys made later?`, "Exclude"))) return;
    button.disabled = true;
    try {
      const result = await ctx.api("/api/agent-keys/rules", { method: "POST", json: { paths, keys: every ? null : [...ticked], rule } });
      notes.replaceChildren(...(result.notes || []).map((note) => el("p", { class: "agent-access-warning" }, note)));
      ctx.toast(rule === "allow" ? "Allowed. It applies from each agent's next request." : rule === "exclude" ? "Excluded. It applies from each agent's next request." : "Rules removed");
      await load();
      if (ctx.onAgentAccessChange) ctx.onAgentAccessChange();
      render().catch(() => {});
    } catch (err) {
      ctx.toast(err.message, true);
    } finally {
      button.disabled = false;
    }
  };
  allow.addEventListener("click", () => act("allow", allow));
  exclude.addEventListener("click", () => act("exclude", exclude));
  clear.addEventListener("click", () => act("clear", clear));
  whoEvery.addEventListener("change", paint);
  whoChosen.addEventListener("change", paint);

  const dialog = el("dialog", { class: "workspace-editor agent-settings agent-access" },
    el("h3", {}, `AI agent access: ${title}`),
    el("p", { class: "muted" }, "Which MCP keys may reach ", count, ". An exclusion always wins: an excluded item stays out of reach even inside a folder a key was given."),
    summary,
    el("fieldset", { class: "agent-scope" }, el("legend", {}, "For"),
      el("label", { class: "agent-scope-choice" }, whoEvery, el("span", {}, "Every key", el("span", { class: "muted" }, " — an exclusion also holds for keys made later"))),
      el("label", { class: "agent-scope-choice" }, whoChosen, el("span", {}, "The keys ticked below"))),
    table,
    notes,
    el("div", { class: "dialog-actions" }, clear, el("span", { class: "spacer" }), el("button", { type: "button", onclick: () => dialog.close() }, "Close"), exclude, allow));
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  try {
    await load();
  } catch (err) {
    dialog.close();
    throw err;
  }
}

const SHARE_EXPIRY = [["", "Never"], ["7", "In 7 days"], ["30", "In 30 days"], ["90", "In 90 days"]];

/// "Share with AI agent…" from the file explorer, like sharing a folder in a cloud drive: who
/// has access now (with Remove), sharing with a key that exists, or a new key for just these
/// items, whose secret and MCP settings are shown once to copy.
export async function shareDialog(paths, title) {
  if (!paths.length) return;
  let keys = [];
  let items = [];
  const count = paths.length === 1 ? "this item" : `these ${paths.length} items`;
  const people = el("div", { class: "agent-access-keys" }, el("p", { class: "muted" }, "Loading…"));
  const existing = el("select", { "aria-label": "Key to share with" });
  const shareExisting = el("button", { type: "button" }, "Share");
  const existingRow = el("div", { class: "agent-share-row" }, existing, shareExisting);
  const name = el("input", { type: "text", maxlength: "80", value: `${title}`.slice(0, 80), "aria-label": "Name of the new key" });
  const level = el("select", { "aria-label": "What the agent can do" },
    el("option", { value: "read" }, "Can read"),
    el("option", { value: "organise" }, "Can read and organise"),
    el("option", { value: "write" }, "Can read, organise and write"));
  const expires = el("select", { "aria-label": "Key expires" }, SHARE_EXPIRY.map(([value, label]) => el("option", { value, selected: value === "30" }, label)));
  const create = el("button", { type: "button", class: "primary" }, "Create key");
  const secretHost = el("div");
  const changed = () => {
    if (ctx.onAgentAccessChange) ctx.onAgentAccessChange();
    render().catch(() => {});
  };

  const paint = () => {
    const withAccess = keys.map((key) => {
      const reaches = items.map((item) => ({ path: item.path, reach: item.keys.find((reach) => reach.keyId === key.id) })).filter((entry) => entry.reach && shared(entry.reach.state));
      return { key, reaches };
    }).filter((entry) => entry.reaches.length);
    people.replaceChildren(...(withAccess.length ? withAccess.map(({ key, reaches }) => {
      const how = reaches.length === items.length && reaches.every((entry) => entry.reach.state === "allowed")
        ? "Shared directly"
        : reaches.length === 1 && items.length === 1
          ? `Through ${folderLabel(reaches[0].reach.by)}`
          : `${reaches.length} of ${items.length} items`;
      const remove = el("button", { type: "button", class: "agent-share-remove" }, "Remove");
      remove.addEventListener("click", () => removeAccess(key, reaches, remove));
      return el("div", { class: "agent-access-key" },
        el("span", {}, el("strong", {}, key.name), el("span", { class: "muted" }, ` · ${LEVELS[key.level] || key.level}`), el("br"), el("span", { class: "muted agent-share-how" }, how)),
        remove);
    }) : [el("p", { class: "muted" }, `No AI agent can reach ${count} yet.`)]));
    const others = keys.filter((key) => !withAccess.some((entry) => entry.key.id === key.id && entry.reaches.length === items.length));
    existing.replaceChildren(...others.map((key) => el("option", { value: String(key.id) }, `${key.name} · ${LEVELS[key.level] || key.level}`)));
    existingRow.hidden = !others.length;
  };

  const load = async () => {
    const [listed, access] = await Promise.all([ctx.api("/api/agent-keys"), ctx.api("/api/agent-keys/access", { method: "POST", json: { paths } })]);
    const now = Math.floor(Date.now() / 1000);
    keys = listed.keys.filter((key) => !key.revokedAt && !(key.expiresAt && key.expiresAt <= now));
    items = access.items;
    paint();
  };

  /// Takes these items away from a key: an item given directly loses its entry, one reached
  /// through a folder is excluded. A key left with nothing else is revoked instead.
  const removeAccess = async (key, reaches, button) => {
    const direct = reaches.filter((entry) => entry.reach.state === "allowed").map((entry) => entry.path);
    const through = reaches.filter((entry) => entry.reach.state === "inside").map((entry) => entry.path);
    const onlyThese = key.folders.length && key.folders.every((folder) => direct.includes(folder));
    button.disabled = true;
    try {
      if (onlyThese) {
        if (!(await ctx.askConfirm(`“${key.name}” can reach nothing else. Revoke the key? Agents using it lose access at once.`, "Revoke"))) return;
        await ctx.api("/api/agent-keys/revoke", { method: "POST", json: { id: key.id } });
        ctx.toast(`Revoked “${key.name}”`);
      } else {
        if (direct.length) await ctx.api("/api/agent-keys/rules", { method: "POST", json: { paths: direct, keys: [key.id], rule: "clear" } });
        if (through.length) await ctx.api("/api/agent-keys/rules", { method: "POST", json: { paths: through, keys: [key.id], rule: "exclude" } });
        ctx.toast(through.length ? `Removed for “${key.name}”: excluded inside its folder` : `Removed for “${key.name}”`);
      }
      await load();
      changed();
    } catch (err) {
      ctx.toast(err.message, true);
    } finally {
      button.disabled = false;
    }
  };

  shareExisting.addEventListener("click", async () => {
    const id = Number(existing.value);
    if (!id) return;
    shareExisting.disabled = true;
    try {
      const result = await ctx.api("/api/agent-keys/rules", { method: "POST", json: { paths, keys: [id], rule: "allow" } });
      for (const note of result.notes || []) ctx.toast(note, true);
      if (!(result.notes || []).length) ctx.toast("Shared. It applies from the agent's next request.");
      await load();
      changed();
    } catch (err) {
      ctx.toast(err.message, true);
    } finally {
      shareExisting.disabled = false;
    }
  });

  create.addEventListener("click", async () => {
    if (!name.value.trim()) return ctx.toast("Give the key a name", true);
    create.disabled = true;
    try {
      const body = { name: name.value.trim(), level: level.value, approve: level.value !== "read", folders: paths };
      if (expires.value) body.expiresAt = Math.floor(Date.now() / 1000) + Number(expires.value) * DAY;
      const made = await ctx.api("/api/agent-keys", { method: "POST", json: body });
      secretHost.replaceChildren(secretCard(body.name, made.token));
      secretHost.scrollIntoView({ block: "nearest", behavior: "smooth" });
      await load();
      changed();
    } catch (err) {
      ctx.toast(err.message, true);
    } finally {
      create.disabled = false;
    }
  });

  const advanced = el("button", { type: "button", title: "Allow or exclude for every key or chosen ones" }, "Allow or exclude…");
  const dialog = el("dialog", { class: "workspace-editor agent-settings agent-access agent-share" },
    el("h3", {}, `Share with AI agents: ${title}`),
    el("p", { class: "muted" }, "An AI agent such as Claude can use what you share here over MCP, with a key. Hidden files and the Trash are never shared."),
    el("h3", {}, "Who has access"),
    people,
    existingRow,
    el("h3", {}, "New key for ", count),
    el("p", { class: "muted" }, "It reaches only ", count, ". Changes by an organising key wait for your approval."),
    el("div", { class: "agent-share-new" },
      el("label", { class: "agent-field" }, el("span", {}, "Name"), name),
      el("label", { class: "agent-field" }, el("span", {}, "Access"), level),
      el("label", { class: "agent-field" }, el("span", {}, "Expires"), expires)),
    el("div", { class: "agent-actions" }, create),
    secretHost,
    el("div", { class: "dialog-actions" }, advanced, el("span", { class: "spacer" }), el("button", { type: "button", onclick: () => dialog.close() }, "Done")));
  advanced.addEventListener("click", () => {
    dialog.close();
    accessDialog(paths, title).catch((err) => ctx.toast(err.message, true));
  });
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  try {
    await load();
  } catch (err) {
    dialog.close();
    throw err;
  }
}
