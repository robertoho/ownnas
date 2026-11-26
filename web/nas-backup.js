// Settings → Backup: backing up the library and OwnNAS's data with restic, to one or more
// destinations (a drive, an S3-compatible bucket, another computer over SFTP, Google Drive or
// another of rclone's remotes), each with its own schedule and keep rules; the snapshots, and
// restoring a folder from one. Loaded on demand by app.js.

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

/// The destination with this id, from the last status.
const destination = (id) => status.destinations.find((d) => d.id === id);
/// Which destination's form is open: an id, "new", or nothing.
let editing = null;

export async function renderSettings(target) {
  section = target;
  clearTimeout(timer);
  status = await ctx.api("/api/nas-backup");
  section.replaceChildren();
  section.append(el("p", { class: "muted" },
    "Back up the library and OwnNAS's own data (users, albums, tags, settings, and file history) to a drive, a bucket, Google Drive, or another computer. Backups are encrypted, and after the first one only what changed is sent. Two destinations, one at home and one far away, keep a copy safe even if the house loses both the server and the drive."));
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
  const list = status.destinations;
  if (editing && editing !== "new" && !destination(editing)) editing = null;
  if (!list.length) editing = "new";
  for (const dest of list) {
    section.append(editing === dest.id ? renderForm(dest) : renderStatus(dest));
  }
  if (editing === "new") {
    section.append(renderForm(null));
  } else if (list.length < status.maxDestinations) {
    section.append(el("div", { class: "dialog-actions backup-add" },
      el("button", { type: "button", onclick: () => { editing = "new"; renderSettings(section); } }, list.length === 1 ? "Add a second destination" : "Add a destination")));
  }
  if (list.length === 1 && editing !== "new") {
    section.append(el("p", { class: "muted backup-hint" }, "A second destination is a backup of its own, somewhere else (Google Drive, say, while this one is a drive at home), with its own schedule and keep rules."));
  }
  poll();
}

/// Keeps the status cards current while a backup runs (and now and then otherwise).
function poll() {
  clearTimeout(timer);
  if (!section || !status?.destinations.length) return;
  timer = setTimeout(async () => {
    if (!section.isConnected || section.hidden) return;
    try {
      const before = status.progress;
      status = await ctx.api("/api/nas-backup");
      for (const dest of status.destinations) {
        const card = section.querySelector(`.backup-status[data-id="${CSS.escape(dest.id)}"]`);
        if (card) {
          const open = card.querySelector(".backup-snapshots");
          const fresh = renderStatus(dest);
          card.replaceWith(fresh);
          // Something just finished there: its snapshots changed.
          if (open) showSnapshots(fresh, dest.id, Boolean(before && !status.progress && before.destination === dest.id));
        }
      }
      if (before && !status.progress) {
        const dest = destination(before.destination);
        const done = dest?.state;
        const to = dest && status.destinations.length > 1 ? ` (${dest.name})` : "";
        if (!done) {
          // Taken off the list meanwhile.
        } else if (before.action === "backup") {
          if (done.lastResult === "ok") ctx.toast(`Backup finished${to}`);
          else if (done.lastResult === "failed") ctx.toast(`The backup${to} failed: ${done.lastError}`, true);
        } else if (before.action === "restore" && done.lastRestore) {
          if (done.lastRestore.error) ctx.toast(`The restore failed: ${done.lastRestore.error}`, true);
          else ctx.toast(`Restored to ${done.lastRestore.to}`);
        } else if (before.action === "tidy") {
          ctx.toast(done.tidyError ? `Could not free space: ${done.tidyError}` : `Space freed at the destination${to}`, Boolean(done.tidyError));
        } else if (before.action === "verify") {
          ctx.toast(done.verifyResult === "ok" ? `The backup${to} checked out` : `The check found problems: ${done.verifyError}`, done.verifyResult !== "ok");
        }
      }
    } catch {}
    poll();
  }, status.progress ? 1500 : 20000);
}

function renderStatus(dest) {
  const { state, config } = dest;
  const running = status.progress;
  const progress = running && running.destination === dest.id ? running : null;
  const card = el("div", { class: "backup-card backup-status", "data-id": dest.id });
  const kind = config.destination.kind === "rclone" && config.destination.remoteType === "drive" ? "Google Drive" : null;
  card.append(el("div", { class: "backup-card-head" },
    el("h3", {}, dest.name || "Backup"),
    el("span", { class: "muted" }, kind && dest.name !== kind ? `${kind} · ${dest.where}` : dest.where),
    config.enabled && config.frequency !== "manual" ? null : el("span", { class: "backup-badge" }, config.enabled ? "By hand" : "Off")));
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
  if (result === "failed" && state.full) {
    card.append(el("p", { class: "muted" }, config.frequency === "manual"
      ? "It is not tried again on its own: once there is room, choose Back up now."
      : "It is not tried again in an hour, as other failures are: the next scheduled backup tries again, or choose Back up now once there is room."));
  }
  const facts = [];
  if (config.frequency === "manual") facts.push("Only when you choose Back up now");
  else if (dest.next) facts.push(`Next ${when(dest.next)}`);
  if (state.storedBytes) facts.push(`it takes ${size(state.storedBytes)} there`);
  if (dest.space) facts.push(`${size(dest.space.available)} free of ${size(dest.space.total)}`);
  if (facts.length) card.append(el("p", { class: "muted" }, `${facts.join(" · ")}.`));
  if (dest.space && dest.space.available < Math.max(dest.space.total * 0.05, 2e9)) {
    card.append(el("p", { class: "text-warning" }, `The backup drive is nearly full (${size(dest.space.available)} left). Make room there, keep fewer snapshots, or leave large folders out before it runs out.`));
  }
  if (state.lastVerified) {
    card.append(el("p", { class: state.verifyResult === "ok" ? "muted" : "text-warning" },
      state.verifyResult === "ok" ? `Last checked ${when(state.lastVerified)}: a sample read back correctly.` : `The check ${when(state.lastVerified)} found problems: ${state.verifyError}`));
  }
  if (state.lastRestore && !state.lastRestore.error) {
    const open = el("button", { type: "button", class: "link-button", onclick: () => ctx.goToFolder(state.lastRestore.to) }, state.lastRestore.to);
    card.append(el("p", { class: "muted" }, `Last restore ${when(state.lastRestore.finished)} went to `, open));
  }
  if (progress) {
    const label = { backup: "Backing up", restore: "Restoring", verify: "Checking the backup", connect: "Connecting", tidy: "Freeing space" }[progress.action] || progress.action;
    const bar = el("span");
    bar.style.width = `${Math.round((progress.percent || 0) * 100)}%`;
    const bits = [];
    if (progress.totalFiles) bits.push(`${number(progress.filesDone)} of ${number(progress.totalFiles)} files`);
    if (progress.totalBytes) bits.push(`${size(progress.bytesDone)} of ${size(progress.totalBytes)}`);
    if (progress.secondsRemaining) bits.push(`about ${duration(progress.secondsRemaining)} left`);
    if (config.gentle && progress.action !== "connect") bits.push("going easy on this computer");
    card.append(el("div", { class: "backup-progress" },
      el("p", {}, el("strong", {}, label), ` · ${progress.phase}`),
      el("div", { class: "backup-bar", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(Math.round((progress.percent || 0) * 100)) }, bar),
      bits.length ? el("p", { class: "muted" }, bits.join(" · ")) : null,
      progress.current ? el("p", { class: "muted backup-current" }, progress.current) : null));
  }
  if (running && !progress) {
    const other = destination(running.destination);
    card.append(el("p", { class: "muted" }, `Waits for ${other ? `“${other.name}”` : "the other destination"} to finish: one backup runs at a time.`));
  }
  const actions = el("div", { class: "dialog-actions backup-actions" });
  if (progress) {
    actions.append(el("button", { type: "button", onclick: stop }, "Stop"));
  } else {
    actions.append(
      el("button", { type: "button", class: "primary", disabled: Boolean(running), onclick: () => start("run", dest.id) }, "Back up now"),
      el("button", { type: "button", disabled: Boolean(running), onclick: () => start("verify", dest.id), title: "Reads back 5% of the backup to check it is complete and intact" }, "Check the backup"),
    );
    if (state.full) {
      actions.append(el("button", { type: "button", disabled: Boolean(running), onclick: () => start("tidy", dest.id),
        title: "Removes the snapshots past the keep rules (after you lower them) and what unfinished backups left at the destination" }, "Free up space"));
    }
  }
  actions.append(
    el("button", { type: "button", onclick: () => { const open = card.querySelector(".backup-snapshots"); if (open) open.remove(); else showSnapshots(card, dest.id, false); } }, "Snapshots"),
    el("button", { type: "button", onclick: () => { editing = dest.id; renderSettings(section); } }, "Settings"),
  );
  card.append(actions);
  return card;
}

/// The snapshots of a destination, under its card.
function showSnapshots(card, id, fresh) {
  const box = el("div", { class: "backup-snapshots" });
  card.append(box);
  renderSnapshots(box, fresh, id);
}

async function start(what, id) {
  try {
    await ctx.api(`/api/nas-backup/${what}`, { method: "POST", json: { id } });
    await renderSettings(section);
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

/// The settings of a destination, or of a new one (`dest` null).
function renderForm(dest) {
  const config = dest?.config || { enabled: true, destination: {}, frequency: "daily", minute: 180, weekday: 0, keepDaily: 7, keepWeekly: 4, keepMonthly: 12, skipTrash: true, excludes: [], gentle: true };
  const d = config.destination;
  const others = status.destinations.filter((other) => other.id !== dest?.id);
  const form = el("form", { class: `user-form backup-form${status.destinations.length ? " backup-card" : ""}` });
  const kinds = [["local", "Drive or folder"], ["gdrive", "Google Drive"], ["s3", "S3-compatible bucket"], ["sftp", "Another computer (SFTP)"], ["rclone", "Another rclone remote"]];
  // A second destination is usually somewhere else than the first: a cloud when that is a drive.
  let kind = d.kind === "rclone" && d.remoteType === "drive" ? "gdrive" : d.kind || (others.some((other) => other.config.destination.kind === "local") ? "gdrive" : "local");
  const name = el("input", { autocomplete: "off", value: dest?.name || "", placeholder: "Such as USB drive, or Google Drive", maxlength: "60" });
  if (status.destinations.length) {
    form.append(el("h3", {}, dest ? `Settings of “${dest.name}”` : "A new destination"));
  }

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
    s3Secret: el("input", { type: "password", autocomplete: "new-password", placeholder: dest?.s3SecretSet ? "Saved. Leave empty to keep it" : "" }),
    host: el("input", { autocomplete: "off", spellcheck: "false", value: d.host, placeholder: "backup.example.com" }),
    port: el("input", { type: "number", min: "1", max: "65535", value: d.port || 22 }),
    user: el("input", { autocomplete: "off", spellcheck: "false", value: d.user }),
    remotePath: el("input", { autocomplete: "off", spellcheck: "false", value: d.kind === "sftp" ? d.path : "", placeholder: "/srv/backups/ownnas" }),
    remote: el("select", {}, d.remote ? el("option", { value: d.remote }, d.remote) : null),
    remoteFolder: el("input", { autocomplete: "off", spellcheck: "false", value: d.kind === "rclone" ? d.path : "OwnNAS backup", placeholder: "backups/ownnas" }),
    drive: el("select", {}, d.remote && d.remoteType === "drive" ? el("option", { value: d.remote }, d.remote) : null),
    driveFolder: el("input", { autocomplete: "off", spellcheck: "false", value: d.kind === "rclone" ? d.path : "OwnNAS backup", placeholder: "OwnNAS backup" }),
  };
  inputs.remote.value = d.remote || "";
  inputs.drive.value = d.remoteType === "drive" ? d.remote : "";
  /// rclone's remotes (OneDrive, Nextcloud, another server…), to pick one for the backups.
  const remotesBox = () => {
    const box = el("p", { class: "muted backup-hint" }, "Looking for rclone's remotes…");
    ctx.api("/api/remotes").then((data) => {
      if (!data.available) {
        box.textContent = "rclone is not installed on the server. Install it and set up a remote with: rclone config";
        return;
      }
      if (data.error) {
        box.textContent = data.error;
        return;
      }
      const chosen = inputs.remote.value;
      inputs.remote.replaceChildren(el("option", { value: "" }, "Choose a remote"), ...data.remotes.map((remote) => el("option", { value: remote.name }, `${remote.name}${remote.type ? ` (${remote.type})` : ""}`)));
      inputs.remote.value = chosen;
      box.textContent = data.remotes.length
        ? `restic writes through rclone, with the remotes in ${data.config || "rclone's config file"}.`
        : "rclone has no remotes yet. Set one up on the server with: rclone config";
    }).catch((err) => { box.textContent = err.message; });
    return box;
  };
  /// The drives plugged in now: one click puts a backup folder on it.
  const drivesBox = () => {
    const box = el("div", { class: "backup-drives" }, el("p", { class: "muted" }, "Looking for drives…"));
    const size = (bytes) => {
      const units = ["B", "KB", "MB", "GB", "TB"];
      let value = bytes;
      let unit = 0;
      while (value >= 1000 && unit < units.length - 1) { value /= 1000; unit += 1; }
      return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
    };
    const paint = async () => {
      try {
        const { drives } = await ctx.api("/api/drives");
        box.replaceChildren(
          el("div", { class: "backup-drives-head" },
            el("span", { class: "muted" }, drives.length ? "Drives plugged in:" : "No other drives are plugged in. Plug one in and look again."),
            el("button", { type: "button", class: "backup-drives-refresh", onclick: () => { box.replaceChildren(el("p", { class: "muted" }, "Looking for drives…")); paint(); } }, "Look again")),
          ...drives.map((drive) => el("button", {
            type: "button",
            class: `backup-drive${drive.holdsLibrary ? " holds-library" : ""}`,
            title: drive.path,
            onclick: () => {
              inputs.path.value = `${drive.path.replace(/[\\/]+$/, "")}${drive.path.includes("\\") ? "\\" : "/"}OwnNAS backup`;
              inputs.path.focus();
            },
          },
          el("strong", {}, `${drive.removable ? "💾" : "🗄"} ${drive.name}`),
          el("span", { class: "muted" }, `${size(drive.available)} free of ${size(drive.total)}${drive.filesystem ? ` · ${drive.filesystem}` : ""}`),
          drive.holdsLibrary ? el("span", { class: "backup-drive-warning" }, "The library is on this drive") : null)));
      } catch (err) {
        box.replaceChildren(el("p", { class: "muted" }, err.message));
      }
    };
    paint();
    return box;
  };
  const showKind = () => {
    for (const button of kindPicker.children) button.setAttribute("aria-checked", button.dataset.kind === kind ? "true" : "false");
    pane.replaceChildren();
    if (kind === "local") {
      pane.append(field("Folder", inputs.path, "The full path of a folder on a drive this computer can write to, such as a USB disk. Keep it on a different drive from the library."), drivesBox());
    } else if (kind === "s3") {
      pane.append(
        field("Endpoint", inputs.endpoint, "AWS: s3.amazonaws.com. Backblaze B2: s3.<region>.backblazeb2.com. Cloudflare R2: <account>.r2.cloudflarestorage.com. MinIO and others: https://host:port."),
        el("div", { class: "backup-row" }, field("Bucket", inputs.bucket), field("Folder in the bucket", inputs.prefix)),
        field("Region", inputs.region),
        el("div", { class: "backup-row" }, field("Access key ID", inputs.accessKeyId), field("Secret access key", inputs.s3Secret)),
      );
    } else if (kind === "rclone") {
      pane.append(
        el("div", { class: "backup-row" }, field("Remote", inputs.remote), field("Folder on it", inputs.remoteFolder)),
        remotesBox(),
      );
    } else if (kind === "gdrive") {
      pane.append(driveBox(inputs));
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
  form.append(el("h3", {}, "Where"), kindPicker, pane, field("Its name on this page", name));

  // The password.
  const passwordBox = el("div", { class: "backup-password" });
  const repoPassword = el("input", { type: "password", autocomplete: "off", spellcheck: "false" });
  if (!dest?.passwordSet) {
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
        el("button", { type: "button", class: "link-button", onclick: () => revealPassword(dest.id) }, "Show the password"), " · ", change),
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

  // How hard.
  const gentle = el("input", { type: "checkbox" });
  gentle.checked = config.gentle !== false;
  form.append(el("h3", {}, "While it runs"),
    el("label", { class: "check" }, gentle, " Go easy on this computer"),
    el("p", { class: "muted" }, "restic runs at low priority, on half the processor and reading one file at a time, and OwnNAS's thumbnails and photo and text indexing wait until it ends. Backups take longer, and OwnNAS stays quick. Turn it off to back up as fast as possible."));

  const enabled = el("input", { type: "checkbox" });
  enabled.checked = config.enabled !== false;
  form.append(el("label", { class: "check" }, enabled, " Back up to this destination (turn off to pause it, keeping its settings and snapshots)"));

  const account = el("input", { type: "password", autocomplete: "current-password" });
  form.append(field("Your account password", account, "Needed when you change where backups go or their password."));
  const error = el("p", { class: "form-error", role: "alert" });
  const save = el("button", { type: "submit", class: "primary" }, dest ? "Save" : "Save and set up the backup");
  const buttons = el("div", { class: "dialog-actions" });
  if (dest) buttons.append(el("button", { type: "button", class: "danger backup-remove", onclick: () => removeDestination(dest) }, "Remove…"));
  if (status.destinations.length) buttons.append(el("button", { type: "button", onclick: () => { editing = null; renderSettings(section); } }, "Cancel"));
  buttons.append(save);
  form.append(buttons, error);

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    error.textContent = "";
    const [hours, minutes] = time.value.split(":").map(Number);
    const destination = { kind };
    if (kind === "local") destination.path = inputs.path.value;
    if (kind === "s3") Object.assign(destination, { endpoint: inputs.endpoint.value, bucket: inputs.bucket.value, prefix: inputs.prefix.value, region: inputs.region.value, accessKeyId: inputs.accessKeyId.value });
    if (kind === "sftp") Object.assign(destination, { host: inputs.host.value, port: Number(inputs.port.value) || 22, user: inputs.user.value, path: inputs.remotePath.value });
    if (kind === "rclone") Object.assign(destination, { remote: inputs.remote.value, path: inputs.remoteFolder.value });
    if (kind === "gdrive") {
      if (!inputs.drive.value) {
        error.textContent = "Connect a Google account first, or choose one.";
        return;
      }
      Object.assign(destination, { kind: "rclone", remote: inputs.drive.value, path: inputs.driveFolder.value });
    }
    const body = {
      id: dest?.id,
      accountPassword: account.value,
      config: {
        name: name.value,
        enabled: enabled.checked,
        destination,
        frequency: frequency.value,
        minute: (hours || 0) * 60 + (minutes || 0),
        weekday: Number(weekday.value),
        keepDaily: Number(keepDaily.value) || 0,
        keepWeekly: Number(keepWeekly.value) || 0,
        keepMonthly: Number(keepMonthly.value) || 0,
        skipTrash: skipTrash.checked,
        excludes: excludes.value.split("\n"),
        gentle: gentle.checked,
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
        editing = result.id;
        await renderSettings(section);
        const shown = section.querySelector(".backup-form .form-error");
        if (shown) shown.textContent = `Saved, but the destination did not work: ${connected.error}`;
        return;
      }
      ctx.toast(connected?.created ? "Saved. A new, empty backup was set up there." : connected?.ok ? "Saved. The backup there opened with its password." : "Saved");
      editing = null;
      await renderSettings(section);
    } catch (err) {
      error.textContent = err.message;
      save.disabled = false;
      save.textContent = dest ? "Save" : "Save and set up the backup";
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

async function revealPassword(id) {
  const account = await ctx.askText("Show the backup's password", "Your account password", "", "Show", { password: true });
  if (!account) return;
  try {
    const { password } = await ctx.api("/api/nas-backup/password", { method: "POST", json: { id, accountPassword: account } });
    await showPassword(password, false);
  } catch (err) {
    ctx.toast(err.message, true);
  }
}

/// Takes a destination off the list; the backup at it stays, readable with its password.
async function removeDestination(dest) {
  const sure = await ctx.askConfirm(`Stop backing up to “${dest.name}” and take it off this page? The snapshots already there stay where they are, and you can restore from them again later by adding the same destination with its password. Make sure you have that password written down (Show the password) before you go on.`, "Remove");
  if (!sure) return;
  const account = await ctx.askText(`Remove “${dest.name}”`, "Your account password", "", "Remove", { password: true });
  if (!account) return;
  try {
    await ctx.api("/api/nas-backup/remove", { method: "POST", json: { id: dest.id, accountPassword: account } });
    editing = null;
    ctx.toast(`“${dest.name}” removed. Its backup is still there.`);
    await renderSettings(section);
  } catch (err) {
    ctx.toast(err.message, true);
  }
}

/// The rclone command that signs in to Google on a computer with a browser, for the scope (and
/// client) given: rclone takes its settings as URL-safe base64 of JSON.
function authorizeCommand(clientId, clientSecret) {
  const settings = { scope: "drive.file" };
  if (clientId && clientSecret) Object.assign(settings, { client_id: clientId, client_secret: clientSecret });
  const blob = btoa(JSON.stringify(settings)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return `rclone authorize "drive" "${blob}"`;
}

/// Google Drive: a Google account connected through rclone, and the folder there. A server has
/// no browser to sign in with, so the sign-in happens with rclone on another computer, and what
/// it prints is pasted here.
function driveBox(inputs) {
  const box = el("div", { class: "backup-where" });
  const note = el("p", { class: "muted backup-hint" }, "Looking for Google accounts…");
  const connectBox = el("div", { class: "backup-connect" });
  const accounts = field("Google account", inputs.drive);
  box.append(el("div", { class: "backup-row" }, accounts, field("Folder in Google Drive", inputs.driveFolder)), note, connectBox);
  let drives = [];
  let taken = [];

  const freeName = () => {
    for (let n = 1; ; n += 1) {
      const name = n === 1 ? "gdrive" : `gdrive${n}`;
      if (!taken.includes(name)) return name;
    }
  };

  const showConnect = (reconnect) => {
    const clientId = el("input", { autocomplete: "off", spellcheck: "false", placeholder: "…apps.googleusercontent.com" });
    const clientSecret = el("input", { type: "password", autocomplete: "off", spellcheck: "false" });
    const command = el("code", { class: "backup-command" }, authorizeCommand());
    const update = () => { command.textContent = authorizeCommand(clientId.value.trim(), clientSecret.value.trim()); };
    clientId.addEventListener("input", update);
    clientSecret.addEventListener("input", update);
    const pasted = el("textarea", { rows: "4", spellcheck: "false", placeholder: "Paste here everything between ---> and <---End paste" });
    const remoteName = el("input", { autocomplete: "off", spellcheck: "false", value: reconnect || freeName(), readonly: Boolean(reconnect) });
    const problem = el("p", { class: "form-error", role: "alert" });
    const connect = el("button", { type: "button", class: "primary" }, reconnect ? "Sign in again" : "Connect");
    const own = el("details", { class: "backup-own-client" },
      el("summary", {}, "Use a Google API client of your own (optional)"),
      el("p", { class: "muted" }, "rclone's own client is shared by everyone who uses rclone, and Google slows it down when it is busy. One of your own keeps backups quick: in Google Cloud Console, make a project, turn on the Google Drive API, set up the OAuth consent screen, and create an OAuth client ID of the kind “Desktop app”. Then enter it here before you run the command."),
      el("div", { class: "backup-row" }, field("Client ID", clientId), field("Client secret", clientSecret)));
    connect.addEventListener("click", async () => {
      problem.textContent = "";
      connect.disabled = true;
      connect.textContent = "Connecting…";
      try {
        const result = await ctx.api("/api/remotes/google-drive", { method: "POST", json: { name: remoteName.value, token: pasted.value, clientId: clientId.value, clientSecret: clientSecret.value } });
        ctx.toast(result.created ? `Google Drive connected as “${result.name}”` : `“${result.name}” is signed in again`);
        connectBox.replaceChildren();
        await load(result.name);
      } catch (err) {
        problem.textContent = err.message;
        connect.disabled = false;
        connect.textContent = reconnect ? "Sign in again" : "Connect";
      }
    });
    connectBox.replaceChildren(el("div", { class: "backup-card backup-drive-connect" },
      el("p", {}, el("strong", {}, reconnect ? `Sign “${reconnect}” in to Google again` : "Connect a Google account")),
      el("ol", { class: "backup-steps" },
        el("li", {}, el("span", {}, "On a computer with a web browser (this one, if it is not the server), install rclone from ",
          el("a", { href: "https://rclone.org/downloads/", target: "_blank", rel: "noopener" }, "rclone.org/downloads"), " if it is not there yet.")),
        el("li", {}, el("span", {}, "Open a terminal there and run this command. A browser window opens: sign in to Google and allow access."), command,
          el("button", { type: "button", onclick: () => copyText(command.textContent, "Command copied") }, "Copy")),
        el("li", {}, el("span", {}, "rclone then prints a long text between arrows. Copy it and paste it here:"), pasted)),
      own,
      field("Name in rclone", remoteName, "How this account is called among rclone's remotes (and under Remotes)."),
      el("p", { class: "muted backup-hint" }, "OwnNAS asks Google only for the files it puts in your Drive itself: it cannot see or change anything else there. Backups are encrypted before they leave this computer, so Google only ever has scrambled data."),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => connectBox.replaceChildren(drives.length ? links() : el("span")) }, "Cancel"),
        connect),
      problem));
  };

  const links = () => el("p", { class: "muted backup-hint" },
    el("button", { type: "button", class: "link-button", onclick: () => showConnect(null) }, "Connect another Google account"),
    inputs.drive.value ? [" · ", el("button", { type: "button", class: "link-button", onclick: () => showConnect(inputs.drive.value) }, `Sign “${inputs.drive.value}” in again`)] : null);

  async function load(chosen) {
    try {
      const data = await ctx.api("/api/remotes");
      if (!data.available) {
        accounts.hidden = true;
        note.replaceChildren("Google Drive backups go through rclone, which is not installed on the server. Install it there (",
          el("code", {}, "sudo apt install rclone"), " or from ", el("a", { href: "https://rclone.org/downloads/", target: "_blank", rel: "noopener" }, "rclone.org/downloads"), "), then come back to this page.");
        return;
      }
      if (data.error) {
        note.textContent = data.error;
        return;
      }
      taken = data.remotes.map((remote) => remote.name);
      drives = data.remotes.filter((remote) => remote.type === "drive");
      const keep = chosen || inputs.drive.value;
      inputs.drive.replaceChildren(...drives.map((remote) => el("option", { value: remote.name }, remote.name)));
      inputs.drive.value = drives.some((remote) => remote.name === keep) ? keep : drives[0]?.name || "";
      accounts.hidden = !drives.length;
      note.replaceChildren(drives.length
        ? "restic writes the encrypted backup there through rclone. Google takes up to 750 GB a day from one account, so a large first backup takes some days: each run carries on where the last one stopped."
        : "No Google account is connected yet.");
      if (!drives.length) showConnect(null);
      else if (!connectBox.querySelector(".backup-drive-connect")) connectBox.replaceChildren(links());
    } catch (err) {
      note.textContent = err.message;
    }
  }
  inputs.drive.addEventListener("change", () => { if (!connectBox.querySelector(".backup-drive-connect")) connectBox.replaceChildren(links()); });
  load(null);
  return box;
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

async function renderSnapshots(box, fresh, id) {
  box.replaceChildren(el("h3", {}, "Snapshots"), el("p", { class: "muted" }, "Looking at the backup…"));
  let list;
  try {
    list = (await ctx.api(`/api/nas-backup/snapshots?id=${encodeURIComponent(id)}${fresh ? "&fresh=1" : ""}`)).snapshots;
  } catch (err) {
    box.replaceChildren(el("h3", {}, "Snapshots"), el("p", { class: "form-error" }, err.message));
    return;
  }
  const refresh = el("button", { type: "button", class: "link-button", onclick: () => renderSnapshots(box, true, id) }, "Refresh");
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
      el("button", { type: "button", onclick: () => restoreDialog(snap, id) }, "Restore…")));
  }
  box.append(ul);
}

/// Picks a folder in a snapshot to restore.
function restoreDialog(snap, id) {
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
      const { entries } = await ctx.api(`/api/nas-backup/browse?id=${encodeURIComponent(id)}&snapshot=${encodeURIComponent(snap.id)}&path=${encodeURIComponent(path)}`);
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
      const result = await ctx.api("/api/nas-backup/restore", { method: "POST", json: { id, snapshot: snap.id, path } });
      dialog.close();
      ctx.toast(`Restoring into ${result.to}…`);
      await renderSettings(section);
    } catch (err) {
      ctx.toast(err.message, true);
      go.disabled = false;
    }
  });
  document.body.append(dialog);
  dialog.showModal();
  open("");
}
