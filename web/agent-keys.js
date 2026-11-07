// Settings → AI agents: the keys that let an AI agent use the library over MCP, what each one
// can reach (the whole library or chosen folders) and do there (read, or also organise), what
// each one has done, and the changes organising keys made or are waiting to have approved, with
// Undo. Loaded on demand by app.js.

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
    el("p", { class: "muted" }, `${LEVELS[key.level] || key.level}${organises ? (key.approve ? ", asks first" : ", without asking") : ""}${key.tools ? ` · ${key.tools.length} chosen tools` : ""} · ${keyStatus(key, now)} · Made by ${key.createdBy} on ${date(key.createdAt)} · ${key.lastUsedAt ? `Last used ${dateTime(key.lastUsedAt)}` : "Never used"}`),
    probing ? el("p", { class: "backup-card agent-waiting agent-probing", role: "status" }, `Refused ${probing.refused} times in the last hour: it may be trying to reach files it wasn't given. Look at its Activity, and revoke it if that's not expected.`) : null,
    el("div", { class: "agent-actions" }, showLog, showUsage, showChanges, approval, active && el("button", { type: "button", onclick: () => settingsDialog(key) }, "Limits & tools…"), revoke),
    logHost,
    usageHost,
    changesHost);
}

async function render({ open } = {}) {
  const [{ keys, pending = {} }, attention] = await Promise.all([ctx.api("/api/agent-keys"), ctx.api("/api/agent-keys/attention").catch(() => ({ probing: [] }))]);
  const waiting = Object.values(pending).reduce((sum, count) => sum + count, 0);
  const now = Math.floor(Date.now() / 1000);
  section.replaceChildren(
    el("h3", {}, "AI agents"),
    el("p", { class: "muted" }, "A key lets an AI agent search and read the library over MCP, and, if you allow it, organise it. You choose what each key can reach. Agents never delete anything for good, and never see hidden files or the Trash."),
    waiting ? el("p", { class: "backup-card agent-waiting", role: "status" }, `${waiting} change${waiting === 1 ? "" : "s"} waiting for your approval: open Changes on the key below.`) : null,
    el("p", {}, "Address for agents: ", el("code", {}, endpoint()), " ", el("button", { type: "button", class: "agent-copy", onclick: () => copy(endpoint(), "Address copied") }, "Copy")),
    el("div", { "data-agent-secret": true }),
    el("h3", {}, "New key"),
    createForm(),
    el("h3", {}, "Keys"),
    ...(keys.length ? keys.map((key) => keyCard(key, now, pending[key.id] || 0, open === key.id || (pending[key.id] || 0) > 0, attention.probing.find((entry) => entry.id === key.id))) : [el("p", { class: "muted" }, "No keys yet, so no agent can connect.")]));
}

export async function renderSettings(host) {
  section = host;
  await render();
}
