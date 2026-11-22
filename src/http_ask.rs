//! Folder descriptions and Ask over HTTP (`folder_notes.rs`, `folder_ask.rs`): reading and
//! writing a folder's description, a suggested draft, descriptions in listings and search, Ask
//! itself, and its settings (on or off, and the languages, which choose the model).

use std::sync::Arc;

use super::*;
use crate::folder_ask::{self, Embed, Meaning, TextModel};
use crate::folder_notes::{self, Note};

fn failed(err: String) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err)
}

/// A folder that can be described: inside the library (not the top of it) and outside the Trash.
fn describable(state: &AppState, path: &str) -> Result<String, ApiError> {
    let folder = files::resolve(&state.root, path)?;
    if folder.rel.is_empty() || folder.rel == files::TRASH_DIR || folder.rel.starts_with(&format!("{}/", files::TRASH_DIR)) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a folder inside the library, outside the Trash"));
    }
    if !folder.full.is_dir() {
        return Err(FileError::NotADirectory.into());
    }
    Ok(folder.rel)
}

/// Saves a description and/or the leave-out mark, and has Ask read it.
pub(super) fn save_description(state: &AppState, user: &User, rel: &str, description: Option<&str>, leave_out: Option<bool>) -> Result<Option<Note>, ApiError> {
    let note = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        folder_notes::set(&conn, rel, description, leave_out, &user.username).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?
    };
    state.ask.wake();
    let what = match (description.is_some(), leave_out) {
        (_, Some(true)) => "left out of Ask",
        (_, Some(false)) => "back in Ask",
        _ => "described",
    };
    record(state, user, "folder-description", &format!("{rel} ({what})"));
    Ok(note)
}

/// What the folder view shows at the top: the folder's own description, or the one it belongs
/// to above it, and whether it is left out of Ask. Null at the top of the library.
pub(super) fn note_for(state: &AppState, access: &Access, rel: &str) -> Result<serde_json::Value, ApiError> {
    if rel.is_empty() || rel == files::TRASH_DIR || rel.starts_with(&format!("{}/", files::TRASH_DIR)) {
        return Ok(serde_json::Value::Null);
    }
    let mut standing = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        folder_notes::standing(&conn, rel).map_err(failed)?
    };
    // Above an account's folders it sees only the way to them: no descriptions from there.
    if standing.inherited.as_ref().is_some_and(|note| !access.holds(&note.path)) {
        standing.inherited = None;
    }
    if standing.left_out_by.as_ref().is_some_and(|path| !access.holds(path)) {
        standing.left_out_by = None;
    }
    Ok(json!(standing))
}

/// Adds each folder's own description to a list of entries or search hits.
pub(super) fn decorate_descriptions(state: &AppState, entries: &mut serde_json::Value) -> Result<(), ApiError> {
    let Some(entries) = entries.as_array_mut() else { return Ok(()) };
    let paths: Vec<String> = entries.iter().filter(|entry| entry["dir"] == true).filter_map(|entry| entry["path"].as_str().map(String::from)).collect();
    if paths.is_empty() {
        return Ok(());
    }
    let notes = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        folder_notes::for_paths(&conn, &paths).map_err(failed)?
    };
    for entry in entries {
        let Some(note) = entry["path"].as_str().and_then(|path| notes.get(path)) else { continue };
        if !note.description.is_empty() {
            entry["description"] = json!(note.description);
        }
        if note.leave_out {
            entry["leaveOut"] = json!(true);
        }
    }
    Ok(())
}

/// Search finds folders by their descriptions too: those come first, as folder results with
/// their description (a folder also found by name is marked, not listed twice).
pub(super) async fn add_described_folders(state: &Arc<AppState>, rel: &str, needle: &str, hits: &mut Vec<files::SearchHit>) -> Result<(), ApiError> {
    let found = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        folder_notes::search(&conn, rel, needle, 50).map_err(failed)?
    };
    if found.is_empty() {
        return Ok(());
    }
    let mut extra = Vec::new();
    for (path, description) in &found {
        if let Some(hit) = hits.iter_mut().find(|hit| &hit.path == path) {
            hit.description = Some(description.clone());
            hit.matched_description = true;
        } else {
            extra.push((path.clone(), description.clone()));
        }
    }
    let root = state.root.clone();
    let extra = tokio::task::spawn_blocking(move || {
        extra
            .into_iter()
            .filter_map(|(path, description)| {
                let meta = root.join(&path).metadata().ok().filter(|meta| meta.is_dir())?;
                Some(files::SearchHit {
                    name: path.rsplit('/').next().unwrap_or(&path).to_string(),
                    dir: true,
                    size: 0,
                    modified: meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0),
                    kind: "folder".into(),
                    matched_tag: None,
                    matched_content: None,
                    description: Some(description),
                    matched_description: true,
                    path,
                })
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|_| failed("Search failed".into()))?;
    hits.splice(0..0, extra);
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct NoteQuery {
    path: Option<String>,
}

/// A folder's description, the one it belongs to, and whether this account may change it.
pub(super) async fn get_note(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(query): Query<NoteQuery>) -> Result<impl IntoResponse, ApiError> {
    let (user, access) = require_access(&state, &headers)?;
    let rel = describable(&state, &rel_of(&query.path))?;
    let mut value = note_for(&state, &access, &rel)?;
    value["path"] = json!(rel);
    value["canEdit"] = json!(!state.readonly && !user.read_only && access.check(&state.root, &rel, Need::Write).is_ok());
    Ok(Json(value))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NoteBody {
    path: String,
    description: Option<String>,
    leave_out: Option<bool>,
}

pub(super) async fn set_note(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(body): Json<NoteBody>) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, access) = require_access(&state, &headers)?;
    require_write(&state)?;
    let rel = describable(&state, &body.path)?;
    check_access(&state, &access, &rel, Need::Write)?;
    save_description(&state, &user, &rel, body.description.as_deref(), body.leave_out)?;
    let mut value = note_for(&state, &access, &rel)?;
    value["path"] = json!(rel);
    value["canEdit"] = json!(true);
    Ok(Json(value))
}

/// A draft description made from what the indexes know about the folder (no model).
pub(super) async fn suggest(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(query): Query<NoteQuery>) -> Result<impl IntoResponse, ApiError> {
    require_access(&state, &headers)?;
    let rel = describable(&state, &rel_of(&query.path))?;
    let db_path = state.db_path.clone();
    let draft = tokio::task::spawn_blocking(move || {
        let conn = db::connect(&db_path).map_err(|err| err.to_string())?;
        folder_notes::suggest(&conn, &rel)
    })
    .await
    .map_err(|_| failed("Could not suggest a description".into()))?
    .map_err(failed)?;
    Ok(Json(json!({ "draft": draft })))
}

/// The model Ask uses now: on, for the languages turned on, and downloaded.
fn current_model(state: &AppState, conn: &rusqlite::Connection) -> Option<&'static TextModel> {
    if !folder_ask::enabled(conn) {
        return None;
    }
    let model = folder_ask::model_for(&folder_ask::languages(conn));
    #[cfg(ownnas_ml)]
    if state.models.ready(crate::ml::Feature::for_text_model(model)) {
        return Some(model);
    }
    let _ = (state, model);
    None
}

/// Ask's answer, by meaning when the model is on and here, else by words, among what
/// `reachable` allows. Blocking: run it away from the async threads.
pub(crate) fn answer_question(state: &AppState, question: &str, scope: &str, reachable: &dyn Fn(&str) -> bool) -> Result<serde_json::Value, String> {
    let conn = db::connect(&state.db_path).map_err(|err| err.to_string())?;
    let model = current_model(state, &conn);
    #[cfg(ownnas_ml)]
    let vector = model.and_then(|model| state.models.describe_sentence(crate::ml::Feature::for_text_model(model), question, true, 2).ok());
    #[cfg(not(ownnas_ml))]
    let vector: Option<Vec<f32>> = None;
    let meaning = match (model, &vector) {
        (Some(model), Some(vector)) => Some(Meaning { model: model.id, vector }),
        _ => None,
    };
    let answer = folder_ask::ask_where(&conn, &state.root, question, scope, meaning, reachable)?;
    let mut value = json!(answer);
    value["modelOn"] = json!(folder_ask::enabled(&conn));
    Ok(value)
}

#[derive(Deserialize)]
pub(super) struct AskQuery {
    q: String,
    path: Option<String>,
}

/// The described folders that answer a question best, with the files in each that fit it.
pub(super) async fn ask(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(query): Query<AskQuery>) -> Result<impl IntoResponse, ApiError> {
    let (_, access) = require_access(&state, &headers)?;
    let question: String = query.q.trim().chars().take(300).collect();
    if question.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Ask a question"));
    }
    let scope = files::resolve(&state.root, &rel_of(&query.path))?.rel;
    let worker = state.clone();
    let answer = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let root = worker.root.clone();
        let reachable = |path: &str| access.holds(path) && (!access.limited() || access.check(&root, path, Need::Read).is_ok());
        answer_question(&worker, &question, &scope, &reachable)
    })
    .await
    .map_err(|_| failed("Could not answer".into()))?
    .map_err(failed)?;
    Ok(Json(answer))
}

/// Has the background reader turn new descriptions into vectors with the current model.
pub(super) fn start_ask_index(state: &AppState) {
    let media = state.media.clone();
    #[cfg(ownnas_ml)]
    let (ready, embedder) = (state.models.clone(), state.models.clone());
    let current = move |conn: &rusqlite::Connection| -> Option<&'static TextModel> {
        if !folder_ask::enabled(conn) {
            return None;
        }
        let model = folder_ask::model_for(&folder_ask::languages(conn));
        #[cfg(ownnas_ml)]
        if ready.ready(crate::ml::Feature::for_text_model(model)) {
            return Some(model);
        }
        let _ = model;
        None
    };
    let embed: Arc<Embed> = Arc::new(move |model: &TextModel, text: &str, question: bool, threads: usize| {
        #[cfg(ownnas_ml)]
        return embedder.describe_sentence(crate::ml::Feature::for_text_model(model), text, question, threads);
        #[cfg(not(ownnas_ml))]
        {
            let _ = (model, text, question, threads);
            Err("Ask's model is not available on this device".to_string())
        }
    });
    state.ask.start(state.db_path.clone(), move || media.speed(), current, embed);
}

/// Ask's settings and how far it is, for Settings and the search box.
pub(super) async fn settings(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    let (user, _) = require_user(&state, &headers)?;
    let (enabled, languages) = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        (folder_ask::enabled(&conn), folder_ask::languages(&conn))
    };
    let model = folder_ask::model_for(&languages);
    #[cfg(ownnas_ml)]
    let (available, ready, download) = {
        let feature = crate::ml::Feature::for_text_model(model);
        (true, state.models.ready(feature), json!(state.models.download_state(feature)))
    };
    #[cfg(not(ownnas_ml))]
    let (available, ready, download) = (false, false, serde_json::Value::Null);
    let offered: Vec<serde_json::Value> = folder_ask::LANGUAGES.iter().map(|(code, name)| json!({ "code": code, "name": name })).collect();
    Ok(Json(json!({
        "available": available,
        "enabled": enabled,
        "languages": languages,
        "offered": offered,
        "model": model,
        "models": [&folder_ask::ENGLISH, &folder_ask::MULTILINGUAL],
        "ready": ready,
        "download": download,
        "index": state.ask.status(),
        "canChange": user.is_admin,
    })))
}

#[derive(Deserialize)]
pub(super) struct AskSettingsBody {
    enabled: Option<bool>,
    languages: Option<Vec<String>>,
}

/// Turns Ask's model on or off and chooses the languages (administrators). Turned on, the model
/// for the languages is downloaded if it is not here yet, and descriptions are read with it; a
/// change of model reads them all again and deletes the other model's files.
pub(super) async fn set_settings(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(body): Json<AskSettingsBody>) -> Result<Response, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    let (enabled, languages) = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(languages) = &body.languages {
            let codes = folder_ask::parse_languages(&languages.join(","));
            db::set_setting(&conn, folder_ask::SETTING_LANGUAGES, Some(&codes.join(","))).map_err(failed)?;
        }
        if let Some(on) = body.enabled {
            #[cfg(not(ownnas_ml))]
            if on {
                return Err(ApiError::new(StatusCode::BAD_REQUEST, "Ask's model is not available on this device; Ask still finds folders by the words in their descriptions."));
            }
            db::set_setting(&conn, folder_ask::SETTING_ON, Some(if on { "1" } else { "0" })).map_err(failed)?;
        }
        (folder_ask::enabled(&conn), folder_ask::languages(&conn))
    };
    let model = folder_ask::model_for(&languages);
    record(&state, &user, "ask-settings", &format!("{} ({}, {})", if enabled { "on" } else { "off" }, languages.join(", "), model.name));
    #[cfg(ownnas_ml)]
    if enabled {
        use crate::ml::Feature;
        let feature = Feature::for_text_model(model);
        let other = if feature == Feature::AskEnglish { Feature::AskMultilingual } else { Feature::AskEnglish };
        let (ask, models) = (state.ask.clone(), state.models.clone());
        let switched = move || {
            models.remove(other);
            ask.wake();
        };
        if state.models.ready(feature) {
            switched();
        } else {
            state.models.download(feature, switched);
        }
    }
    state.ask.wake();
    Ok(settings(State(state), headers).await?.into_response())
}
