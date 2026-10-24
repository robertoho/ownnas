// Settings → Backup: backing up the library and OwnNAS's data with restic, to a drive, an
// S3-compatible bucket, or another computer over SFTP; the snapshots, and restoring a folder
// from one. Loaded on demand by app.js.

let ctx = null;
let section = null;
let timer = 0;
let status = null;

export function init(context) {
  ctx = context;
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === false || value === undefined || value === null) continue;
    if (key === "class") node.className = value;
    // Through the CSSOM: the page's security policy blocks style attributes.
    else if (key === "style") node.style.cssText = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : String(child));
  }
  return node;
}

const number = (n) => Number(n).toLocaleString();
const size = (bytes) => ctx.formatSize(Number(bytes) || 0, false);
const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// "today at 03:00", "yesterday at 03:00", "Tuesday at 03:00", "12 Oct at 03:00".
function when(seconds) {
  const date = new Date(seconds * 1000);
  const time = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const day = new Date(date);
  day.setHours(0, 0, 0, 0);
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const days = Math.round((day - today) / 86400000);
  if (days === 0) return `today at ${time}`;
  if (days === -1) return `yesterday at ${time}`;
  if (days === 1) return `tomorrow at ${time}`;
  if (days > -7 && days < 7) return `${WEEKDAYS[date.getDay()]} at ${time}`;
  return `${date.toLocaleDateString([], { day: "numeric", month: "short", year: date.getFullYear() === new Date().getFullYear() ? undefined : "numeric" })} at ${time}`;
}

function duration(seconds) {
  const s = Math.round(seconds);
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.round(s / 60)} min`;
  return `${Math.floor(s / 3600)} h ${Math.round((s % 3600) / 60)} min`;
}

const pad = (n) => String(n).padStart(2, "0");

export async function renderSettings(target) {
  section = target;
  clearTimeout(timer);
  status = await ctx.api("/api/nas-backup");
  section.replaceChildren();
  section.append(el("p", { class: "muted" },
    "Back up the library and OwnNAS's own data (users, albums, tags, settings, and file history) to a drive, a bucket, or another computer. Backups are encrypted, and after the first one only what changed is sent."));
  if (!status.restic.found) {
    section.append(el("div", { class: "backup-card" },
      el("p", {}, el("strong", {}, "Backups use restic, which is not installed on this computer.")),
      el("p", { class: "muted" }, "Install it, then come back to this page:"),
      el("code", { class: "backup-command" }, status.restic.install),
      el("div", { class: "dialog-actions" }, el("button", { type: "button", onclick: () => renderSettings(section) }, "Check again"))));
    return;
  }
  if (!status.restic.newEnough) {
    section.append(el("p", { class: "text-warning" }, `restic ${status.restic.version} is too old for restoring folders. Update it to 0.17 or newer.`));
  }
  if (status.configured) section.append(renderStatus());
  section.append(renderForm());
  if (status.configured) {
    const box = el("div", { class: "backup-snapshots" });
    section.append(box);
    renderSnapshots(box, false);
  }
  poll();
}

/// Keeps the status card current while a backup runs (and now and then otherwise).
function poll() {
  clearTimeout(timer);
  if (!section || !status?.configured) return;
  timer = setTimeout(async () => {
    if (!section.isConnected || section.hidden) return;
    try {
      const before = status.progress;
      status = await ctx.api("/api/nas-backup");
      const card = section.querySelector(".backup-status");
      if (card) card.replaceWith(renderStatus());
      // Something just finished: the snapshots changed.
      if (before && !status.progress) {
        const box = section.querySelector(".backup-snapshots");
        if (box) renderSnapshots(box, true);
        const done = status.state;
        if (before.action === "backup") {
          if (done.lastResult === "ok") ctx.toast("Backup finished");
          else if (done.lastResult === "failed") ctx.toast(`The backup failed: ${done.lastError}`, true);
        } else if (before.action === "restore" && done.lastRestore) {
          if (done.lastRestore.error) ctx.toast(`The restore failed: ${done.lastRestore.error}`, true);
          else ctx.toast(`Restored to ${done.lastRestore.to}`);
        } else if (before.action === "verify") {
          ctx.toast(done.verifyResult === "ok" ? "The backup checked out" : `The check found problems: ${done.verifyError}`, done.verifyResult !== "ok");
        }
      }
    } catch {}
    poll();
  }, status.progress ? 1500 : 20000);
}

function renderStatus() {
  const { state, progress, config } = status;
  const card = el("div", { class: "backup-card backup-status" });
  const result = state.lastResult;
  let line;
  if (!state.lastStarted) line = el("p", { class: "backup-headline" }, "No backup yet.");
  else if (result === "failed") line = el("p", { class: "backup-headline is-bad" }, `The last backup failed ${when(state.lastFinished)}: ${state.lastError || "unknown error"}`);
  else if (result === "stopped") line = el("p", { class: "backup-headline" }, `The last backup was stopped ${when(state.lastFinished)}.`);
  else if (state.lastSummary) {
    const s = state.lastSummary;
    const changed = s.filesNew + s.filesChanged;
    line = el("p", { class: `backup-headline${result === "warning" ? " is-warn" : " is-good"}` },
      `Last backup ${when(state.lastFinished)}: ${number(s.totalFiles)} files, ${changed ? `${number(changed)} new or changed (${size(s.dataAdded)} sent)` : "nothing changed"}, in ${duration(s.seconds)}.`);
  } else line = el("p", { class: "backup-headline" }, "No backup yet.");
  card.append(line);
  if (result === "warning" && state.lastError) card.append(el("p", { class: "text-warning" }, state.lastError));
  if (result === "failed" && state.lastSuccess) card.append(el("p", { class: "muted" }, `The last one that worked was ${when(state.lastSuccess)}.`));
  const facts = [];
  facts.push(`To ${status.where}`);
  if (config.frequency === "manual") facts.push("only when you choose Back up now");
  else if (status.next) facts.push(`next ${when(status.next)}`);
  if (state.storedBytes) facts.push(`it takes ${size(state.storedBytes)} there`);
  card.append(el("p", { class: "muted" }, `${facts.join(" · ")}.`));
  if (state.lastVerified) {
    card.append(el("p", { class: state.verifyResult === "ok" ? "muted" : "text-warning" },
      state.verifyResult === "ok" ? `Last checked ${when(state.lastVerified)}: a sample read back correctly.` : `The check ${when(state.lastVerified)} found problems: ${state.verifyError}`));
  }
  if (state.lastRestore && !state.lastRestore.error) {
    const open = el("button", { type: "button", class: "link-button", onclick: () => ctx.goToFolder(state.lastRestore.to) }, state.lastRestore.to);
    card.append(el("p", { class: "muted" }, `Last restore ${when(state.lastRestore.finished)} went to `, open));
  }
  if (progress) {
    const label = { backup: "Backing up", restore: "Restoring", verify: "Checking the backup", connect: "Connecting" }[progress.action] || progress.action;
    const bar = el("span");
    bar.style.width = `${Math.round((progress.percent || 0) * 100)}%`;
    const bits = [];
    if (progress.totalFiles) bits.push(`${number(progress.filesDone)} of ${number(progress.totalFiles)} files`);
    if (progress.totalBytes) bits.push(`${size(progress.bytesDone)} of ${size(progress.totalBytes)}`);
    if (progress.secondsRemaining) bits.push(`about ${duration(progress.secondsRemaining)} left`);
    card.append(el("div", { class: "backup-progress" },
      el("p", {}, el("strong", {}, label), ` · ${progress.phase}`),
      el("div", { class: "backup-bar", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(Math.round((progress.percent || 0) * 100)) }, bar),
      bits.length ? el("p", { class: "muted" }, bits.join(" · ")) : null,
      progress.current ? el("p", { class: "muted backup-current" }, progress.current) : null));
  }
  const actions = el("div", { class: "dialog-actions backup-actions" });
  if (progress) {
    actions.append(el("button", { type: "button", onclick: stop }, "Stop"));
  } else {
    actions.append(
      el("button", { type: "button", class: "primary", onclick: () => start("run") }, "Back up now"),
      el("button", { type: "button", onclick: () => start("verify"), title: "Reads back 5% of the backup to check it is complete and intact" }, "Check the backup"),
    );
  }
  card.append(actions);
  return card;
}

async function start(what) {
  try {
    status = { ...status, ...(await ctx.api(`/api/nas-backup/${what}`, { method: "POST", json: {} })) };
    status = await ctx.api("/api/nas-backup");
    section.querySelector(".backup-status")?.replaceWith(renderStatus());
    poll();
  } catch (err) {
    ctx.toast(err.message, true);
  }
}

async function stop() {
  if (!(await ctx.askConfirm("Stop now? Nothing is lost: the next backup carries on from what is already there.", "Stop"))) return;
  try {
    await ctx.api("/api/nas-backup/stop", { method: "POST", json: {} });
  } catch (err) {
    ctx.toast(err.message, true);
  }
}

function field(label, input, hint) {
  return el("label", {}, label, input, hint ? el("span", { class: "muted backup-hint" }, hint) : null);
}

function renderForm() {
  const { config } = status;
  const d = config.destination;
  const form = el("form", { class: "user-form backup-form" });
  const kinds = [["local", "Drive or folder"], ["s3", "S3-compatible bucket"], ["sftp", "Another computer (SFTP)"]];
  let kind = d.kind || "local";

  // Where.
  const kindPicker = el("div", { class: "backup-kinds", role: "radiogroup", "aria-label": "Where backups go" });
  const pane = el("div", { class: "backup-where" });
  const inputs = {
    path: el("input", { autocomplete: "off", spellcheck: "false", value: d.kind === "local" ? d.path : "", placeholder: "/Volumes/Backup/OwnNAS" }),
    endpoint: el("input", { autocomplete: "off", spellcheck: "false", value: d.endpoint, placeholder: "s3.amazonaws.com" }),
    bucket: el("input", { autocomplete: "off", spellcheck: "false", value: d.bucket }),
    prefix: el("input", { autocomplete: "off", spellcheck: "false", value: d.prefix, placeholder: "Optional" }),
    region: el("input", { autocomplete: "off", spellcheck: "false", value: d.region, placeholder: "Optional, such as eu-central-1" }),
    accessKeyId: el("input", { autocomplete: "off", spellcheck: "false", value: d.accessKeyId }),
    s3Secret: el("input", { type: "password", autocomplete: "new-password", placeholder: status.s3SecretSet ? "Saved. Leave empty to keep it" : "" }),
    host: el("input", { autocomplete: "off", spellcheck: "false", value: d.host, placeholder: "backup.example.com" }),
    port: el("input", { type: "number", min: "1", max: "65535", value: d.port || 22 }),
    user: el("input", { autocomplete: "off", spellcheck: "false", value: d.user }),
    remotePath: el("input", { autocomplete: "off", spellcheck: "false", value: d.kind === "sftp" ? d.path : "", placeholder: "/srv/backups/ownnas" }),
  };
  const showKind = () => {
    for (const button of kindPicker.children) button.setAttribute("aria-checked", button.dataset.kind === kind ? "true" : "false");
    pane.replaceChildren();
    if (kind === "local") {
      pane.append(field("Folder", inputs.path, "The full path of a folder on a drive this computer can write to, such as a USB disk. Keep it on a different drive from the library."));
    } else if (kind === "s3") {
      pane.append(
        field("Endpoint", inputs.endpoint, "AWS: s3.amazonaws.com. Backblaze B2: s3.<region>.backblazeb2.com. Cloudflare R2: <account>.r2.cloudflarestorage.com. MinIO and others: https://host:port."),
        el("div", { class: "backup-row" }, field("Bucket", inputs.bucket), field("Folder in the bucket", inputs.prefix)),
        field("Region", inputs.region),
        el("div", { class: "backup-row" }, field("Access key ID", inputs.accessKeyId), field("Secret access key", inputs.s3Secret)),
      );
    } else {
      pane.append(
        el("div", { class: "backup-row" }, field("Computer", inputs.host), field("Port", inputs.port)),
        el("div", { class: "backup-row" }, field("User", inputs.user), field("Folder", inputs.remotePath)),
        sshKeyBox(),
      );
    }
  };
  for (const [value, label] of kinds) {
    kindPicker.append(el("button", { type: "button", role: "radio", "data-kind": value, onclick: () => { kind = value; showKind(); } }, label));
  }
  showKind();
  form.append(el("h3", {}, "Where"), kindPicker, pane);

  // The password.
  const passwordBox = el("div", { class: "backup-password" });
  const repoPassword = el("input", { type: "password", autocomplete: "off", spellcheck: "false" });
  if (!status.passwordSet) {
    const existing = el("input", { type: "checkbox" });
    const passwordField = field("Its password", repoPassword);
    passwordField.hidden = true;
    existing.addEventListener("change", () => { passwordField.hidden = !existing.checked; });
    passwordBox.append(
      el("p", { class: "muted" }, "Backups are encrypted. OwnNAS makes up a password when you save, and shows it once for you to write down."),
      el("label", { class: "check" }, existing, " There is already an OwnNAS backup there, with a password of its own"),
      passwordField);
  } else {
    const change = el("button", { type: "button", class: "link-button" }, "Use a different password");
    const passwordField = field("The backup's password", repoPassword, "Only for a destination that already has a backup made with this password.");
    passwordField.hidden = true;
    change.addEventListener("click", () => { passwordField.hidden = false; change.hidden = true; });
    passwordBox.append(
      el("p", { class: "muted" }, "Backups are encrypted with their password. Without it, nobody can restore them. ",
        el("button", { type: "button", class: "link-button", onclick: revealPassword }, "Show the password"), " · ", change),
      passwordField);
  }
  form.append(passwordBox);

  // When.
  const frequency = el("select", {},
    el("option", { value: "daily" }, "Every day"),
    el("option", { value: "weekly" }, "Every week"),
    el("option", { value: "manual" }, "Only when I choose Back up now"));
  frequency.value = config.frequency;
  const weekday = el("select", {}, WEEKDAYS.map((name, i) => el("option", { value: String(i) }, `on ${name}`)));
  weekday.value = String(config.weekday);
  const time = el("input", { type: "time", value: `${pad(Math.floor(config.minute / 60))}:${pad(config.minute % 60)}`, required: true });
  const syncWhen = () => {
    weekday.hidden = frequency.value !== "weekly";
    time.hidden = frequency.value === "manual";
    atLabel.hidden = frequency.value === "manual";
  };
  const atLabel = el("span", {}, "at");
  frequency.addEventListener("change", syncWhen);
  form.append(el("h3", {}, "When"), el("div", { class: "backup-when" }, frequency, weekday, atLabel, time),
    el("p", { class: "muted backup-hint" }, "If the computer is off or asleep then, the backup runs when it is back. A failed backup is tried again an hour later.",
      status.serverOffset !== -new Date().getTimezoneOffset() ? ` The time is on the server's clock (UTC${status.serverOffset < 0 ? "−" : "+"}${pad(Math.floor(Math.abs(status.serverOffset) / 60))}:${pad(Math.abs(status.serverOffset) % 60)}), not this device's.` : ""));
  syncWhen();

  // How long.
  const keep = (value) => el("input", { type: "number", min: "0", max: "1000", value: String(value), class: "backup-keep" });
  const keepDaily = keep(config.keepDaily);
  const keepWeekly = keep(config.keepWeekly);
  const keepMonthly = keep(config.keepMonthly);
  form.append(el("h3", {}, "How long to keep them"),
    el("p", { class: "backup-keeps" }, "One a day for ", keepDaily, " days, one a week for ", keepWeekly, " weeks, and one a month for ", keepMonthly, " months. The newest is always kept."));

  // What.
  const skipTrash = el("input", { type: "checkbox" });
  skipTrash.checked = config.skipTrash;
  const excludes = el("textarea", { rows: "3", spellcheck: "false", placeholder: "*.tmp\n/full/path/of/a/folder" });
  excludes.value = (config.excludes || []).join("\n");
  form.append(el("h3", {}, "What"),
    el("p", { class: "muted" }, "The library (", el("code", {}, status.library), ") and OwnNAS's data (", el("code", {}, status.dataDir), "), without thumbnails and other caches, which are made again."),
    el("label", { class: "check" }, skipTrash, " Leave out the Trash"),
    field("Also leave out", excludes, "One per line: a pattern such as *.iso, or a folder's full path."));

  const account = el("input", { type: "password", autocomplete: "current-password" });
  form.append(field("Your account password", account, "Needed when you change where backups go or their password."));
  const error = el("p", { class: "form-error", role: "alert" });
  const save = el("button", { type: "submit", class: "primary" }, status.configured ? "Save" : "Save and set up the backup");
  form.append(el("div", { class: "dialog-actions" }, save), error);

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    error.textContent = "";
    const [hours, minutes] = time.value.split(":").map(Number);
    const destination = { kind };
    if (kind === "local") destination.path = inputs.path.value;
    if (kind === "s3") Object.assign(destination, { endpoint: inputs.endpoint.value, bucket: inputs.bucket.value, prefix: inputs.prefix.value, region: inputs.region.value, accessKeyId: inputs.accessKeyId.value });
    if (kind === "sftp") Object.assign(destination, { host: inputs.host.value, port: Number(inputs.port.value) || 22, user: inputs.user.value, path: inputs.remotePath.value });
    const body = {
      accountPassword: account.value,
      config: {
        enabled: true,
        destination,
        frequency: frequency.value,
        minute: (hours || 0) * 60 + (minutes || 0),
        weekday: Number(weekday.value),
        keepDaily: Number(keepDaily.value) || 0,
        keepWeekly: Number(keepWeekly.value) || 0,
        keepMonthly: Number(keepMonthly.value) || 0,
        skipTrash: skipTrash.checked,
        excludes: excludes.value.split("\n"),
      },
    };
    if (repoPassword.value && !repoPassword.closest("label").hidden) body.password = repoPassword.value;
    if (kind === "s3" && inputs.s3Secret.value) body.s3Secret = inputs.s3Secret.value;
    save.disabled = true;
    save.textContent = "Saving and connecting…";
    try {
      const result = await ctx.api("/api/nas-backup/config", { method: "POST", json: body });
      account.value = "";
      if (result.newPassword) await showPassword(result.newPassword, true);
      const connected = result.connected;
      if (connected && !connected.ok) {
        await renderSettings(section);
        section.querySelector(".backup-form .form-error").textContent = `Saved, but the destination did not work: ${connected.error}`;
        return;
      }
      ctx.toast(connected?.created ? "Saved. A new, empty backup was set up there." : connected?.ok ? "Saved. The backup there opened with its password." : "Saved");
      await renderSettings(section);
    } catch (err) {
      error.textContent = err.message;
      save.disabled = false;
      save.textContent = status.configured ? "Save" : "Save and set up the backup";
    }
  });
  return form;
}

/// SFTP signs in with this computer's SSH key, never a password.
function sshKeyBox() {
  const box = el("div", { class: "backup-ssh" });
  if (status.sshKey) {
    const key = el("code", { class: "backup-command" }, status.sshKey);
    const copy = el("button", { type: "button", onclick: () => copyText(status.sshKey, "Key copied") }, "Copy");
    box.append(el("p", { class: "muted" }, "The other computer must let this one in with its SSH key: add this line to ", el("code", {}, "~/.ssh/authorized_keys"), " of that user there."), key, copy);
  } else {
    const make = el("button", { type: "button" }, "Make an SSH key");
    make.addEventListener("click", async () => {
      make.disabled = true;
      try {
        status.sshKey = (await ctx.api("/api/nas-backup/ssh-key", { method: "POST", json: {} })).sshKey;
        box.replaceWith(sshKeyBox());
      } catch (err) {
        ctx.toast(err.message, true);
        make.disabled = false;
      }
    });
    box.append(el("p", { class: "muted" }, "The other computer must let this one in with an SSH key, and this computer's account has none yet."), make);
  }
  return box;
}

async function copyText(text, done) {
  try {
    await navigator.clipboard.writeText(text);
    ctx.toast(done);
  } catch {
    ctx.toast("Could not copy. Select the text and copy it instead.", true);
  }
}

async function revealPassword() {
  const account = await ctx.askText("Show the backup's password", "Your account password", "", "Show", { password: true });
  if (!account) return;
  try {
    const { password } = await ctx.api("/api/nas-backup/password", { method: "POST", json: { accountPassword: account } });
    await showPassword(password, false);
  } catch (err) {
    ctx.toast(err.message, true);
  }
}

/// The backup's password, to write down. Without it, the backup cannot be read.
function showPassword(password, isNew) {
  return new Promise((resolve) => {
    const dialog = el("dialog", { class: "backup-password-dialog" });
    const form = el("form", { method: "dialog" },
      el("h3", {}, isNew ? "Write this password down" : "The backup's password"),
      el("p", {}, "Keep it somewhere safe, away from this computer, such as a password manager. If this computer is lost, you need it to restore the backup. Without it, nobody can."),
      el("code", { class: "backup-secret" }, password),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => copyText(password, "Password copied") }, "Copy"),
        el("button", { type: "submit", class: "primary" }, isNew ? "I've saved it" : "Done")));
    dialog.append(form);
    dialog.addEventListener("close", () => { dialog.remove(); resolve(); });
    document.body.append(dialog);
    dialog.showModal();
  });
}

async function renderSnapshots(box, fresh) {
  box.replaceChildren(el("h3", {}, "Snapshots"), el("p", { class: "muted" }, "Looking at the backup…"));
  let list;
  try {
    list = (await ctx.api(`/api/nas-backup/snapshots${fresh ? "?fresh=1" : ""}`)).snapshots;
  } catch (err) {
    box.replaceChildren(el("h3", {}, "Snapshots"), el("p", { class: "form-error" }, err.message));
    return;
  }
  const refresh = el("button", { type: "button", class: "link-button", onclick: () => renderSnapshots(box, true) }, "Refresh");
  box.replaceChildren(el("div", { class: "backup-snapshots-head" }, el("h3", {}, "Snapshots"), refresh));
  if (!list.length) {
    box.append(el("p", { class: "muted" }, "None yet. The first appears after the first backup."));
    return;
  }
  box.append(el("p", { class: "muted" }, `${list.length} kept. Restoring copies a folder from one into a new folder in the library; nothing is overwritten.`));
  const ul = el("ul", { class: "backup-snapshot-list" });
  for (const snap of list) {
    const date = new Date(snap.time);
    const facts = [];
    if (snap.totalFiles != null) facts.push(`${number(snap.totalFiles)} files`);
    if (snap.totalBytes != null) facts.push(size(snap.totalBytes));
    ul.append(el("li", {},
      el("span", {}, el("strong", {}, date.toLocaleString([], { dateStyle: "medium", timeStyle: "short" })), facts.length ? el("span", { class: "muted" }, ` · ${facts.join(" · ")}`) : null),
      el("button", { type: "button", onclick: () => restoreDialog(snap) }, "Restore…")));
  }
  box.append(ul);
}

/// Picks a folder in a snapshot to restore.
function restoreDialog(snap) {
  let path = "";
  const dialog = el("dialog", { class: "backup-restore-dialog" });
  const crumbs = el("nav", { class: "backup-crumbs", "aria-label": "Folder" });
  const list = el("ul", { class: "backup-browse" });
  const go = el("button", { type: "button", class: "primary" });
  const cancel = el("button", { type: "button", onclick: () => dialog.close() }, "Cancel");
  const when = new Date(snap.time).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
  dialog.append(
    el("h3", {}, `Restore from ${when}`),
    el("p", { class: "muted" }, `Open the folder to restore. It is copied into “${status.restoreFolder}” in the library, next to what is there now.`),
    crumbs, list, el("div", { class: "dialog-actions" }, cancel, go));
  dialog.addEventListener("close", () => dialog.remove());

  async function open(next) {
    path = next;
    crumbs.replaceChildren();
    const parts = path ? path.split("/") : [];
    crumbs.append(el("button", { type: "button", class: "link-button", onclick: () => open("") }, "Library"));
    parts.forEach((part, i) => crumbs.append(" / ", el("button", { type: "button", class: "link-button", onclick: () => open(parts.slice(0, i + 1).join("/")) }, part)));
    go.textContent = path ? `Restore “${parts[parts.length - 1]}”` : "Restore the whole library";
    list.replaceChildren(el("li", { class: "muted" }, "Loading…"));
    try {
      const { entries } = await ctx.api(`/api/nas-backup/browse?snapshot=${encodeURIComponent(snap.id)}&path=${encodeURIComponent(path)}`);
      list.replaceChildren();
      if (!entries.length) list.append(el("li", { class: "muted" }, "Empty folder"));
      for (const entry of entries) {
        list.append(entry.dir
          ? el("li", {}, el("button", { type: "button", class: "backup-dir", onclick: () => open(path ? `${path}/${entry.name}` : entry.name) }, `📁 ${entry.name}`))
          : el("li", { class: "backup-file" }, el("span", {}, entry.name), el("span", { class: "muted" }, size(entry.size))));
      }
    } catch (err) {
      list.replaceChildren(el("li", { class: "form-error" }, err.message));
    }
  }

  go.addEventListener("click", async () => {
    go.disabled = true;
    try {
      const result = await ctx.api("/api/nas-backup/restore", { method: "POST", json: { snapshot: snap.id, path } });
      dialog.close();
      ctx.toast(`Restoring into ${result.to}…`);
      status = await ctx.api("/api/nas-backup");
      section.querySelector(".backup-status")?.replaceWith(renderStatus());
      poll();
    } catch (err) {
      ctx.toast(err.message, true);
      go.disabled = false;
    }
  });
  document.body.append(dialog);
  dialog.showModal();
  open("");
}
