// Settings → AI agents: the keys that let an AI agent read the library over MCP, what each one
// can reach (the whole library or chosen folders), and what each one has done. Loaded on demand
// by app.js.

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
const draft = { name: "", scope: "folders", folders: [], expires: "" };

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
  const wholeNote = el("p", { class: "muted" }, "The agent can read every file in the library, including folders added later.");
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

  const submit = el("button", { type: "submit", class: "primary" }, "Create key");
  const form = el("form", { class: "backup-card agent-form" },
    el("label", { class: "agent-field" }, el("span", {}, "Name"), name),
    el("fieldset", { class: "agent-scope" },
      el("legend", {}, "What it can read"),
      scopeChoice("folders", "Only these folders"),
      scopeChoice("whole", "Whole library"),
      foldersBlock,
      wholeNote),
    el("label", { class: "agent-field" }, el("span", {}, "Expires"), expires),
    el("div", { class: "agent-actions" }, submit));

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const folders = draft.scope === "whole" ? [WHOLE_LIBRARY] : draft.folders;
    if (!draft.name.trim()) return ctx.toast("Give the key a name", true);
    if (!folders.length) return ctx.toast("Add at least one folder, or choose the whole library", true);
    if (draft.scope === "whole" && !(await ctx.askConfirm("This key will be able to read every file in the library, apart from hidden files and the Trash. Create it?", "Create key"))) return;
    submit.disabled = true;
    try {
      const body = { name: draft.name.trim(), level: "read", folders };
      if (draft.expires) body.expiresAt = Math.floor(Date.now() / 1000) + Number(draft.expires) * DAY;
      const made = await ctx.api("/api/agent-keys", { method: "POST", json: body });
      const label = body.name;
      Object.assign(draft, { name: "", scope: "folders", folders: [], expires: "" });
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

function keyCard(key, now) {
  const active = !key.revokedAt && !(key.expiresAt && key.expiresAt <= now);
  const logHost = el("div", { class: "agent-log", hidden: true });
  const scope = key.folders.length
    ? key.folders.map((folder) => el("span", { class: `tag-chip mini agent-chip${folder === WHOLE_LIBRARY ? " agent-chip-whole" : ""}` }, folderLabel(folder)))
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
          entry.path ? el("code", {}, ` ${entry.path}`) : null)))
        : el("p", { class: "muted" }, "It has not been used yet."));
      logHost.hidden = false;
    } catch (err) {
      ctx.toast(err.message, true);
    }
  });
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
    el("p", { class: "muted" }, `Read only · ${keyStatus(key, now)} · Made by ${key.createdBy} on ${date(key.createdAt)} · ${key.lastUsedAt ? `Last used ${dateTime(key.lastUsedAt)}` : "Never used"}`),
    el("div", { class: "agent-actions" }, showLog, revoke),
    logHost);
}

async function render() {
  const { keys } = await ctx.api("/api/agent-keys");
  const now = Math.floor(Date.now() / 1000);
  section.replaceChildren(
    el("h3", {}, "AI agents"),
    el("p", { class: "muted" }, "A key lets an AI agent search and read the library over MCP. You choose what each key can reach. Agents can't change anything, and can never see hidden files or the Trash."),
    el("p", {}, "Address for agents: ", el("code", {}, endpoint()), " ", el("button", { type: "button", class: "agent-copy", onclick: () => copy(endpoint(), "Address copied") }, "Copy")),
    el("div", { "data-agent-secret": true }),
    el("h3", {}, "New key"),
    createForm(),
    el("h3", {}, "Keys"),
    ...(keys.length ? keys.map((key) => keyCard(key, now)) : [el("p", { class: "muted" }, "No keys yet, so no agent can connect.")]));
}

export async function renderSettings(host) {
  section = host;
  await render();
}
