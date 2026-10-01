use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, ReadBuf};
use tokio_util::io::ReaderStream;

use crate::auth::{self, COOKIE_NAME};
use crate::db::{self, User};
use crate::files::{self, FileError};
use crate::preview;
use crate::thumbs;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub root: PathBuf,
    pub thumbs: PathBuf,
    pub readonly: bool,
    pub secure_cookie: bool,
    pub ffmpeg: bool,
    pub attempts: Mutex<AttemptGate>,
}

pub struct AttemptGate {
    users: std::collections::HashMap<String, (u32, Instant)>,
    global: (u32, Instant),
}

impl AttemptGate {
    fn new() -> Self {
        Self {
            users: std::collections::HashMap::new(),
            global: (0, Instant::now()),
        }
    }
}

pub fn new_state(
    root: PathBuf,
    data_dir: PathBuf,
    readonly: bool,
    secure_cookie: bool,
    ffmpeg: bool,
) -> Result<AppState, String> {
    let db_path = data_dir.join("ownnas.db");
    let conn = db::open(&db_path)?;
    let thumbs = data_dir.join("thumbs");
    std::fs::create_dir_all(&thumbs).map_err(|_| "Could not create the thumbnail cache".to_string())?;
    Ok(AppState {
        db: Mutex::new(conn),
        root,
        thumbs,
        readonly,
        secure_cookie,
        ffmpeg,
        attempts: Mutex::new(AttemptGate::new()),
    })
}

pub fn router(state: std::sync::Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/app.css", get(css))
        .route("/assets/app.js", get(javascript))
        .route("/api/health", get(health))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/password", post(change_password))
        .route("/api/users", get(list_users).post(create_managed_user))
        .route("/api/users/password", post(admin_set_password))
        .route("/api/users/admin", post(set_user_admin))
        .route("/api/users/remove", post(remove_managed_user))
        .route("/api/list", get(list))
        .route("/api/meta", get(meta))
        .route("/api/raw", get(raw))
        .route("/api/thumb", get(thumb))
        .route("/api/mkdir", post(mkdir))
        .route("/api/create", post(create_entry))
        .route("/api/write", post(write_entry))
        .route("/api/rename", post(rename))
        .route("/api/upload", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/api/upload/conflicts", post(upload_conflicts))
        .route("/api/entry", axum::routing::delete(delete_entry))
        .route("/api/restore", post(restore_item))
        .route("/api/trash/empty", post(empty_trash))
        .route("/api/bookmarks", get(list_bookmarks).post(add_bookmark).delete(remove_bookmark))
        .route("/api/recent", get(list_recent).post(touch_recent))
        .route("/api/search", get(search))
        .route("/api/usage", get(folder_usage))
        .route("/api/hash", get(file_hash))
        .route("/api/activity", get(activity))
        .route("/api/zip", get(zip_folder))
        .route("/api/download", post(download_selection))
        .route("/api/move", post(move_item))
        .route("/api/copy", post(copy_item))
        .route("/api/duplicate", post(duplicate_item))
        .route("/api/annotations", get(get_annotations).post(ack_annotations))
        .route("/api/tags", post(add_tag).delete(remove_tag))
        .route("/api/comments", post(add_comment).delete(remove_comment))
        .with_state(state)
}

pub async fn serve(state: std::sync::Arc<AppState>, addr: &str, open_browser: bool) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|_| format!("Could not listen on {addr}. The port may already be in use."))?;
    let bound = listener
        .local_addr()
        .map_err(|_| "Could not read the listen address".to_string())?;
    print_banner(&state, bound);
    if open_browser {
        let url = format!("http://127.0.0.1:{}", bound.port());
        open_url(&url);
    }
    let app = router(state);
    axum::serve(listener, app)
        .await
        .map_err(|_| "The server stopped because of a network error".to_string())
}

fn print_banner(state: &AppState, bound: SocketAddr) {
    let local = match bound.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => format!("http://127.0.0.1:{}", bound.port()),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => format!("http://[::1]:{}", bound.port()),
        _ => format!("http://{bound}"),
    };
    println!("OwnNAS is listening on http://{bound}");
    println!("On this machine: {local}");
    println!("Sharing: {}", state.root.display());
    println!(
        "Database: {}",
        state.thumbs.parent().unwrap_or(&state.thumbs).join("ownnas.db").display()
    );
    if state.readonly {
        println!("Mode: read-only");
    }
    if state.ffmpeg {
        println!("Video thumbnails: ffmpeg");
    } else {
        println!("Video thumbnails: install ffmpeg and restart OwnNAS to enable them");
    }
    println!("Use the VPN address of this computer from other devices. Press Ctrl+C to stop.");
}

fn open_url(url: &str) {
    let result = {
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn()
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(url).spawn()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open").arg(url).spawn()
        }
    };
    if let Err(err) = result {
        eprintln!("Could not open a browser: {err}");
    }
}

struct ApiError {
    status: StatusCode,
    body: serde_json::Value,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: json!({ "error": message.into() }),
        }
    }

    fn conflict(existing: files::ExistingFile) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            body: json!({
                "error": "An item with that name already exists",
                "exists": true,
                "dir": existing.dir,
                "modified": existing.modified,
                "size": existing.size,
            }),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<FileError> for ApiError {
    fn from(err: FileError) -> Self {
        match err {
            FileError::Forbidden => ApiError::new(StatusCode::FORBIDDEN, "That path is outside the shared folder"),
            FileError::NotFound => ApiError::new(StatusCode::NOT_FOUND, "Not found"),
            FileError::NotADirectory => ApiError::new(StatusCode::BAD_REQUEST, "That path is not a folder"),
            FileError::IsADirectory => ApiError::new(StatusCode::BAD_REQUEST, "That path is a folder"),
            FileError::AlreadyExists => {
                ApiError::new(StatusCode::CONFLICT, "An item with that name already exists")
            }
            FileError::InvalidName => ApiError::new(StatusCode::BAD_REQUEST, "That name is not allowed"),
            FileError::Rejected(message) => ApiError::new(StatusCode::BAD_REQUEST, message),
            FileError::Io(message) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message),
        }
    }
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct PasswordBody {
    current: String,
    new: String,
}

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
    download: Option<String>,
    hidden: Option<String>,
    slow: Option<String>,
}

#[derive(Deserialize)]
struct UploadQuery {
    path: Option<String>,
    conflict: Option<String>,
    modified: Option<i64>,
}

#[derive(Deserialize)]
struct ConflictBody {
    path: Option<String>,
    names: Vec<String>,
}

#[derive(Deserialize)]
struct NameBody {
    path: Option<String>,
    name: String,
}

#[derive(Deserialize)]
struct CreateBody {
    path: Option<String>,
    name: String,
    kind: String,
}

#[derive(Deserialize)]
struct WriteBody {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct RenameBody {
    path: String,
    name: String,
}

fn flag(value: &Option<String>) -> bool {
    matches!(value.as_deref(), Some("1") | Some("true") | Some("yes"))
}

fn rel_of(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

fn check_csrf(headers: &HeaderMap) -> Result<(), ApiError> {
    match headers.get("x-ownnas").and_then(|v| v.to_str().ok()) {
        Some("1") => Ok(()),
        _ => Err(ApiError::new(StatusCode::FORBIDDEN, "Missing request header")),
    }
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    auth::read_cookie(raw, COOKIE_NAME)
}

fn require_user(state: &AppState, headers: &HeaderMap) -> Result<(User, String), ApiError> {
    let token = session_token(headers)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "Sign in required"))?;
    let user = state
        .db
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .pipe(|conn| db::user_for_token(&conn, &token))
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "Sign in required"))?;
    Ok((user, token))
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

fn require_write(state: &AppState) -> Result<(), ApiError> {
    if state.readonly {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "This OwnNAS server is read-only",
        ))
    } else {
        Ok(())
    }
}

fn require_admin(user: &User) -> Result<(), ApiError> {
    if user.is_admin {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Administrator access required",
        ))
    }
}

fn login_allowed(gate: &mut AttemptGate, username: &str) -> bool {
    let now = Instant::now();
    let window = Duration::from_secs(10 * 60);
    if now.duration_since(gate.global.1) > window {
        gate.global = (0, now);
    }
    if gate.global.0 >= 40 {
        return false;
    }
    let key = username.trim().to_ascii_lowercase();
    let entry = gate.users.entry(key).or_insert((0, now));
    if now.duration_since(entry.1) > window {
        *entry = (0, now);
    }
    entry.0 < 8
}

fn login_failed(gate: &mut AttemptGate, username: &str) {
    let now = Instant::now();
    gate.global.0 += 1;
    if gate.global.1.elapsed() > Duration::from_secs(10 * 60) {
        gate.global = (1, now);
    }
    let key = username.trim().to_ascii_lowercase();
    let entry = gate.users.entry(key).or_insert((0, now));
    entry.0 += 1;
}

fn login_succeeded(gate: &mut AttemptGate, username: &str) {
    gate.users.remove(&username.trim().to_ascii_lowercase());
}

async fn index() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::X_FRAME_OPTIONS, "SAMEORIGIN"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; img-src 'self'; media-src 'self'; frame-src 'self'; style-src 'self'; script-src 'self'; base-uri 'none'; form-action 'self'",
            ),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        INDEX_HTML,
    )
}

async fn css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        APP_CSS,
    )
}

async fn javascript() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        APP_JS,
    )
}

async fn health() -> impl IntoResponse {
    Json(json!({ "ok": true }))
}

async fn login(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    {
        let mut gate = state.attempts.lock().unwrap_or_else(|err| err.into_inner());
        if !login_allowed(&mut gate, &body.username) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many sign-in attempts. Wait a few minutes and try again.",
            ));
        }
    }
    let user = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::find_user(&conn, &body.username)
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?
    };
    let valid = user
        .as_ref()
        .is_some_and(|user| auth::verify_password(&body.password, &user.password_hash));
    if !valid {
        let mut gate = state.attempts.lock().unwrap_or_else(|err| err.into_inner());
        login_failed(&mut gate, &body.username);
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password",
        ));
    }
    let user = user.unwrap();
    let token = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let token = db::create_session(&conn, user.id)
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
        let _ = db::log_event(&conn, Some(user.id), &user.username, "sign-in", "");
        token
    };
    {
        let mut gate = state.attempts.lock().unwrap_or_else(|err| err.into_inner());
        login_succeeded(&mut gate, &body.username);
    }
    Ok((
        [(
            header::SET_COOKIE,
            auth::session_cookie(&token, state.secure_cookie),
        )],
        Json(json!({ "username": user.username })),
    ))
}

async fn logout(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    if let Some(token) = session_token(&headers) {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        if let Ok(Some(user)) = db::user_for_token(&conn, &token) {
            let _ = db::log_event(&conn, Some(user.id), &user.username, "sign-out", "");
        }
        let _ = db::delete_session(&conn, &token);
    }
    Ok((
        [(
            header::SET_COOKIE,
            auth::clear_cookie(state.secure_cookie),
        )],
        Json(json!({ "ok": true })),
    ))
}

async fn me(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let (user, _) = require_user(&state, &headers)?;
    Ok(Json(json!({
        "username": user.username,
        "admin": user.is_admin,
        "readonly": state.readonly,
        "version": env!("CARGO_PKG_VERSION"),
        "rootName": files::root_label(&state.root),
        "ffmpeg": state.ffmpeg,
    })))
}

async fn list_users(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let users = db::list_user_infos(&conn)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!({ "users": users })))
}

#[derive(Deserialize)]
struct ManagedUserBody {
    username: String,
    password: String,
    #[serde(default)]
    admin: bool,
}

async fn create_managed_user(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ManagedUserBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::create_user_with_admin(&conn, &body.username, &body.password, body.admin)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    record(
        &state,
        &user,
        "user-add",
        &format!("{}{}", body.username.trim(), if body.admin { " (admin)" } else { "" }),
    );
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AdminPasswordBody {
    username: String,
    password: String,
}

async fn admin_set_password(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AdminPasswordBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::set_password(&conn, &body.username, &body.password)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    record(&state, &user, "user-password", body.username.trim());
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AdminFlagBody {
    username: String,
    admin: bool,
}

async fn set_user_admin(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AdminFlagBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::set_user_admin(&conn, &body.username, body.admin)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    record(
        &state,
        &user,
        if body.admin { "user-admin-grant" } else { "user-admin-revoke" },
        body.username.trim(),
    );
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RemoveUserBody {
    username: String,
}

async fn remove_managed_user(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RemoveUserBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    if user.username.eq_ignore_ascii_case(body.username.trim()) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "You cannot remove your own account while signed in",
        ));
    }
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::delete_user(&conn, &body.username)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    record(&state, &user, "user-remove", body.username.trim());
    Ok(Json(json!({ "ok": true })))
}

async fn change_password(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, token) = require_user(&state, &headers)?;
    if !auth::verify_password(&body.current, &user.password_hash) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Current password is wrong"));
    }
    auth::validate_password(&body.new).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::set_password(&conn, &user.username, &body.new)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    db::delete_other_sessions(&conn, user.id, &token)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    let new_token = db::create_session(&conn, user.id)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    let _ = db::delete_session(&conn, &token);
    Ok((
        [(
            header::SET_COOKIE,
            auth::session_cookie(&new_token, state.secure_cookie),
        )],
        Json(json!({ "ok": true })),
    ))
}

async fn list(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let migrations = tokio::task::spawn_blocking({
        let root = root.clone();
        move || files::migrate_legacy_dirs(&root)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not prepare special folders"))?
    .map_err(ApiError::from)?;
    for (from, to) in migrations {
        rewrite_meta(&state, &from, &to);
    }
    let rel = rel_of(&query.path);
    let hidden = flag(&query.hidden);
    let ffmpeg = state.ffmpeg;
    let mut listing = tokio::task::spawn_blocking(move || files::list_dir(&root, &rel, hidden, ffmpeg))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not list the folder"))?
        .map_err(ApiError::from)?;
    let paths: Vec<String> = listing.entries.iter().map(|entry| entry.path.clone()).collect();
    if !paths.is_empty() {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        if let Ok(map) = db::tags_for_paths(&conn, &paths) {
            for entry in &mut listing.entries {
                if let Some(tags) = map.get(&entry.path) {
                    entry.tags = tags.clone();
                }
            }
        }
        for entry in &mut listing.entries {
            if !entry.dir {
                continue;
            }
            let Ok(stamp) = files::folder_stamp(&state.root, &entry.path) else {
                continue;
            };
            if let Ok(Some(row)) = db::get_folder_size(&conn, &entry.path) {
                if row.fingerprint == stamp {
                    entry.measured_size = Some(row.bytes);
                    entry.measured_truncated = Some(row.truncated);
                }
            }
        }
    }
    Ok(Json(listing))
}

async fn meta(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let payload = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, FileError> {
        let resolved = files::resolve(&root, &rel)?;
        if resolved.full.is_dir() {
            return Err(FileError::IsADirectory);
        }
        let name = resolved
            .full
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let file_meta = std::fs::metadata(&resolved.full).map_err(|_| FileError::NotFound)?;
        let inspection = preview::inspect(&resolved.full, &name);
        Ok(json!({
            "name": name,
            "path": resolved.rel,
            "size": file_meta.len(),
            "modified": file_meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0),
            "kind": files::kind_of(&name),
            "text": inspection.text,
            "archive": inspection.archive,
            "archiveTruncated": inspection.archive_truncated,
            "note": inspection.note,
        }))
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the file"))?
    .map_err(ApiError::from)?;
    Ok(Json(payload))
}

async fn raw(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Response, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let download = flag(&query.download);
    let resolved = files::resolve(&root, &rel)?;
    if resolved.full.is_dir() {
        return Err(FileError::IsADirectory.into());
    }
    let name = resolved
        .full
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let meta = tokio::fs::metadata(&resolved.full)
        .await
        .map_err(|_| ApiError::from(FileError::NotFound))?;
    let total = meta.len();
    let mime = raw_media_type(&resolved.full, &name);
    let sandbox = files::active_content(&name);
    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(|s| s.to_string());
    let span = match range_request(range_header.as_deref(), total) {
        RangeReq::Full => None,
        RangeReq::Partial(span) => Some(span),
        RangeReq::Unsatisfiable => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{total}"))],
            )
                .into_response());
        }
    };
    let mut file = tokio::fs::File::open(&resolved.full)
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not open the file"))?;
    let (status, start, len) = if let Some(span) = span {
        file.seek(std::io::SeekFrom::Start(span.start))
            .await
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the file"))?;
        (StatusCode::PARTIAL_CONTENT, span.start, span.len)
    } else {
        (StatusCode::OK, 0, total)
    };
    let stream = ReaderStream::new(file.take(len));
    let body = Body::from_stream(stream);
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, &mime)
        .header(header::CONTENT_LENGTH, len)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_DISPOSITION,
            files::content_disposition(&name, download || mime == "application/octet-stream"),
        )
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "private, max-age=60");
    if status == StatusCode::PARTIAL_CONTENT {
        let end = start + len.saturating_sub(1);
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    if sandbox {
        response = response.header(
            header::CONTENT_SECURITY_POLICY,
            "sandbox; default-src 'none'",
        );
    }
    response
        .body(body)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not send the file"))
}

fn raw_media_type(path: &std::path::Path, name: &str) -> String {
    let kind = files::kind_of(name);
    if kind == "text" || matches!(extension_of(name).as_str(), "html" | "htm" | "xhtml" | "xml" | "js" | "mjs")
    {
        return "text/plain; charset=utf-8".to_string();
    }
    if kind == "svg" {
        return "image/svg+xml".to_string();
    }
    mime_guess::from_path(path)
        .first_raw()
        .unwrap_or(if matches!(kind, "image" | "video" | "audio" | "pdf") {
            match kind {
                "pdf" => "application/pdf",
                "image" => "application/octet-stream",
                "video" => "video/mp4",
                "audio" => "audio/mpeg",
                _ => "application/octet-stream",
            }
        } else {
            "application/octet-stream"
        })
        .to_string()
}

fn extension_of(name: &str) -> String {
    std::path::Path::new(&name.to_ascii_lowercase())
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_string()
}

enum RangeReq {
    Full,
    Partial(ByteSpan),
    Unsatisfiable,
}

struct ByteSpan {
    start: u64,
    len: u64,
}

fn range_request(header: Option<&str>, total: u64) -> RangeReq {
    let Some(header) = header else {
        return RangeReq::Full;
    };
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return RangeReq::Full;
    };
    if spec.contains(',') || total == 0 {
        return RangeReq::Full;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return RangeReq::Full;
    };
    if start.is_empty() {
        let Ok(suffix) = end.parse::<u64>() else {
            return RangeReq::Full;
        };
        if suffix == 0 {
            return RangeReq::Unsatisfiable;
        }
        let len = suffix.min(total);
        return RangeReq::Partial(ByteSpan {
            start: total - len,
            len,
        });
    }
    let Ok(start) = start.parse::<u64>() else {
        return RangeReq::Full;
    };
    if start >= total {
        return RangeReq::Unsatisfiable;
    }
    let end_incl = if end.is_empty() {
        total - 1
    } else {
        match end.parse::<u64>() {
            Ok(value) => value.min(total - 1),
            Err(_) => return RangeReq::Full,
        }
    };
    if end_incl < start {
        return RangeReq::Unsatisfiable;
    }
    RangeReq::Partial(ByteSpan {
        start,
        len: end_incl - start + 1,
    })
}

async fn thumb(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Response, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let cache = state.thumbs.clone();
    let rel = rel_of(&query.path);
    let ffmpeg = state.ffmpeg;
    let image = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let resolved = files::resolve(&root, &rel).map_err(|_| "Not found".to_string())?;
        if resolved.full.is_dir() {
            return Err("Not found".into());
        }
        let name = resolved
            .full
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if !files::can_thumb(&name, ffmpeg) {
            return Err("No thumbnail".into());
        }
        let video = files::kind_of(&name) == "video";
        let cached = thumbs::ensure(&resolved.full, &cache, video)?;
        thumbs::read_cached(&cached)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not make a thumbnail"))?
    .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "No thumbnail"))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "private, max-age=86400"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        image,
    )
        .into_response())
}

async fn mkdir(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<NameBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let parent = rel_of(&body.path);
    let name = body.name;
    let label = if parent.is_empty() { name.clone() } else { format!("{parent}/{name}") };
    tokio::task::spawn_blocking(move || files::make_dir(&root, &parent, &name))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not create the folder"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &rel_of(&body.path));
    record(&state, &user, "create-folder", &label);
    Ok(Json(json!({ "ok": true })))
}

async fn create_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let parent = rel_of(&body.path);
    let name = body.name;
    let kind = body.kind;
    let created = tokio::task::spawn_blocking(move || files::create_file(&root, &parent, &name, &kind))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not create the file"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&created));
    record(&state, &user, "create-file", &created);
    Ok(Json(json!({ "ok": true, "path": created })))
}

async fn write_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WriteBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let path = body.path.clone();
    let content = body.content;
    tokio::task::spawn_blocking(move || files::write_text_file(&root, &path, &content))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the file"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&body.path));
    record(&state, &user, "edit-file", &body.path);
    Ok(Json(json!({ "ok": true })))
}

async fn rename(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RenameBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let from = body.path.clone();
    let label = format!("{} → {}", body.path, body.name);
    let to = tokio::task::spawn_blocking(move || files::rename_entry(&root, &body.path, &body.name))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not rename the item"))?
        .map_err(ApiError::from)?;
    rewrite_meta(&state, &from, &to);
    record(&state, &user, "rename", &label);
    Ok(Json(json!({ "ok": true, "path": to })))
}

async fn delete_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let label = rel.clone();
    let (action, new_path) = tokio::task::spawn_blocking(move || files::trash_or_delete(&root, &rel))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not delete the item"))?
        .map_err(ApiError::from)?;
    if let Some(to) = new_path {
        rewrite_meta(&state, &label, &to);
    } else {
        delete_meta(&state, &label);
    }
    record(&state, &user, action, &label);
    Ok(Json(json!({ "ok": true, "action": action })))
}

async fn restore_item(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let from = body.path.clone();
    let label = body.path.clone();
    let path = tokio::task::spawn_blocking(move || files::restore_entry(&root, &body.path))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not restore the item"))?
        .map_err(ApiError::from)?;
    rewrite_meta(&state, &from, &path);
    record(&state, &user, "restore", &label);
    Ok(Json(json!({ "path": path })))
}

async fn empty_trash(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let removed = tokio::task::spawn_blocking(move || files::empty_trash(&root))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not empty the Trash"))?
        .map_err(ApiError::from)?;
    delete_meta(&state, files::TRASH_DIR);
    delete_meta(&state, "Trash");
    record(&state, &user, "empty-trash", &format!("{removed} item(s)"));
    Ok(Json(json!({ "removed": removed })))
}

fn upload_choice(query: &UploadQuery) -> Result<Option<files::UploadChoice>, ApiError> {
    let Some(conflict) = query.conflict.as_deref() else {
        return Ok(None);
    };
    match conflict {
        "overwrite" => Ok(Some(files::UploadChoice::Overwrite)),
        "keep" => Ok(Some(files::UploadChoice::KeepBoth)),
        "archive-existing" => Ok(Some(files::UploadChoice::ArchiveExisting)),
        "ignore" => Ok(Some(files::UploadChoice::Ignore)),
        "archive-older" => {
            let incoming_modified = query.modified.ok_or_else(|| {
                ApiError::new(StatusCode::BAD_REQUEST, "Archive older needs the file date")
            })?;
            Ok(Some(files::UploadChoice::ArchiveOlder { incoming_modified }))
        }
        _ => Err(ApiError::new(StatusCode::BAD_REQUEST, "Unknown upload choice")),
    }
}

async fn upload_conflicts(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ConflictBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (_, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    if body.names.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose at least one file"));
    }
    if body.names.len() > files::MAX_LIST {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "That upload contains too many files",
        ));
    }
    let root = state.root.clone();
    let dir = rel_of(&body.path);
    let names = body.names;
    let items = tokio::task::spawn_blocking(move || {
        names
            .iter()
            .map(|name| files::upload_status(&root, &dir, name))
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not check the upload"))?
    .map_err(ApiError::from)?;
    Ok(Json(json!({ "items": items })))
}

async fn upload(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let choice = upload_choice(&query)?;
    let dir_rel = rel_of(&query.path);
    let mut saved = 0u32;
    let mut skipped = 0u32;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "The upload was interrupted"))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field
            .file_name()
            .map(|name| name.to_string())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "The upload is missing a file name"))?;
        let root = state.root.clone();
        let dir = dir_rel.clone();
        let prepared = tokio::task::spawn_blocking(move || files::prepare_upload(&root, &dir, &filename, choice))
            .await
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload"))?
            .map_err(ApiError::from)?;
        let (dest, archived_as) = match prepared {
            files::PreparedUpload::Skip => {
                skipped += 1;
                let mut field = field;
                while field
                    .chunk()
                    .await
                    .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "The upload was interrupted"))?
                    .is_some()
                {}
                continue;
            }
            files::PreparedUpload::Ask(existing) => return Err(ApiError::conflict(existing)),
            files::PreparedUpload::Write(dest) => (dest, None),
            files::PreparedUpload::StoreInArchive { path, original_name } => (path, Some(original_name)),
        };
        let mut out = tokio::fs::File::create(&dest).await.map_err(|_| {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload")
        })?;
        let mut field = field;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "The upload was interrupted"))?
        {
            tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
                .await
                .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload"))?;
        }
        drop(out);
        if let Some(original_name) = archived_as {
            let root = state.root.clone();
            let saved_path = dest.clone();
            tokio::task::spawn_blocking(move || files::record_archived_file(&root, &saved_path, &original_name))
                .await
                .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not update the archive log"))?
                .map_err(ApiError::from)?;
        }
        saved += 1;
    }
    if saved == 0 && skipped == 0 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose at least one file"));
    }
    if saved > 0 {
        invalidate_sizes(&state, &dir_rel);
        record(&state, &user, "upload", &format!("{saved} file(s)"));
    }
    Ok(Json(json!({ "saved": saved, "skipped": skipped })))
}

fn record(state: &AppState, user: &User, action: &str, detail: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::log_event(&conn, Some(user.id), &user.username, action, detail);
}

fn rewrite_meta(state: &AppState, from: &str, to: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::rewrite_path_meta(&conn, from, to);
}

fn delete_meta(state: &AppState, path: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::delete_path_meta(&conn, path);
}

fn invalidate_sizes(state: &AppState, path: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::invalidate_folder_sizes(&conn, path);
}

fn parent_rel_path(path: &str) -> String {
    match path.rfind('/') {
        Some(idx) => path[..idx].to_string(),
        None => String::new(),
    }
}

fn copy_meta(state: &AppState, from: &str, to: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::copy_path_meta(&conn, from, to);
}

#[derive(Deserialize)]
struct SearchQuery {
    path: Option<String>,
    q: Option<String>,
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

#[derive(Deserialize)]
struct PathsBody {
    paths: Vec<String>,
}

#[derive(Deserialize)]
struct MoveBody {
    path: String,
    dest: String,
}

#[derive(Deserialize)]
struct TagBody {
    path: String,
    tag: String,
}

#[derive(Deserialize)]
struct TagQuery {
    path: Option<String>,
    tag: Option<String>,
}

#[derive(Deserialize)]
struct CommentBody {
    path: String,
    body: String,
}

#[derive(Deserialize)]
struct CommentQuery {
    id: Option<i64>,
}

struct DeleteOnDrop<T> {
    inner: T,
    path: PathBuf,
}

impl<T> Drop for DeleteOnDrop<T> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for DeleteOnDrop<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

async fn list_bookmarks(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let (user, _) = require_user(&state, &headers)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let paths = db::list_bookmarks(&conn, user.id)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!({ "paths": paths })))
}

async fn add_bookmark(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    let root = state.root.clone();
    let requested = body.path;
    let rel = tokio::task::spawn_blocking(move || -> Result<String, FileError> {
        let resolved = files::resolve(&root, &requested)?;
        if !resolved.full.is_dir() {
            return Err(FileError::NotADirectory);
        }
        Ok(resolved.rel)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the bookmark"))?
    .map_err(ApiError::from)?;
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::add_bookmark(&conn, user.id, &rel)
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    }
    record(&state, &user, "bookmark", &rel);
    Ok(Json(json!({ "ok": true })))
}

async fn remove_bookmark(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    let rel = rel_of(&query.path);
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::remove_bookmark(&conn, user.id, &rel)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_recent(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let (user, _) = require_user(&state, &headers)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let items = db::list_recent(&conn, user.id)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!({ "items": items })))
}

async fn touch_recent(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    let root = state.root.clone();
    let requested = body.path;
    let (rel, name) = tokio::task::spawn_blocking(move || -> Result<(String, String), FileError> {
        let resolved = files::resolve(&root, &requested)?;
        if resolved.full.is_dir() {
            return Err(FileError::IsADirectory);
        }
        let name = resolved
            .full
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        Ok((resolved.rel, name))
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not update recent files"))?
    .map_err(ApiError::from)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::touch_recent(&conn, user.id, &rel, &name)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!({ "ok": true })))
}

async fn search(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let raw = query.q.unwrap_or_default();
    let trimmed = raw.trim();
    let (tag_only, exact_tag, needle) = if let Some(rest) = trimmed
        .strip_prefix("tag:")
        .or_else(|| trimmed.strip_prefix("TAG:"))
        .or_else(|| trimmed.strip_prefix("Tag:"))
    {
        (true, true, rest.trim().to_string())
    } else {
        (false, false, trimmed.to_string())
    };
    if needle.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Enter a search"));
    }
    if !tag_only && needle.chars().count() < 2 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Type at least 2 characters"));
    }

    let mut hits = if tag_only {
        Vec::new()
    } else {
        let root = root.clone();
        let rel = rel.clone();
        let needle = needle.clone();
        tokio::task::spawn_blocking(move || files::search(&root, &rel, &needle))
            .await
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Search failed"))?
            .map_err(ApiError::from)?
    };

    let tag_matches = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::search_tags(&conn, &needle, &rel, exact_tag)
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?
    };

    if !tag_matches.is_empty() {
        let root = state.root.clone();
        let existing: std::collections::HashSet<String> =
            hits.iter().map(|hit| hit.path.clone()).collect();
        let extras = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for (path, tag) in tag_matches {
                if existing.contains(&path) {
                    continue;
                }
                let Ok(resolved) = files::resolve(&root, &path) else {
                    continue;
                };
                let name = resolved
                    .full
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                let dir = resolved.full.is_dir();
                out.push(files::SearchHit {
                    kind: if dir {
                        "folder".to_string()
                    } else {
                        files::kind_of(&name).to_string()
                    },
                    name,
                    path,
                    dir,
                    matched_tag: Some(tag),
                });
                if out.len() >= 100 {
                    break;
                }
            }
            out
        })
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Search failed"))?;

        // Mark name hits that also have the tag.
        {
            let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
            for hit in &mut hits {
                if hit.matched_tag.is_some() {
                    continue;
                }
                if let Ok(tags) = db::list_tags(&conn, &hit.path) {
                    if let Some(tag) = tags.into_iter().find(|tag| {
                        if exact_tag {
                            tag.eq_ignore_ascii_case(&needle)
                        } else {
                            tag.to_lowercase().contains(&needle.to_lowercase())
                        }
                    }) {
                        hit.matched_tag = Some(tag);
                    }
                }
            }
        }
        hits.extend(extras);
    }

    Ok(Json(json!({ "hits": hits })))
}

async fn folder_usage(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let slow = flag(&query.slow);
    let stamp = tokio::task::spawn_blocking({
        let root = root.clone();
        let rel = rel.clone();
        move || files::folder_stamp(&root, &rel)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the folder"))?
    .map_err(ApiError::from)?;
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        if let Ok(Some(row)) = db::get_folder_size(&conn, &rel) {
            if row.fingerprint == stamp {
                return Ok(Json(files::Usage {
                    bytes: row.bytes,
                    files: row.files,
                    truncated: row.truncated,
                    cached: true,
                    fingerprint: row.fingerprint,
                    max_mtime: row.max_mtime,
                }));
            }
        }
    }
    let usage = tokio::task::spawn_blocking({
        let root = root.clone();
        let rel = rel.clone();
        move || {
            if slow {
                files::usage_paced(&root, &rel, true)
            } else {
                files::usage(&root, &rel)
            }
        }
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not measure the folder"))?
    .map_err(ApiError::from)?;
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let _ = db::invalidate_folder_size_ancestors(&conn, &rel);
        let _ = db::upsert_folder_size(
            &conn,
            &rel,
            usage.bytes,
            usage.files,
            usage.truncated,
            usage.max_mtime,
            &usage.fingerprint,
        );
    }
    Ok(Json(usage))
}

async fn file_hash(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let sha256 = tokio::task::spawn_blocking(move || files::sha256_file(&root, &rel))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not hash the file"))?
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "sha256": sha256 })))
}

async fn activity(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let events = db::list_events(&conn)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!({ "events": events })))
}

async fn move_item(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<MoveBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let from = body.path.clone();
    let label = format!("{} → {}", body.path, body.dest);
    let path = tokio::task::spawn_blocking(move || files::move_entry(&root, &body.path, &body.dest))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not move the item"))?
        .map_err(ApiError::from)?;
    rewrite_meta(&state, &from, &path);
    record(&state, &user, "move", &label);
    Ok(Json(json!({ "path": path })))
}

async fn copy_item(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<MoveBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let from = body.path.clone();
    let label = format!("{} → {}", body.path, body.dest);
    let path = tokio::task::spawn_blocking(move || files::copy_entry(&root, &body.path, &body.dest))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not copy the item"))?
        .map_err(ApiError::from)?;
    copy_meta(&state, &from, &path);
    invalidate_sizes(&state, &parent_rel_path(&path));
    record(&state, &user, "copy", &label);
    Ok(Json(json!({ "path": path })))
}

async fn duplicate_item(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let from = body.path.clone();
    let label = body.path.clone();
    let path = tokio::task::spawn_blocking(move || files::duplicate(&root, &body.path))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not duplicate the item"))?
        .map_err(ApiError::from)?;
    copy_meta(&state, &from, &path);
    invalidate_sizes(&state, &parent_rel_path(&path));
    record(&state, &user, "duplicate", &label);
    Ok(Json(json!({ "path": path })))
}

async fn get_annotations(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let path = rel_of(&query.path);
    if path.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a file or folder"));
    }
    let (size, modified) = files::entry_stats(&state.root, &path).map_err(ApiError::from)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let notes = db::list_annotations(&conn, &path, size as i64, modified)
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(notes))
}

async fn ack_annotations(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let path = body.path.trim().trim_start_matches('/').to_string();
    if path.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a file or folder"));
    }
    let (size, modified) = files::entry_stats(&state.root, &path).map_err(ApiError::from)?;
    let notes = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::touch_fingerprint(&conn, &path, size as i64, modified)
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
        db::list_annotations(&conn, &path, size as i64, modified)
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?
    };
    record(&state, &user, "ack-notes", &path);
    Ok(Json(notes))
}

async fn add_tag(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TagBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let path = body.path.trim().trim_start_matches('/').to_string();
    if path.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a file or folder"));
    }
    let (size, modified) = files::entry_stats(&state.root, &path).map_err(ApiError::from)?;
    let tag = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let tag = db::add_tag(&conn, &path, &body.tag)
            .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
        let _ = db::touch_fingerprint(&conn, &path, size as i64, modified);
        tag
    };
    record(&state, &user, "tag", &format!("{path} #{tag}"));
    Ok(Json(json!({ "tag": tag })))
}

async fn remove_tag(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TagQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let path = rel_of(&query.path);
    let tag = query.tag.unwrap_or_default();
    if path.is_empty() || tag.trim().is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a path and tag"));
    }
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::remove_tag(&conn, &path, &tag)
            .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    }
    record(&state, &user, "untag", &format!("{path} #{tag}"));
    Ok(Json(json!({ "ok": true })))
}

async fn add_comment(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CommentBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let path = body.path.trim().trim_start_matches('/').to_string();
    if path.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a file or folder"));
    }
    let (size, modified) = files::entry_stats(&state.root, &path).map_err(ApiError::from)?;
    let comment = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let comment = db::add_comment(&conn, &path, user.id, &user.username, &body.body)
            .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
        let _ = db::touch_fingerprint(&conn, &path, size as i64, modified);
        comment
    };
    record(&state, &user, "comment", &path);
    Ok(Json(comment))
}

async fn remove_comment(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CommentQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let id = query.id.ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "Choose a comment"))?;
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::remove_comment(&conn, id)
            .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    }
    record(&state, &user, "uncomment", &id.to_string());
    Ok(Json(json!({ "ok": true })))
}

async fn zip_folder(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Response, ApiError> {
    let (user, _) = require_user(&state, &headers)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let tmp = std::env::temp_dir().join(format!(
        "ownnas-zip-{}-{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let output = tmp.clone();
    let packed = rel.clone();
    let name = tokio::task::spawn_blocking(move || files::write_zip(&root, &packed, &output))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not build the zip"))?;
    let name = match name {
        Ok(name) => name,
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(err.into());
        }
    };
    record(&state, &user, "zip", &rel);
    send_temp_zip(tmp, &name).await
}

async fn download_selection(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PathsBody>,
) -> Result<Response, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    if body.paths.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose at least one item"));
    }
    let root = state.root.clone();
    let paths = body.paths;
    let label = if paths.len() == 1 {
        paths[0].clone()
    } else {
        format!("{} items", paths.len())
    };
    let tmp = std::env::temp_dir().join(format!(
        "ownnas-dl-{}-{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let output = tmp.clone();
    let name = tokio::task::spawn_blocking(move || files::write_zip_selection(&root, &paths, &output))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not build the download"))?;
    let name = match name {
        Ok(name) => name,
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(err.into());
        }
    };
    record(&state, &user, "download", &label);
    send_temp_zip(tmp, &name).await
}

async fn send_temp_zip(tmp: PathBuf, name: &str) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(&tmp).await.map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not open the zip")
    })?;
    let len = file.metadata().await.map(|meta| meta.len()).unwrap_or(0);
    let body = Body::from_stream(ReaderStream::new(DeleteOnDrop { inner: file, path: tmp }));
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_LENGTH, len)
        .header(header::CONTENT_DISPOSITION, files::content_disposition(name, true))
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(body)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not send the zip"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn scratch() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "ownnas-http-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("root");
        let data = base.join("data");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(root.join("hello.txt"), b"hello ownnas").unwrap();
        (root, data)
    }

    async fn app() -> (Router, PathBuf) {
        let (root, data) = scratch();
        let state = Arc::new(new_state(root.clone(), data, false, false, false).unwrap());
        {
            let conn = state.db.lock().unwrap();
            db::create_user(&conn, "ada", "longenough").unwrap();
        }
        (router(state), root)
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn login_list_and_block_escape() {
        let (app, root) = app().await;
        let login = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header("content-type", "application/json")
            .header("x-ownnas", "1")
            .body(Body::from(r#"{"username":"ada","password":"longenough"}"#))
            .unwrap();
        let response = app.clone().oneshot(login).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();

        let list = Request::builder()
            .uri("/api/list")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(list).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let text = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(text.contains("hello.txt"));

        let escaped = Request::builder()
            .uri("/api/raw?path=../ownnas.db")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(escaped).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let raw = Request::builder()
            .uri("/api/raw?path=hello.txt")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(raw).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, b"hello ownnas");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn parses_ranges() {
        match range_request(Some("bytes=0-10"), 100) {
            RangeReq::Partial(span) => {
                assert_eq!(span.start, 0);
                assert_eq!(span.len, 11);
            }
            _ => panic!("expected partial"),
        }
        match range_request(Some("bytes=90-"), 100) {
            RangeReq::Partial(span) => {
                assert_eq!(span.start, 90);
                assert_eq!(span.len, 10);
            }
            _ => panic!("expected partial"),
        }
        match range_request(Some("bytes=-10"), 100) {
            RangeReq::Partial(span) => {
                assert_eq!(span.start, 90);
                assert_eq!(span.len, 10);
            }
            _ => panic!("expected suffix"),
        }
        assert!(matches!(
            range_request(Some("bytes=100-"), 100),
            RangeReq::Unsatisfiable
        ));
    }
}
