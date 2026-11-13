//! Tools for keys at the "write" level: making text files, changing them, adding rows to tables,
//! and copying. Like the organising tools (`mcp_organise.rs`, which runs these too, with
//! approval and undo), they work only inside the key's folders. Before a file is changed, what
//! it held is kept in its history (`versions::keep_for_agent`), so every change can be undone;
//! a file or copy the agent made is undone by moving it to the Trash.

use crate::agent_keys::{self, AgentKey};
use crate::files;
use crate::http::AppState;
use crate::mcp::{allowed, blocking, failed, Args, Stop};
use crate::mcp_organise::{done, file_error, Done};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// The largest text an agent can write in one go, and the largest file it can change.
const MOST_TEXT: usize = 5 * 1024 * 1024;
/// The largest file an agent can upload in one call (base64 makes the request a third bigger).
const MOST_UPLOAD: usize = 25 * 1024 * 1024;
const MOST_ROWS: usize = 10_000;

pub fn tools() -> Vec<Value> {
    let changes = json!({ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false });
    let path = |about: &str| json!({ "type": "string", "description": about });
    vec![
        json!({
            "name": "write_file",
            "title": "Write a text file",
            "description": "Makes a new text file (Markdown, plain text, CSV, JSON, code…) with the given text. To replace a file that is there, pass replace: true; what it held is kept in its history.",
            "inputSchema": { "type": "object", "required": ["path", "text"], "properties": {
                "path": path("The file to write, with its folder and name, such as Notes/summary.md."),
                "text": { "type": "string", "description": "Everything the file should hold." },
                "replace": { "type": "boolean", "description": "Replace the file if it is there. Leave out to only make new files." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "edit_text",
            "title": "Change a passage",
            "description": "Replaces one exact passage of a text file, or of a Word (docx) or OpenDocument (odt) document, with new text. The passage must appear exactly once: include enough around it to make it unique (in a document, within one paragraph). What the file held is kept in its history.",
            "inputSchema": { "type": "object", "required": ["path", "find", "replace"], "properties": {
                "path": path("The text file."),
                "find": { "type": "string", "description": "The exact text to replace, as it is in the file." },
                "replace": { "type": "string", "description": "The new text (empty to take the passage out)." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "append_rows",
            "title": "Add rows to a table",
            "description": "Adds rows after the last row of a CSV or TSV file, or of a sheet of a spreadsheet (xlsx). Numbers stay numbers. What the file held is kept in its history.",
            "inputSchema": { "type": "object", "required": ["path", "rows"], "properties": {
                "path": path("The CSV, TSV or xlsx file."),
                "sheet": { "type": "string", "description": "For a spreadsheet: the sheet's name. Leave out for the first." },
                "rows": { "type": "array", "maxItems": MOST_ROWS, "items": { "type": "array", "items": { "type": ["string", "number", "boolean", "null"] } }, "description": "The rows, each a list of cells." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "create_note",
            "title": "Make a note",
            "description": "Makes a note in the user's notes (a Markdown file in their notes folder, shown as a card in OwnNAS Notes): a title, text, and checklist items. Use #words in the text as labels and [[Note name]] to link to another note.",
            "inputSchema": { "type": "object", "properties": {
                "title": { "type": "string", "description": "The note's title." },
                "text": { "type": "string", "description": "Its text, in Markdown." },
                "checklist": { "type": "array", "items": { "type": "string" }, "maxItems": 500, "description": "Checklist items, unticked." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "append_to_note",
            "title": "Add to a note",
            "description": "Adds text (or checklist lines such as \"- [ ] milk\") to the end of a note.",
            "inputSchema": { "type": "object", "required": ["path", "text"], "properties": {
                "path": path("The note, from list_notes."),
                "text": { "type": "string", "description": "What to add, in Markdown." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "tick_item",
            "title": "Tick a checklist item",
            "description": "Ticks a checklist item in a note (or clears it with done: false), found by its text.",
            "inputSchema": { "type": "object", "required": ["path", "item"], "properties": {
                "path": path("The note."),
                "item": { "type": "string", "description": "The item's text, as list_notes or read_note give it." },
                "done": { "type": "boolean", "description": "Ticked (the default) or not." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "add_paragraphs",
            "title": "Add paragraphs",
            "description": "Adds paragraphs at the end of a Word (docx) or OpenDocument (odt) document, or of a text or Markdown file. The document's look and everything else in it stay as they were; what it held is kept in its history.",
            "inputSchema": { "type": "object", "required": ["path", "paragraphs"], "properties": {
                "path": path("The document."),
                "paragraphs": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 1000, "description": "The paragraphs, as plain text." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "upload_file",
            "title": "Upload a file",
            "description": "Saves a file of any kind (a picture, PDF, spreadsheet…) from base64, up to 25 MB, in a folder. A name already taken gets a number added, unless replace is true; what a replaced file held is kept in its history.",
            "inputSchema": { "type": "object", "required": ["path", "name", "base64"], "properties": {
                "path": path("The folder to save it in."),
                "name": { "type": "string", "description": "The file's name, with its extension." },
                "base64": { "type": "string", "description": "The file's contents, base64-encoded." },
                "replace": { "type": "boolean", "description": "Replace a file with that name." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "copy",
            "title": "Copy",
            "description": "Copies a file or folder into a folder. A name already taken there gets a number added. Returns the copy's path.",
            "inputSchema": { "type": "object", "required": ["path", "to"], "properties": {
                "path": path("The file or folder to copy."),
                "to": path("The folder to copy it into."),
            } },
            "annotations": changes,
        }),
    ]
}

pub fn has(name: &str) -> bool {
    tools().iter().any(|tool| tool["name"] == name)
}

pub(crate) async fn run(state: &Arc<AppState>, key: &AgentKey, name: &str, args: &Args<'_>) -> Result<Done, Stop> {
    let replace_text = args.raw().get("replace").and_then(Value::as_str).map(str::to_string);
    match name {
        "write_file" => write_file(state, key, &args.path(), &args.text("text"), args.raw().get("replace").and_then(Value::as_bool).unwrap_or(false)).await,
        "edit_text" => edit_text(state, key, &args.path(), &args.text("find"), replace_text.as_deref()).await,
        "append_rows" if args.path().to_ascii_lowercase().ends_with(".xlsx") => append_sheet_rows(state, key, &args.path(), &args.text("sheet"), args.raw().get("rows")).await,
        "append_rows" => append_rows(state, key, &args.path(), args.raw().get("rows")).await,
        "add_paragraphs" => add_paragraphs(state, key, &args.path(), &args.texts("paragraphs")).await,
        "upload_file" => upload_file(state, key, &args.path(), &args.text("name"), &args.text("base64"), args.raw().get("replace").and_then(Value::as_bool).unwrap_or(false)).await,
        "copy" => copy(state, key, &args.path(), &args.text("to")).await,
        "create_note" => create_note(state, key, &args.text("title"), &args.text("text"), &args.texts("checklist")).await,
        "append_to_note" => append_to_note(state, key, &args.path(), &args.text("text")).await,
        "tick_item" => tick_item(state, key, &args.path(), &args.text("item"), args.raw().get("done").and_then(Value::as_bool).unwrap_or(true)).await,
        _ => failed("Unknown tool"),
    }
}

/// Keeps what a file holds before it changes; the version's id (`None` for an empty file).
fn keep(state: &AppState, key: &AgentKey, full: &Path, rel: &str) -> Result<Option<i64>, Stop> {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    crate::versions::keep_for_agent(&conn, &state.versions, full, rel, &format!("agent: {}", key.name))
        .map_err(|_| Stop::Failed("Could not keep a copy of the file before changing it, so it was left as it is.".to_string()))
}

fn after_write(state: &AppState, key: &AgentKey, action: &str, rel: &str) {
    crate::http::invalidate_sizes(state, rel.rsplit_once('/').map_or("", |(parent, _)| parent));
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = crate::db::log_event(&conn, None, &format!("agent: {}", key.name), action, rel);
}

/// A text file the key may change: allowed, a file, and small enough to work on.
async fn text_file(state: &Arc<AppState>, key: &AgentKey, path: &str) -> Result<(agent_keys::Allowed, String), Stop> {
    let found = allowed(state, key, path).await?;
    let full = found.full.clone();
    let text = blocking(move || {
        if full.is_dir() {
            return failed("That is a folder.");
        }
        let size = std::fs::metadata(&full).map_err(|_| Stop::Refused)?.len();
        if size as usize > MOST_TEXT {
            return failed("That file is too large to change here.");
        }
        let bytes = std::fs::read(&full).map_err(|_| Stop::Failed("Could not read that file.".to_string()))?;
        if bytes.contains(&0) {
            return failed("That file is not text. Only text files can be changed here.");
        }
        String::from_utf8(bytes).map_err(|_| Stop::Failed("That file is not UTF-8 text. Only text files can be changed here.".to_string()))
    })
    .await?;
    Ok((found, text))
}

/// Writes `text` over a file that exists, keeping what it held first.
async fn overwrite(state: &Arc<AppState>, key: &AgentKey, found: &agent_keys::Allowed, text: String) -> Result<Option<i64>, Stop> {
    let version = keep(state, key, &found.full, &found.rel)?;
    let full = found.full.clone();
    blocking(move || std::fs::write(&full, text).map_err(|_| Stop::Failed("Could not write the file.".to_string()))).await?;
    Ok(version)
}

async fn write_file(state: &Arc<AppState>, key: &AgentKey, path: &str, text: &str, replace: bool) -> Result<Done, Stop> {
    if text.len() > MOST_TEXT {
        return failed("That is too much text for one file here (5 MB at most).");
    }
    if text.contains('\0') {
        return failed("Only text can be written.");
    }
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name == ".." {
        return failed("Give the file a name that does not start with a dot.");
    }
    if !key.allows_name(name) {
        return failed("This key can only make files with certain extensions.");
    }
    let folder = allowed(state, key, parent).await?;
    if !folder.full.is_dir() {
        return failed("Its folder is not there. Make it first with create_folder.");
    }
    let target = folder.full.join(name);
    let rel = if folder.rel.is_empty() { name.to_string() } else { format!("{}/{name}", folder.rel) };
    if target.exists() || target.is_symlink() {
        if !replace {
            return failed("There is a file with that name already. Pass replace: true to replace it, or use edit_text.");
        }
        let (found, _) = text_file(state, key, &rel).await?;
        let version = overwrite(state, key, &found, text.to_string()).await?;
        after_write(state, key, "agent-write", &found.rel);
        return done(json!({ "path": found.rel, "replaced": true }), json!({ "kind": "version", "path": found.rel, "version": version }));
    }
    let (full, body) = (target.clone(), text.to_string());
    blocking(move || {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&full).map_err(|_| Stop::Failed("Could not make the file.".to_string()))?;
        file.write_all(body.as_bytes()).map_err(|_| Stop::Failed("Could not write the file.".to_string()))
    })
    .await?;
    // Made through a link, it could be somewhere else: then it goes again.
    if allowed(state, key, &rel).await.is_err() {
        let _ = std::fs::remove_file(&target);
        return Err(Stop::Refused);
    }
    after_write(state, key, "agent-create", &rel);
    done(json!({ "path": rel, "replaced": false }), json!({ "kind": "created", "path": rel }))
}

async fn edit_text(state: &Arc<AppState>, key: &AgentKey, path: &str, find: &str, replace: Option<&str>) -> Result<Done, Stop> {
    if find.is_empty() {
        return failed("Give the exact passage to replace.");
    }
    let Some(replace) = replace else { return failed("Give the new text (an empty one takes the passage out).") };
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".docx") || lower.ends_with(".odt") {
        let (found, bytes) = binary_file(state, key, path).await?;
        let (find, replace, word) = (find.to_string(), replace.to_string(), lower.ends_with(".docx"));
        let changed = blocking(move || if word { crate::office_edit::docx_replace(&bytes, &find, &replace) } else { crate::office_edit::odt_replace(&bytes, &find, &replace) }.map_err(Stop::Failed)).await?;
        let version = overwrite_bytes(state, key, &found, changed).await?;
        after_write(state, key, "agent-edit", &found.rel);
        return done(json!({ "path": found.rel, "changed": true }), json!({ "kind": "version", "path": found.rel, "version": version }));
    }
    let (found, text) = text_file(state, key, path).await?;
    let times = text.matches(find).count();
    if times == 0 {
        return failed("That passage is not in the file. Copy it exactly, with its spaces and line breaks, from read_text.");
    }
    if times > 1 {
        return failed(&format!("That passage is in the file {times} times: include more of the text around it, so it is there only once."));
    }
    let changed = text.replacen(find, replace, 1);
    if changed.len() > MOST_TEXT {
        return failed("The file would be too large.");
    }
    let version = overwrite(state, key, &found, changed).await?;
    after_write(state, key, "agent-edit", &found.rel);
    done(json!({ "path": found.rel, "changed": true }), json!({ "kind": "version", "path": found.rel, "version": version }))
}

fn cell_text(cell: &Value) -> String {
    match cell {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A row as CSV or TSV, quoting the cells that need it.
fn delimited(row: &[Value], separator: char) -> String {
    row.iter()
        .map(|cell| {
            let text = cell_text(cell);
            if text.contains(separator) || text.contains('"') || text.contains('\n') || text.contains('\r') {
                format!("\"{}\"", text.replace('"', "\"\""))
            } else {
                text
            }
        })
        .collect::<Vec<_>>()
        .join(&separator.to_string())
}

async fn append_rows(state: &Arc<AppState>, key: &AgentKey, path: &str, rows: Option<&Value>) -> Result<Done, Stop> {
    let Some(rows) = rows.and_then(Value::as_array).filter(|rows| !rows.is_empty()) else {
        return failed("Give at least one row, as a list of cells.");
    };
    if rows.len() > MOST_ROWS {
        return failed(&format!("Give at most {MOST_ROWS} rows at a time."));
    }
    let extension = path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
    let separator = match extension.as_str() {
        "csv" => ',',
        "tsv" | "tab" => '\t',
        _ => return failed("Rows can be added to CSV and TSV files only."),
    };
    let (found, mut text) = text_file(state, key, path).await?;
    // Excel writes CSV with semicolons where the comma is the decimal mark: keep to the file's.
    let first = text.lines().next().unwrap_or_default();
    let separator = if separator == ',' && first.matches(';').count() > first.matches(',').count() { ';' } else { separator };
    let line_end = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(line_end);
    }
    for row in rows {
        let Some(cells) = row.as_array() else { return failed("Each row is a list of cells.") };
        text.push_str(&delimited(cells, separator));
        text.push_str(line_end);
    }
    if text.len() > MOST_TEXT {
        return failed("The file would be too large.");
    }
    let version = overwrite(state, key, &found, text).await?;
    after_write(state, key, "agent-append", &found.rel);
    done(json!({ "path": found.rel, "added": rows.len() }), json!({ "kind": "version", "path": found.rel, "version": version }))
}

async fn copy(state: &Arc<AppState>, key: &AgentKey, path: &str, to: &str) -> Result<Done, Stop> {
    let found = allowed(state, key, path).await?;
    let dest = allowed(state, key, to).await?;
    if !dest.full.is_dir() {
        return failed("Copy it into a folder.");
    }
    if found.full.is_dir() && key.extensions.is_some() {
        let (scope, full) = (key.scope(), found.full.clone());
        if !blocking(move || Ok(scope.folder_fits(&full))).await? {
            return failed("That folder holds files of kinds this key cannot use, so it cannot copy it.");
        }
    }
    let (root, from, into) = (state.root.clone(), found.rel.clone(), dest.rel.clone());
    let made = blocking(move || files::copy_entry(&root, &from, &into).map_err(file_error)).await?;
    after_write(state, key, "agent-copy", &made);
    done(json!({ "path": made }), json!({ "kind": "created", "path": made }))
}

async fn create_note(state: &Arc<AppState>, key: &AgentKey, title: &str, text: &str, checklist: &[String]) -> Result<Done, Stop> {
    let mut body = String::new();
    if !title.trim().is_empty() {
        body.push_str(&format!("# {}\n\n", title.trim()));
    }
    if !text.trim().is_empty() {
        body.push_str(text.trim_end());
        body.push('\n');
    }
    for item in checklist.iter().map(|item| item.trim()).filter(|item| !item.is_empty()) {
        body.push_str(&format!("- [ ] {}\n", item.replace('\n', " ")));
    }
    if body.trim().is_empty() {
        return failed("Give the note a title, text or checklist items.");
    }
    if body.len() > MOST_TEXT || body.contains('\0') {
        return failed("That note is too long, or not text.");
    }
    let place = crate::mcp_notes::notes_place(state, key).await?;
    let name = crate::notes::name_for(&body, &chrono::Local::now().format("%Y-%m-%d %H.%M").to_string());
    let dir = place.full.clone();
    let created = blocking(move || {
        let target = crate::notes::free_path(&dir, &name);
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|_| Stop::Failed("Could not make the note.".to_string()))?;
        file.write_all(body.as_bytes()).map_err(|_| Stop::Failed("Could not write the note.".to_string()))?;
        Ok(target.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default())
    })
    .await?;
    let rel = format!("{}/{created}", place.rel);
    after_write(state, key, "agent-note", &rel);
    done(json!({ "path": rel }), json!({ "kind": "created", "path": rel }))
}

fn is_note(rel: &str) -> bool {
    rel.to_ascii_lowercase().ends_with(".md")
}

async fn append_to_note(state: &Arc<AppState>, key: &AgentKey, path: &str, text: &str) -> Result<Done, Stop> {
    if text.trim().is_empty() {
        return failed("Give the text to add.");
    }
    let (found, mut body) = text_file(state, key, path).await?;
    if !is_note(&found.rel) {
        return failed("That is not a note (a Markdown file). Use edit_text for other files.");
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(text.trim_end_matches(['\n', '\r']));
    body.push('\n');
    if body.len() > MOST_TEXT {
        return failed("The note would be too long.");
    }
    let version = overwrite(state, key, &found, body).await?;
    after_write(state, key, "agent-note", &found.rel);
    done(json!({ "path": found.rel, "added": true }), json!({ "kind": "version", "path": found.rel, "version": version }))
}

async fn tick_item(state: &Arc<AppState>, key: &AgentKey, path: &str, item: &str, done_now: bool) -> Result<Done, Stop> {
    let (found, body) = text_file(state, key, path).await?;
    if !is_note(&found.rel) {
        return failed("That is not a note.");
    }
    let note = crate::notes::read_note(&found.rel, "", &body, 0, 0, false);
    let wanted = item.trim();
    let mut matching: Vec<&crate::notes::Item> = note.items.iter().filter(|candidate| candidate.text.eq_ignore_ascii_case(wanted)).collect();
    if matching.is_empty() {
        matching = note.items.iter().filter(|candidate| candidate.text.to_lowercase().contains(&wanted.to_lowercase())).collect();
        if matching.len() > 1 {
            return failed("More than one item matches: give its whole text.");
        }
    }
    let Some(target) = matching.iter().find(|candidate| candidate.done != done_now).or(matching.first()) else {
        return failed("There is no checklist item like that in the note. read_note lists them.");
    };
    if target.done == done_now {
        return failed(if done_now { "That item is ticked already." } else { "That item is not ticked." });
    }
    let ticked = crate::notes::tick(&body, target.line, &target.text, done_now).map_err(|why| Stop::Failed(why.to_string()))?;
    let version = overwrite(state, key, &found, ticked).await?;
    after_write(state, key, "agent-note", &found.rel);
    done(json!({ "path": found.rel, "item": target.text, "done": done_now }), json!({ "kind": "version", "path": found.rel, "version": version }))
}

/// A file the key may change, as bytes (for documents and spreadsheets).
async fn binary_file(state: &Arc<AppState>, key: &AgentKey, path: &str) -> Result<(agent_keys::Allowed, Vec<u8>), Stop> {
    let found = allowed(state, key, path).await?;
    let full = found.full.clone();
    let bytes = blocking(move || {
        let size = std::fs::metadata(&full).map_err(|_| Stop::Refused)?.len();
        if full.is_dir() {
            return failed("That is a folder.");
        }
        if size > 50 * 1024 * 1024 {
            return failed("That file is too large to change here.");
        }
        std::fs::read(&full).map_err(|_| Stop::Failed("Could not read that file.".to_string()))
    })
    .await?;
    Ok((found, bytes))
}

async fn overwrite_bytes(state: &Arc<AppState>, key: &AgentKey, found: &agent_keys::Allowed, bytes: Vec<u8>) -> Result<Option<i64>, Stop> {
    let version = keep(state, key, &found.full, &found.rel)?;
    let full = found.full.clone();
    blocking(move || std::fs::write(&full, bytes).map_err(|_| Stop::Failed("Could not write the file.".to_string()))).await?;
    Ok(version)
}

async fn append_sheet_rows(state: &Arc<AppState>, key: &AgentKey, path: &str, sheet: &str, rows: Option<&Value>) -> Result<Done, Stop> {
    let Some(rows) = rows.and_then(Value::as_array).filter(|rows| !rows.is_empty()) else {
        return failed("Give at least one row, as a list of cells.");
    };
    if rows.len() > MOST_ROWS {
        return failed(&format!("Give at most {MOST_ROWS} rows at a time."));
    }
    let rows: Vec<Vec<Value>> = rows.iter().map(|row| row.as_array().cloned().unwrap_or_default()).collect();
    let (found, bytes) = binary_file(state, key, path).await?;
    let sheet = sheet.to_string();
    let count = rows.len();
    let changed = blocking(move || crate::office_edit::xlsx_add_rows(&bytes, &sheet, &rows).map_err(Stop::Failed)).await?;
    let version = overwrite_bytes(state, key, &found, changed).await?;
    after_write(state, key, "agent-append", &found.rel);
    done(json!({ "path": found.rel, "added": count }), json!({ "kind": "version", "path": found.rel, "version": version }))
}

async fn add_paragraphs(state: &Arc<AppState>, key: &AgentKey, path: &str, paragraphs: &[String]) -> Result<Done, Stop> {
    let paragraphs: Vec<String> = paragraphs.iter().map(|text| text.trim_end().to_string()).collect();
    if paragraphs.is_empty() || paragraphs.len() > 1000 {
        return failed("Give between 1 and 1000 paragraphs.");
    }
    let extension = path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
    let (found, version) = match extension.as_str() {
        "docx" | "odt" => {
            let (found, bytes) = binary_file(state, key, path).await?;
            let list = paragraphs.clone();
            let changed = blocking(move || {
                if extension == "docx" { crate::office_edit::docx_add_paragraphs(&bytes, &list) } else { crate::office_edit::odt_add_paragraphs(&bytes, &list) }.map_err(Stop::Failed)
            })
            .await?;
            let version = overwrite_bytes(state, key, &found, changed).await?;
            (found, version)
        }
        "txt" | "md" | "markdown" | "text" => {
            let (found, mut body) = text_file(state, key, path).await?;
            if !body.is_empty() && !body.ends_with('\n') {
                body.push('\n');
            }
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&paragraphs.join("\n\n"));
            body.push('\n');
            let version = overwrite(state, key, &found, body).await?;
            (found, version)
        }
        _ => return failed("Paragraphs can be added to docx, odt, text and Markdown files."),
    };
    after_write(state, key, "agent-edit", &found.rel);
    done(json!({ "path": found.rel, "added": paragraphs.len() }), json!({ "kind": "version", "path": found.rel, "version": version }))
}

async fn upload_file(state: &Arc<AppState>, key: &AgentKey, folder: &str, name: &str, encoded: &str, replace: bool) -> Result<Done, Stop> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name.contains('/') || name.contains('\\') {
        return failed("Give the file a name that does not start with a dot.");
    }
    if !key.allows_name(name) {
        return failed("This key can only make files with certain extensions.");
    }
    if encoded.len() > MOST_UPLOAD / 3 * 4 + 8 {
        return failed("That file is too large to upload here (25 MB at most).");
    }
    let compact: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = crate::http::base64_decode(&compact).map_err(|()| Stop::Failed("That is not base64.".to_string()))?;
    let place = allowed(state, key, folder).await?;
    if !place.full.is_dir() {
        return failed("Upload it into a folder.");
    }
    let rel = if place.rel.is_empty() { name.to_string() } else { format!("{}/{name}", place.rel) };
    let target = place.full.join(name);
    if replace && target.is_file() {
        let found = allowed(state, key, &rel).await?;
        let version = overwrite_bytes(state, key, &found, bytes).await?;
        after_write(state, key, "agent-upload", &found.rel);
        return done(json!({ "path": found.rel, "replaced": true }), json!({ "kind": "version", "path": found.rel, "version": version }));
    }
    let (dir, file_name) = (place.full.clone(), name.to_string());
    let saved = blocking(move || {
        // A name already taken gets a number: "report 2.pdf".
        let (stem, extension) = file_name.rsplit_once('.').map_or((file_name.clone(), String::new()), |(stem, ext)| (stem.to_string(), format!(".{ext}")));
        let mut candidate = dir.join(&file_name);
        let mut n = 2;
        while candidate.exists() || candidate.is_symlink() {
            candidate = dir.join(format!("{stem} {n}{extension}"));
            n += 1;
        }
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate).map_err(|_| Stop::Failed("Could not save the file.".to_string()))?;
        file.write_all(&bytes).map_err(|_| Stop::Failed("Could not save the file.".to_string()))?;
        Ok(candidate.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default())
    })
    .await?;
    let rel = if place.rel.is_empty() { saved } else { format!("{}/{saved}", place.rel) };
    after_write(state, key, "agent-upload", &rel);
    done(json!({ "path": rel, "replaced": false }), json!({ "kind": "created", "path": rel }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_quoted_only_where_needed() {
        let row = [json!("plain"), json!("a, b"), json!("say \"hi\""), json!(12.5), json!(null), json!(true)];
        assert_eq!(delimited(&row, ','), "plain,\"a, b\",\"say \"\"hi\"\"\",12.5,,true");
        assert_eq!(delimited(&row, '\t'), "plain\ta, b\t\"say \"\"hi\"\"\"\t12.5\t\ttrue");
    }
}
