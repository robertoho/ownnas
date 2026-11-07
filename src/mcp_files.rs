//! Questions about the library as a whole for `/mcp`, answered from the file index rather than
//! by walking folders: what changed lately, and what takes up the space. Hidden files count for
//! nothing here, as everywhere else for keys.

use crate::agent_keys::{self, AgentKey};
use crate::http::AppState;
use crate::mcp::{allowed, as_text, blocking, failed, Args, Stop};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Arc;

const RECENT_DEFAULT: u64 = 50;
const RECENT_MOST: u64 = 500;
/// How many changed files are looked through for the key's.
const RECENT_SCAN: i64 = 20_000;
const USAGE_MOST: usize = 200;
const DUPLICATES_DEFAULT: u64 = 50;
const DUPLICATES_MOST: u64 = 500;
/// How long looking for duplicates may take before it gives up (it hashes files it has not
/// seen before, which is slow the first time).
const DUPLICATES_TIME: std::time::Duration = std::time::Duration::from_secs(60);
/// The file index keeps times in nanoseconds.
const NANOS: i64 = 1_000_000_000;

pub fn tools() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    vec![
        json!({
            "name": "recent_files",
            "title": "Recently changed files",
            "description": "Lists the files changed or added most recently, newest first, optionally only since a date or in one folder.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Only this folder. Leave out for every folder this key can use." },
                "since": { "type": "string", "description": "Only files changed on or after this day, as YYYY-MM-DD." },
                "limit": { "type": "integer", "minimum": 1, "maximum": RECENT_MOST, "description": "How many files to return." },
            } },
            "annotations": read_only,
        }),
        json!({
            "name": "find_duplicates",
            "title": "Find duplicate files",
            "description": "Finds files with exactly the same contents in a folder and the folders inside it, most wasted space first. The first time in a large folder it can take a while, as files are read once to compare them.",
            "inputSchema": { "type": "object", "required": ["path"], "properties": {
                "path": { "type": "string", "description": "The folder to look in." },
                "limit": { "type": "integer", "minimum": 1, "maximum": DUPLICATES_MOST, "description": "How many groups of copies to return." },
            } },
            "annotations": read_only,
        }),
        json!({
            "name": "folder_usage",
            "title": "What takes up space",
            "description": "Returns how much space a folder takes, and its sub-folders and largest files, biggest first.",
            "inputSchema": { "type": "object", "required": ["path"], "properties": {
                "path": { "type": "string", "description": "The folder." },
            } },
            "annotations": read_only,
        }),
    ]
}

pub(crate) async fn call(state: &Arc<AppState>, key: &AgentKey, name: &str, args: &Args<'_>) -> Option<Result<Vec<Value>, Stop>> {
    Some(match name {
        "recent_files" => recent_files(state, key, &args.path(), &args.text("since"), args.number("limit").unwrap_or(RECENT_DEFAULT)).await,
        "folder_usage" => folder_usage(state, key, &args.path()).await,
        "find_duplicates" => find_duplicates(state, key, &args.path(), args.number("limit").unwrap_or(DUPLICATES_DEFAULT)).await,
        _ => return None,
    })
}

fn not_ready(state: &AppState) -> Option<Stop> {
    (!state.file_index.ready()).then(|| Stop::Failed("OwnNAS is still reading the library for the first time. Try again in a few minutes.".to_string()))
}

/// SQL for "in `folder` and not hidden" on `path`. Folders are written as values, not into the text.
const UNDER: &str = "(?1 = '' OR path >= ?1 || '/' AND path < ?1 || '0') AND path NOT LIKE '.%' AND path NOT LIKE '%/.%'";

async fn recent_files(state: &Arc<AppState>, key: &AgentKey, path: &str, since: &str, limit: u64) -> Result<Vec<Value>, Stop> {
    if let Some(stop) = not_ready(state) {
        return Err(stop);
    }
    let from = if since.trim().is_empty() {
        0
    } else {
        let day = chrono::NaiveDate::parse_from_str(since.trim(), "%Y-%m-%d").map_err(|_| Stop::Failed("Give the day as YYYY-MM-DD.".to_string()))?;
        day.and_hms_opt(0, 0, 0).map(|at| at.and_utc().timestamp()).unwrap_or(0).saturating_mul(NANOS)
    };
    let places = if path.is_empty() {
        key.folders.clone()
    } else {
        let found = allowed(state, key, path).await?;
        if !found.full.is_dir() {
            return failed("That is a file, not a folder.");
        }
        vec![found.rel]
    };
    let limit = limit.clamp(1, RECENT_MOST) as usize;
    let (worker, root, folders) = (state.clone(), state.root.clone(), key.folders.clone());
    blocking(move || {
        let fail = |_| Stop::Failed("Could not read the file index.".to_string());
        let mut rows: Vec<(String, i64, i64, String)> = Vec::new();
        {
            let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
            let mut stmt = conn
                .prepare(&format!("SELECT path, size, mtime, kind FROM idx.file_index WHERE dir = 0 AND mtime >= ?2 AND {UNDER} ORDER BY mtime DESC LIMIT ?3"))
                .map_err(fail)?;
            for place in &places {
                let found = stmt.query_map(params![place, from, RECENT_SCAN], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).map_err(fail)?;
                rows.extend(found.filter_map(Result::ok));
            }
        }
        rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        rows.dedup_by(|a, b| a.0 == b.0);
        let mut shown = Vec::new();
        let mut more = false;
        for (path, size, mtime, kind) in rows {
            if agent_keys::check(&root, &folders, &path).is_none() || crate::files::is_under_trash(&path) {
                continue;
            }
            if shown.len() == limit {
                more = true;
                break;
            }
            shown.push(json!({ "path": path, "size": size, "kind": kind, "modified": format!("{} UTC", crate::photo_details::wall_clock(mtime / NANOS)) }));
        }
        Ok(as_text(&json!({ "files": shown, "more": more })))
    })
    .await
}

async fn folder_usage(state: &Arc<AppState>, key: &AgentKey, path: &str) -> Result<Vec<Value>, Stop> {
    if let Some(stop) = not_ready(state) {
        return Err(stop);
    }
    let found = allowed(state, key, path).await?;
    if !found.full.is_dir() {
        return failed("That is a file, not a folder.");
    }
    let (worker, root, folders, rel) = (state.clone(), state.root.clone(), key.folders.clone(), found.rel.clone());
    blocking(move || {
        let fail = |_| Stop::Failed("Could not read the file index.".to_string());
        let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
        let total = |folder: &str| -> Result<(i64, i64), Stop> {
            conn.query_row(&format!("SELECT coalesce(sum(size), 0), count(*) FROM idx.file_index WHERE dir = 0 AND {UNDER}"), params![folder], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(fail)
        };
        let (bytes, count) = total(&rel)?;
        let children: Vec<(String, bool, i64)> = {
            let mut stmt = conn.prepare("SELECT path, dir, size FROM idx.file_index WHERE parent = ?1 AND name_lower NOT LIKE '.%'").map_err(fail)?;
            let rows = stmt.query_map([&rel], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(fail)?;
            rows.filter_map(Result::ok).collect()
        };
        let mut sub = Vec::new();
        let mut files = Vec::new();
        for (path, dir, size) in children {
            if crate::files::is_under_trash(&path) || agent_keys::check(&root, &folders, &path).is_none() {
                continue;
            }
            if dir {
                let (bytes, count) = total(&path)?;
                sub.push(json!({ "path": path, "bytes": bytes, "files": count }));
            } else {
                files.push(json!({ "path": path, "bytes": size }));
            }
        }
        let by_size = |list: &mut Vec<Value>| list.sort_by_key(|entry| std::cmp::Reverse(entry["bytes"].as_i64().unwrap_or(0)));
        by_size(&mut sub);
        by_size(&mut files);
        sub.truncate(USAGE_MOST);
        files.truncate(20);
        Ok(as_text(&json!({ "path": rel, "bytes": bytes, "files": count, "folders": sub, "largestFiles": files })))
    })
    .await
}

async fn find_duplicates(state: &Arc<AppState>, key: &AgentKey, path: &str, limit: u64) -> Result<Vec<Value>, Stop> {
    if let Some(stop) = not_ready(state) {
        return Err(stop);
    }
    let found = allowed(state, key, path).await?;
    if !found.full.is_dir() {
        return failed("That is a file, not a folder.");
    }
    let conn = crate::http::own_connection(state).map_err(|_| Stop::Failed("Could not open the database.".to_string()))?;
    let cancel = crate::files::CancelFlag::new();
    let timer = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(DUPLICATES_TIME).await;
            cancel.cancel();
        })
    };
    let (root, folders, rel, limit) = (state.root.clone(), key.folders.clone(), found.rel.clone(), limit.clamp(1, DUPLICATES_MOST) as usize);
    let answer = blocking(move || {
        let report = crate::duplicates::find(&conn, &root, &rel, &cancel).map_err(|err| match err {
            crate::files::FileError::Cancelled => Stop::Failed("That took too long. Try a smaller folder; what was read so far is kept, so asking again goes faster.".to_string()),
            _ => Stop::Failed("Could not look for duplicates there.".to_string()),
        })?;
        // Copies the key may not see are left out, and groups left with one copy with them.
        let mut groups = Vec::new();
        let mut wasted = 0u64;
        for group in report.groups {
            let copies: Vec<Value> = group
                .copies
                .iter()
                .filter(|copy| agent_keys::check(&root, &folders, &copy.path).is_some())
                .map(|copy| json!({ "path": copy.path, "modified": format!("{} UTC", crate::photo_details::wall_clock(copy.modified)) }))
                .collect();
            if copies.len() < 2 {
                continue;
            }
            wasted += group.size * (copies.len() as u64 - 1);
            groups.push(json!({ "size": group.size, "copies": copies }));
        }
        let more = groups.len() > limit || report.truncated;
        groups.truncate(limit);
        Ok(as_text(&json!({ "path": rel, "groups": groups, "wastedBytes": wasted, "more": more })))
    })
    .await;
    timer.abort();
    answer
}
