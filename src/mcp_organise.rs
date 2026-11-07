//! Tools for keys at the "organise" level: tags, new folders, renaming, moving, the Trash, and
//! albums. Every change is easy to undo by hand and none deletes anything: "delete" means moving
//! to the Trash, and protected files are refused. Both ends of a move are inside the key's
//! folders, and a key's own folders cannot be renamed, moved or put in the Trash.
//!
//! Each change is written down in `agent_changes` with how to undo it, so a person can undo it
//! from Settings → Agents. A key in approval mode only asks: its changes wait there until a
//! person approves them (and are carried out then, checked again) or declines them.

use crate::agent_keys::{self, AgentKey, Allowed, Change};
use crate::files::{self, FileError};
use crate::http::AppState;
use crate::mcp::{allowed, as_text, blocking, failed, Args, Stop, REFUSED};
use crate::{db, mcp_photos, mcp_write};
use serde_json::{json, Value};
use std::sync::Arc;

/// The most paths one call may change.
const MOST_PATHS: usize = 500;

pub fn tools() -> Vec<Value> {
    // Each change can be reversed with another call, so none is marked destructive.
    let changes = json!({ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false });
    let same_again = json!({ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    let path = |about: &str| json!({ "type": "string", "description": about });
    let paths = json!({ "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": MOST_PATHS, "description": "The files or folders." });
    let tags = json!({ "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 20, "description": "The tags, without #." });
    vec![
        json!({
            "name": "add_tags",
            "title": "Tag files",
            "description": "Adds tags to files or folders. Tags show in OwnNAS and can be searched with search (tag:name).",
            "inputSchema": { "type": "object", "required": ["paths", "tags"], "properties": { "paths": paths, "tags": tags } },
            "annotations": same_again,
        }),
        json!({
            "name": "remove_tags",
            "title": "Untag files",
            "description": "Removes tags from files or folders.",
            "inputSchema": { "type": "object", "required": ["paths", "tags"], "properties": { "paths": paths, "tags": tags } },
            "annotations": same_again,
        }),
        json!({
            "name": "create_folder",
            "title": "Make a folder",
            "description": "Makes a new folder inside a folder.",
            "inputSchema": { "type": "object", "required": ["path", "name"], "properties": {
                "path": path("The folder to make it in."),
                "name": { "type": "string", "description": "The new folder's name." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "rename",
            "title": "Rename",
            "description": "Gives a file or folder a new name in the same folder. Returns its new path.",
            "inputSchema": { "type": "object", "required": ["path", "name"], "properties": {
                "path": path("The file or folder."),
                "name": { "type": "string", "description": "The new name, with the extension for a file." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "move",
            "title": "Move",
            "description": "Moves a file or folder into another folder. A name already taken there gets a number added. Returns its new path.",
            "inputSchema": { "type": "object", "required": ["path", "to"], "properties": {
                "path": path("The file or folder to move."),
                "to": path("The folder to move it into."),
            } },
            "annotations": changes,
        }),
        json!({
            "name": "move_to_trash",
            "title": "Move to the Trash",
            "description": "Moves a file or folder to the OwnNAS Trash, where the user can restore it. Nothing is deleted for good. Protected files are refused. Only do this when the user asked for it.",
            "inputSchema": { "type": "object", "required": ["path"], "properties": { "path": path("The file or folder.") } },
            "annotations": changes,
        }),
        json!({
            "name": "create_album",
            "title": "Make an album",
            "description": "Makes a Gallery album, optionally inside another album. Returns its id, for add_to_album.",
            "inputSchema": { "type": "object", "required": ["name"], "properties": {
                "name": { "type": "string", "description": "The album's name." },
                "parent": { "type": "integer", "description": "An album to make it in, from list_albums." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "add_to_album",
            "title": "Add photos to an album",
            "description": "Adds photos and videos to an album. The files stay where they are.",
            "inputSchema": { "type": "object", "required": ["album", "paths"], "properties": {
                "album": { "type": "integer", "description": "The album's id." },
                "paths": paths,
            } },
            "annotations": same_again,
        }),
        json!({
            "name": "sort_into_folders",
            "title": "Sort a folder into sub-folders",
            "description": "Moves the files of a folder into sub-folders by a pattern: {YYYY} year, {MM} month, {DD} day, {MMMM} month name, {type} kind (Pictures, Videos, Music, Documents, Archives, Other), {ext} extension, {first} first letter; / makes a folder inside a folder. For example {YYYY}/{MM} or {YYYY}-{MM}. With dry_run it only says what would move. Can be undone.",
            "inputSchema": { "type": "object", "required": ["path", "pattern"], "properties": {
                "path": path("The folder to sort."),
                "pattern": { "type": "string", "description": "Where each file goes, such as {YYYY}/{MM}." },
                "date": { "type": "string", "enum": ["taken", "modified", "created"], "description": "Which date: taken (photos and videos, else last changed; the default), modified or created." },
                "recursive": { "type": "boolean", "description": "Also the files in the folders inside it." },
                "dry_run": { "type": "boolean", "description": "Only say what would move." },
            } },
            "annotations": changes,
        }),
        json!({
            "name": "pending_changes",
            "title": "Changes waiting for approval",
            "description": "Lists this key's latest changes and what became of them: waiting for the user to approve, made, declined, failed, or undone by the user.",
            "inputSchema": { "type": "object", "properties": {
                "status": { "type": "string", "enum": ["pending", "done", "declined", "failed", "undone"], "description": "Only changes in this state." },
            } },
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
        }),
        json!({
            "name": "remove_from_album",
            "title": "Take photos out of an album",
            "description": "Takes photos out of an album. The files themselves stay.",
            "inputSchema": { "type": "object", "required": ["album", "paths"], "properties": {
                "album": { "type": "integer", "description": "The album's id." },
                "paths": paths,
            } },
            "annotations": same_again,
        }),
    ]
}

pub fn has(name: &str) -> bool {
    tools().iter().any(|tool| tool["name"] == name) || mcp_write::has(name)
}

/// A change that was made: what to tell the agent, and how to undo it.
pub(crate) struct Done {
    content: Vec<Value>,
    undo: Option<Value>,
}

pub(crate) fn done(answer: Value, undo: Value) -> Result<Done, Stop> {
    Ok(Done { content: as_text(&answer), undo: Some(undo) })
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(crate) async fn call(state: &Arc<AppState>, key: &AgentKey, name: &str, args: &Args<'_>) -> Option<Result<Vec<Value>, Stop>> {
    if !has(name) {
        return None;
    }
    if !key.can_organise() {
        return Some(failed("This key can only read. Ask the user for a key with organise access in OwnNAS, Settings → Agents."));
    }
    if mcp_write::has(name) && !key.can_write() {
        return Some(failed("This key cannot write files. Ask the user for a key with write access in OwnNAS, Settings → Agents."));
    }
    if name == "pending_changes" {
        return Some(pending_changes(state, key, &args.text("status")));
    }
    if state.readonly {
        return Some(failed("This OwnNAS server is read-only."));
    }
    if name == "sort_into_folders" && args.raw().get("dry_run").and_then(Value::as_bool).unwrap_or(false) {
        return Some(sort_plan(state, key, args).await.map(|plan| as_text(&plan)));
    }
    let summary = summarize(name, args);
    if key.approve {
        return Some(ask(state, key, name, args, &summary, None).await);
    }
    // Past its changes for the day (UTC), a key that does not ask first asks anyway.
    if changes_today(state, key) >= key.daily_changes {
        let why = format!("This key has made {} changes today, the most it makes without asking: the rest wait for the user's approval.", key.daily_changes);
        return Some(ask(state, key, name, args, &summary, Some(&why)).await);
    }
    Some(match run(state, key, name, args).await {
        Ok(made) => {
            let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
            let result = made.content.first().and_then(|first| first["text"].as_str()).unwrap_or_default().to_string();
            let _ = agent_keys::add_change(&conn, key, name, args.raw(), &summary, "done", made.undo.as_ref(), &result, now());
            Ok(made.content)
        }
        Err(stop) => Err(stop),
    })
}

async fn run(state: &Arc<AppState>, key: &AgentKey, name: &str, args: &Args<'_>) -> Result<Done, Stop> {
    writable(state, key, name, args).await?;
    match name {
        "add_tags" => tags(state, key, args, true).await,
        "remove_tags" => tags(state, key, args, false).await,
        "create_folder" => create_folder(state, key, &args.path(), &args.text("name")).await,
        "rename" => rename(state, key, &args.path(), &args.text("name")).await,
        "move" => move_into(state, key, &args.path(), &args.text("to")).await,
        "move_to_trash" => trash(state, key, &args.path()).await,
        "create_album" => create_album(state, key, &args.text("name"), args.integer("parent")).await,
        "add_to_album" => album_items(state, key, args, true).await,
        "remove_from_album" => album_items(state, key, args, false).await,
        "sort_into_folders" => sort_into_folders(state, key, args).await,
        other if mcp_write::has(other) && key.can_write() => mcp_write::run(state, key, other, args).await,
        _ => failed("Unknown tool"),
    }
}

/// The places a change alters, for read-only folders: a path as given, and the name of what is
/// made inside it, if anything.
fn targets(name: &str, args: &Args<'_>) -> Vec<(String, Option<String>)> {
    match name {
        "add_tags" | "remove_tags" => args.texts("paths").into_iter().map(|path| (path, None)).collect(),
        "create_folder" => vec![(args.path(), Some(args.text("name")))],
        "rename" | "move_to_trash" | "edit_text" | "append_rows" | "add_paragraphs" | "append_to_note" | "tick_item" | "sort_into_folders" => vec![(args.path(), None)],
        "move" => vec![(args.path(), None), (args.text("to"), None)],
        "copy" => vec![(args.text("to"), None)],
        "upload_file" => vec![(args.path(), Some(args.text("name")))],
        "write_file" => {
            let path = args.path();
            let (parent, file) = path.rsplit_once('/').unwrap_or(("", &path));
            vec![(parent.to_string(), Some(file.to_string()))]
        }
        // Albums change nothing in the folders.
        _ => Vec::new(),
    }
}

/// Refuses a change in a folder the key may only read.
async fn writable(state: &Arc<AppState>, key: &AgentKey, name: &str, args: &Args<'_>) -> Result<(), Stop> {
    if key.read_only.is_empty() {
        return Ok(());
    }
    let mut places = targets(name, args);
    if name == "create_note" {
        places.push((crate::http::notes_folder(state), None));
    }
    let (root, key) = (state.root.clone(), key.clone());
    blocking(move || {
        for (path, inside) in places {
            let found = agent_keys::check(&root, &key.folders, path.trim().trim_start_matches('/')).ok_or(Stop::Refused)?;
            let rel = match inside.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
                Some(name) if !found.rel.is_empty() => format!("{}/{name}", found.rel),
                Some(name) => name.to_string(),
                None => found.rel,
            };
            if !key.may_change(&rel) {
                return failed("That folder is read-only for this key: it can look, but not change anything there.");
            }
        }
        Ok(())
    })
    .await
}

/// One line about a change, for the person approving or undoing it.
fn summarize(name: &str, args: &Args<'_>) -> String {
    let paths = args.texts("paths");
    let items = if paths.len() == 1 { paths[0].clone() } else { format!("{} items", paths.len()) };
    let tags = args.texts("tags").iter().map(|tag| format!("#{}", tag.trim_start_matches('#'))).collect::<Vec<_>>().join(" ");
    let album = args.integer("album").map(|id| format!("album {id}")).unwrap_or_default();
    match name {
        "add_tags" => format!("Tag {items} {tags}"),
        "remove_tags" => format!("Untag {items} {tags}"),
        "create_folder" => format!("Make folder {}/{}", args.path(), args.text("name")),
        "rename" => format!("Rename {} to {}", args.path(), args.text("name")),
        "move" => format!("Move {} into {}", args.path(), args.text("to")),
        "move_to_trash" => format!("Move {} to the Trash", args.path()),
        "create_album" => format!("Make album {}", args.text("name")),
        "add_to_album" => format!("Add {items} to {album}"),
        "remove_from_album" => format!("Take {items} out of {album}"),
        "sort_into_folders" => format!("Sort {} into folders by {}", args.path(), args.text("pattern")),
        "write_file" => format!("{} {}", if args.raw().get("replace").and_then(Value::as_bool).unwrap_or(false) { "Write (replacing)" } else { "Write" }, args.path()),
        "edit_text" => format!("Change a passage in {}", args.path()),
        "append_rows" => format!("Add {} row(s) to {}", args.raw().get("rows").and_then(Value::as_array).map_or(0, Vec::len), args.path()),
        "copy" => format!("Copy {} into {}", args.path(), args.text("to")),
        "add_paragraphs" => format!("Add {} paragraph(s) to {}", args.texts("paragraphs").len(), args.path()),
        "upload_file" => format!("Upload {} into {}", args.text("name"), args.path()),
        "create_note" => format!("Make the note “{}”", if args.text("title").trim().is_empty() { args.text("text").lines().next().unwrap_or_default().chars().take(60).collect::<String>() } else { args.text("title") }),
        "append_to_note" => format!("Add to the note {}", args.path()),
        "tick_item" => format!("{} “{}” in {}", if args.raw().get("done").and_then(Value::as_bool).unwrap_or(true) { "Tick" } else { "Untick" }, args.text("item"), args.path()),
        other => other.to_string(),
    }
}

/// Approval mode: checks the paths, then leaves the change for a person to decide on.
fn changes_today(state: &AppState, key: &AgentKey) -> i64 {
    let start = now() - now().rem_euclid(86_400);
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    conn.query_row("SELECT count(*) FROM agent_changes WHERE key_id = ?1 AND status = 'done' AND at >= ?2", rusqlite::params![key.id, start], |row| row.get(0)).unwrap_or(0)
}

async fn ask(state: &Arc<AppState>, key: &AgentKey, name: &str, args: &Args<'_>, summary: &str, why: Option<&str>) -> Result<Vec<Value>, Stop> {
    let mut paths = args.texts("paths");
    for one in ["path", "to"] {
        if args.raw().get(one).is_some() {
            paths.push(args.text(one));
        }
    }
    // A file to be written may not be there yet: its folder must be.
    if name == "write_file" {
        let path = args.path();
        paths = vec![path.rsplit_once('/').map_or(String::new(), |(parent, _)| parent.to_string())];
    }
    let (root, folders) = (state.root.clone(), key.folders.clone());
    blocking(move || {
        for path in paths {
            agent_keys::check(&root, &folders, path.trim().trim_start_matches('/')).ok_or(Stop::Refused)?;
        }
        Ok(())
    })
    .await?;
    writable(state, key, name, args).await?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let id = agent_keys::add_change(&conn, key, name, args.raw(), summary, "pending", None, "", now()).map_err(Stop::Failed)?;
    let mut answer = json!({
        "status": "pending",
        "request": id,
        "note": "Nothing has changed yet: the user approves or declines this in OwnNAS. Use pending_changes to see what became of it, and don't ask again in the meantime.",
    });
    if let Some(why) = why {
        answer["why"] = json!(why);
    }
    Ok(as_text(&answer))
}

fn pending_changes(state: &AppState, key: &AgentKey, status: &str) -> Result<Vec<Value>, Stop> {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let status = (!status.is_empty()).then_some(status);
    let found = agent_keys::changes(&conn, Some(key.id), status, 50).map_err(Stop::Failed)?;
    let shown: Vec<Value> = found.iter().map(|change| json!({ "request": change.id, "change": change.summary, "status": change.status, "result": change.result })).collect();
    Ok(as_text(&json!({ "changes": shown })))
}

/// Carries out (or declines) a change waiting for approval; returns what became of it.
pub(crate) async fn decide(state: &Arc<AppState>, id: i64, approve: bool, by_user: &str) -> Result<Change, String> {
    let (change, key) = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let change = agent_keys::change(&conn, id)?.ok_or("That change is no longer there")?;
        if change.status != "pending" {
            return Err("That change was already decided on".into());
        }
        let key = agent_keys::usable(&conn, change.key_id, now())?;
        (change, key)
    };
    let (status, undo, result) = match key {
        _ if !approve => ("declined", None, String::new()),
        None => ("declined", None, "The key was revoked or has expired.".to_string()),
        Some(_) if state.readonly => return Err("This OwnNAS server is read-only".into()),
        Some(key) => match run(state, &key, &change.tool, &Args(&change.arguments)).await {
            Ok(made) => ("done", made.undo, made.content.first().and_then(|first| first["text"].as_str()).unwrap_or_default().to_string()),
            Err(Stop::Refused) => ("failed", None, REFUSED.to_string()),
            Err(Stop::Failed(why)) => ("failed", None, why),
        },
    };
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    agent_keys::settle_change(&conn, id, status, undo.as_ref(), &result, by_user, now())?;
    let _ = db::log_event(&conn, None, by_user, if approve { "agent-approve" } else { "agent-decline" }, &change.summary);
    agent_keys::change(&conn, id)?.ok_or_else(|| "That change is no longer there".to_string())
}

fn text_of(value: &Value, name: &str) -> String {
    value.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Reverses a change that was made. Undoing is a person's choice, so the key's folders do not
/// limit it, but nothing is overwritten: a place taken since stops it.
pub(crate) async fn undo(state: &Arc<AppState>, id: i64, by_user: &str) -> Result<Change, String> {
    let change = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        agent_keys::change(&conn, id)?.ok_or("That change is no longer there")?
    };
    let Some(how) = change.undo.clone().filter(|_| change.can_undo) else {
        return Err("That change cannot be undone".into());
    };
    let root = state.root.clone();
    let paths = |name: &str| -> Vec<String> { how.get(name).and_then(Value::as_array).map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default() };
    let message = |err: FileError| match file_error(err) {
        Stop::Refused => "It is no longer where the change left it".to_string(),
        Stop::Failed(why) => why,
    };
    match text_of(&how, "kind").as_str() {
        "tags" => {
            let add = how["add"].as_bool().unwrap_or(true);
            let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
            for pair in how["pairs"].as_array().into_iter().flatten() {
                let (path, tag) = (pair[0].as_str().unwrap_or_default(), pair[1].as_str().unwrap_or_default());
                if add {
                    db::remove_tag(&conn, path, tag)?;
                } else {
                    db::add_tag(&conn, path, tag)?;
                }
            }
        }
        "folder" => {
            let path = text_of(&how, "path");
            let found = files::resolve(&root, &path).map_err(message)?;
            if found.rel.is_empty() || !found.full.is_dir() {
                return Err("The folder is no longer there".into());
            }
            std::fs::remove_dir(&found.full).map_err(|_| "The folder has something in it now, so it stays".to_string())?;
            crate::http::invalidate_sizes(state, &found.rel);
        }
        "moved" => {
            let (from, to) = (text_of(&how, "from"), text_of(&how, "to"));
            let (back_from, back_to) = (to.clone(), from.clone());
            tokio::task::spawn_blocking(move || files::move_exact(&root, &back_from, &back_to)).await.map_err(|_| "Could not undo it".to_string())?.map_err(message)?;
            crate::http::rewrite_meta(state, &to, &from);
            crate::http::relink_notes(state, by_user, &to, &from).await;
            crate::http::invalidate_sizes(state, &from);
            crate::http::invalidate_sizes(state, &to);
        }
        "trashed" => {
            let to = text_of(&how, "to");
            let in_trash = to.clone();
            let back = tokio::task::spawn_blocking(move || files::restore_entry(&root, &in_trash)).await.map_err(|_| "Could not undo it".to_string())?.map_err(message)?;
            crate::http::rewrite_meta(state, &to, &back);
            crate::http::invalidate_sizes(state, &back);
        }
        "album" => {
            let id = how["id"].as_i64().unwrap_or_default();
            let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
            match crate::albums::get(&conn, id) {
                Ok(album) if album.count == 0 && album.albums == 0 => crate::albums::delete(&conn, id).map_err(|err| err.to_string())?,
                Ok(_) => return Err("The album has photos in it now, so it stays".into()),
                Err(_) => {}
            }
        }
        "album_items" => {
            let (id, add) = (how["id"].as_i64().unwrap_or_default(), how["add"].as_bool().unwrap_or(true));
            let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
            crate::albums::change_items(&conn, id, &paths("paths"), !add, by_user).map_err(|err| err.to_string())?;
        }
        "version" => {
            // What the file held before: a kept version, or nothing for a file that was empty.
            let path = text_of(&how, "path");
            match how["version"].as_i64() {
                Some(version) => {
                    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
                    crate::versions::restore(&conn, &state.versions, &root, version, by_user)?;
                }
                None => {
                    let found = files::resolve(&root, &path).map_err(message)?;
                    std::fs::write(&found.full, b"").map_err(|_| "Could not undo it".to_string())?;
                }
            }
            crate::http::invalidate_sizes(state, path.rsplit_once('/').map_or("", |(parent, _)| parent));
        }
        "sorted" => {
            let moves: Vec<crate::sort_folder::Move> = serde_json::from_value(how["moves"].clone()).unwrap_or_default();
            let sorted = moves.clone();
            let (back, failed) = tokio::task::spawn_blocking(move || crate::sort_folder::undo(&root, &sorted)).await.map_err(|_| "Could not undo it".to_string())?;
            for item in &moves {
                if !failed.iter().any(|(path, _)| path == &item.to) {
                    crate::http::rewrite_meta(state, &item.to, &item.from);
                }
            }
            if let Some((path, why)) = failed.first() {
                return Err(format!("Put {back} back, but not {path}: {why}"));
            }
        }
        "created" => {
            let path = text_of(&how, "path");
            let rel = path.clone();
            let moved = tokio::task::spawn_blocking(move || files::trash_entry(&root, &rel)).await.map_err(|_| "Could not undo it".to_string())?.map_err(message)?;
            crate::http::rewrite_meta(state, &path, &moved);
            crate::http::invalidate_sizes(state, &path);
        }
        _ => return Err("That change cannot be undone".into()),
    }
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    agent_keys::settle_change(&conn, id, "undone", None, &format!("Undone by {by_user}"), by_user, now())?;
    let _ = db::log_event(&conn, None, by_user, "agent-undo", &change.summary);
    drop(conn);
    state.media.descriptions.invalidate();
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    agent_keys::change(&conn, id)?.ok_or_else(|| "That change is no longer there".to_string())
}

/// Who the activity log says made a change.
fn by(key: &AgentKey) -> String {
    format!("agent: {}", key.name)
}

fn log(state: &AppState, key: &AgentKey, action: &str, detail: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::log_event(&conn, None, &by(key), action, detail);
}

pub(crate) fn file_error(err: FileError) -> Stop {
    Stop::Failed(
        match err {
            FileError::NotFound | FileError::Forbidden => return Stop::Refused,
            FileError::AlreadyExists => "Something with that name is already there.",
            FileError::InvalidName => "That name cannot be used.",
            FileError::NotADirectory => "That is a file, not a folder.",
            FileError::IsADirectory => "That is a folder.",
            FileError::Rejected(why) | FileError::Io(why) => why,
            FileError::Cancelled => "That was stopped.",
        }
        .to_string(),
    )
}

/// A path the key may change: allowed, and not one of the key's own folders (which would take
/// the key's reach with it).
async fn changeable(state: &Arc<AppState>, key: &AgentKey, path: &str) -> Result<Allowed, Stop> {
    let found = allowed(state, key, path).await?;
    if found.rel.is_empty() || key.folders.contains(&found.rel) {
        return failed("That is one of the folders this key was given; it cannot be renamed, moved or put in the Trash.");
    }
    Ok(found)
}

async fn all_allowed(state: &Arc<AppState>, key: &AgentKey, args: &Args<'_>) -> Result<Vec<String>, Stop> {
    let paths = args.texts("paths");
    if paths.is_empty() {
        return failed("Give at least one path.");
    }
    if paths.len() > MOST_PATHS {
        return failed(&format!("Give at most {MOST_PATHS} paths at a time."));
    }
    let (root, folders) = (state.root.clone(), key.folders.clone());
    blocking(move || {
        let mut found = Vec::new();
        for path in paths {
            let place = agent_keys::check(&root, &folders, path.trim().trim_start_matches('/')).ok_or(Stop::Refused)?;
            if !found.contains(&place.rel) {
                found.push(place.rel);
            }
        }
        Ok(found)
    })
    .await
}

async fn tags(state: &Arc<AppState>, key: &AgentKey, args: &Args<'_>, add: bool) -> Result<Done, Stop> {
    let paths = all_allowed(state, key, args).await?;
    let wanted = args.texts("tags");
    if wanted.is_empty() || wanted.len() > 20 {
        return failed("Give between 1 and 20 tags.");
    }
    let mut names = Vec::new();
    for tag in &wanted {
        names.push(db::normalize_tag(tag.trim().trim_start_matches('#')).map_err(Stop::Failed)?);
    }
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    // Only what really changed is undone: a tag that was already there stays.
    let mut pairs = Vec::new();
    for path in &paths {
        let had = db::list_tags(&conn, path).unwrap_or_default();
        for tag in &names {
            let there = had.iter().any(|old| old.eq_ignore_ascii_case(tag));
            if add && !there {
                db::add_tag(&conn, path, tag).map_err(Stop::Failed)?;
                pairs.push(json!([path, tag]));
            } else if !add && there {
                db::remove_tag(&conn, path, tag).map_err(Stop::Failed)?;
                pairs.push(json!([path, tag]));
            }
        }
    }
    let list = names.iter().map(|tag| format!("#{tag}")).collect::<Vec<_>>().join(" ");
    let _ = db::log_event(&conn, None, &by(key), if add { "tag" } else { "untag" }, &format!("{} item(s) {list}", paths.len()));
    done(json!({ "changed": paths.len(), "tags": names }), json!({ "kind": "tags", "add": add, "pairs": pairs }))
}

async fn create_folder(state: &Arc<AppState>, key: &AgentKey, parent: &str, name: &str) -> Result<Done, Stop> {
    let parent = allowed(state, key, parent).await?;
    if !parent.full.is_dir() {
        return failed("That is a file, not a folder.");
    }
    let name = name.trim().to_string();
    let (root, inside) = (state.root.clone(), parent.rel.clone());
    let made = name.clone();
    blocking(move || files::make_dir(&root, &inside, &made).map_err(file_error)).await?;
    let path = if parent.rel.is_empty() { name } else { format!("{}/{name}", parent.rel) };
    crate::http::invalidate_sizes(state, &parent.rel);
    log(state, key, "create-folder", &path);
    done(json!({ "path": path }), json!({ "kind": "folder", "path": path }))
}

async fn rename(state: &Arc<AppState>, key: &AgentKey, path: &str, name: &str) -> Result<Done, Stop> {
    let found = changeable(state, key, path).await?;
    if name.trim().is_empty() || name.contains('/') {
        return failed("Give a new name, without /.");
    }
    let (root, from, to_name) = (state.root.clone(), found.rel.clone(), name.trim().to_string());
    let to = blocking(move || files::rename_entry(&root, &from, &to_name).map_err(file_error)).await?;
    crate::http::rewrite_meta(state, &found.rel, &to);
    crate::http::relink_notes(state, &by(key), &found.rel, &to).await;
    log(state, key, "rename", &format!("{} → {to}", found.rel));
    done(json!({ "path": to }), json!({ "kind": "moved", "from": found.rel, "to": to }))
}

async fn move_into(state: &Arc<AppState>, key: &AgentKey, path: &str, to: &str) -> Result<Done, Stop> {
    let found = changeable(state, key, path).await?;
    let dest = allowed(state, key, to).await?;
    if !dest.full.is_dir() {
        return failed("Move it into a folder.");
    }
    if dest.full.starts_with(&found.full) {
        return failed("A folder cannot go inside itself.");
    }
    let (root, from, into) = (state.root.clone(), found.rel.clone(), dest.rel.clone());
    let moved = blocking(move || files::move_entry(&root, &from, &into).map_err(file_error)).await?;
    crate::http::rewrite_meta(state, &found.rel, &moved);
    crate::http::invalidate_sizes(state, &found.rel);
    crate::http::invalidate_sizes(state, &dest.rel);
    log(state, key, "move", &format!("{} → {}", found.rel, dest.rel));
    done(json!({ "path": moved }), json!({ "kind": "moved", "from": found.rel, "to": moved }))
}

async fn trash(state: &Arc<AppState>, key: &AgentKey, path: &str) -> Result<Done, Stop> {
    let found = changeable(state, key, path).await?;
    let blocker = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::deletion_blocker(&conn, &found.rel).map_err(Stop::Failed)?
    };
    if blocker.is_some() {
        return failed("That is protected from deletion, or holds something that is. Leave it where it is.");
    }
    let (root, rel) = (state.root.clone(), found.rel.clone());
    let moved = blocking(move || files::trash_entry(&root, &rel).map_err(file_error)).await?;
    crate::http::rewrite_meta(state, &found.rel, &moved);
    crate::http::invalidate_sizes(state, &found.rel);
    log(state, key, "trash", &found.rel);
    done(json!({ "moved": found.rel, "note": "It is in the OwnNAS Trash, where the user can restore it." }), json!({ "kind": "trashed", "from": found.rel, "to": moved }))
}

/// Where sorting would move the files: the plan, checked against the key's folders.
async fn sort_plan(state: &Arc<AppState>, key: &AgentKey, args: &Args<'_>) -> Result<Value, Stop> {
    let found = allowed(state, key, &args.path()).await?;
    if !found.full.is_dir() {
        return failed("That is a file, not a folder.");
    }
    let pattern = args.text("pattern");
    crate::sort_folder::check_pattern(&pattern).map_err(Stop::Failed)?;
    let from = match args.text("date").as_str() {
        "modified" => crate::sort_folder::DateFrom::Modified,
        "created" => crate::sort_folder::DateFrom::Created,
        _ => crate::sort_folder::DateFrom::Taken,
    };
    let recursive = args.raw().get("recursive").and_then(Value::as_bool).unwrap_or(false);
    let taken = crate::http::taken_dates(state, &found.rel);
    let (root, rel, folders) = (state.root.clone(), found.rel.clone(), key.folders.clone());
    blocking(move || {
        let months: Vec<String> = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"].map(String::from).to_vec();
        let mut plan = crate::sort_folder::plan(&root, &rel, &pattern, from, recursive, &months, &taken).map_err(file_error)?;
        // Links inside could lead elsewhere: only what the key may use moves.
        plan.moves.retain(|item| agent_keys::check(&root, &folders, &item.from).is_some());
        Ok(json!({ "path": rel, "moves": plan.moves, "folders": plan.folders, "inPlace": plan.in_place, "truncated": plan.truncated }))
    })
    .await
}

async fn sort_into_folders(state: &Arc<AppState>, key: &AgentKey, args: &Args<'_>) -> Result<Done, Stop> {
    let plan = sort_plan(state, key, args).await?;
    let moves: Vec<crate::sort_folder::Move> = serde_json::from_value(plan["moves"].clone()).unwrap_or_default();
    if moves.is_empty() {
        return failed("Nothing to move: every file is where the pattern puts it already.");
    }
    let root = state.root.clone();
    let (made, failed_moves) = blocking(move || Ok(crate::sort_folder::apply(&root, &moves))).await?;
    for item in &made {
        crate::http::rewrite_meta(state, &item.from, &item.to);
    }
    crate::http::invalidate_sizes(state, plan["path"].as_str().unwrap_or_default());
    log(state, key, "sort-folder", &format!("{} by {}: {} file(s)", plan["path"].as_str().unwrap_or_default(), args.text("pattern"), made.len()));
    let failed_list: Vec<Value> = failed_moves.iter().map(|(path, why)| json!({ "path": path, "why": why })).collect();
    done(json!({ "moved": made.len(), "folders": plan["folders"], "failed": failed_list }), json!({ "kind": "sorted", "moves": made }))
}

/// An album the key may change: one with photos in its folders, or an empty one (such as one
/// it just made). Others answer as if they did not exist.
fn album_usable(state: &AppState, key: &AgentKey, id: i64) -> Result<(), Stop> {
    if mcp_photos::album_seen(state, &key.folders, id)?.is_some() {
        return Ok(());
    }
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    match crate::albums::get(&conn, id) {
        Ok(album) if album.count == 0 && album.albums == 0 => Ok(()),
        _ => Err(Stop::Refused),
    }
}

async fn create_album(state: &Arc<AppState>, key: &AgentKey, name: &str, parent: Option<i64>) -> Result<Done, Stop> {
    if let Some(parent) = parent {
        album_usable(state, key, parent)?;
    }
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let id = crate::albums::create(&conn, parent, name, None).map_err(|err| Stop::Failed(err.to_string()))?;
    let _ = db::log_event(&conn, None, &by(key), "album-create", name.trim());
    done(json!({ "album": id, "name": name.trim() }), json!({ "kind": "album", "id": id }))
}

async fn album_items(state: &Arc<AppState>, key: &AgentKey, args: &Args<'_>, add: bool) -> Result<Done, Stop> {
    let Some(id) = args.integer("album") else { return failed("Give the album's id.") };
    album_usable(state, key, id)?;
    let paths = all_allowed(state, key, args).await?;
    if add {
        let root = state.root.clone();
        let names = paths.clone();
        blocking(move || {
            for path in &names {
                let full = root.join(path);
                if !full.is_file() || !matches!(files::kind_of(path), "image" | "video" | "svg") {
                    return failed(&format!("\"{path}\" is not a photo or video."));
                }
            }
            Ok(())
        })
        .await?;
    }
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let who = by(key);
    // Only what really changes is undone later.
    let before: std::collections::HashSet<String> = crate::albums::items(&conn, id).map(|items| items.into_iter().map(|item| item.path).collect()).unwrap_or_default();
    let really: Vec<&String> = paths.iter().filter(|path| before.contains(*path) != add).collect();
    let undo = json!({ "kind": "album_items", "id": id, "add": add, "paths": really });
    let changed = crate::albums::change_items(&conn, id, &paths, add, &who).map_err(|err| Stop::Failed(err.to_string()))?;
    // As in the Gallery: photos added to a workspace's album show in that workspace.
    if add {
        if let Ok(Some(workspace)) = crate::albums::workspace_id(&conn, id) {
            let _ = crate::workspaces::change_items(&conn, &[workspace], &paths, true, &who);
        }
    }
    let _ = db::log_event(&conn, None, &who, if add { "album-add" } else { "album-remove" }, &format!("{changed} item(s), album {id}"));
    drop(conn);
    state.media.descriptions.invalidate();
    done(json!({ "album": id, "changed": changed }), undo)
}
