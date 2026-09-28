use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
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
        .route("/api/list", get(list))
        .route("/api/meta", get(meta))
        .route("/api/raw", get(raw))
        .route("/api/thumb", get(thumb))
        .route("/api/mkdir", post(mkdir))
        .route("/api/rename", post(rename))
        .route("/api/upload", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/api/entry", axum::routing::delete(delete_entry))
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
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
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
}

#[derive(Deserialize)]
struct NameBody {
    path: Option<String>,
    name: String,
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
        db::create_session(&conn, user.id)
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?
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
        "readonly": state.readonly,
        "version": env!("CARGO_PKG_VERSION"),
        "rootName": files::root_label(&state.root),
        "ffmpeg": state.ffmpeg,
    })))
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
    let rel = rel_of(&query.path);
    let hidden = flag(&query.hidden);
    let ffmpeg = state.ffmpeg;
    let listing = tokio::task::spawn_blocking(move || files::list_dir(&root, &rel, hidden, ffmpeg))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not list the folder"))?
        .map_err(ApiError::from)?;
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
    require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let parent = rel_of(&body.path);
    let name = body.name;
    tokio::task::spawn_blocking(move || files::make_dir(&root, &parent, &name))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not create the folder"))?
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true })))
}

async fn rename(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RenameBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    tokio::task::spawn_blocking(move || files::rename_entry(&root, &body.path, &body.name))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not rename the item"))?
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    tokio::task::spawn_blocking(move || files::remove_entry(&root, &rel))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not delete the item"))?
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true })))
}

async fn upload(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    require_user(&state, &headers)?;
    require_write(&state)?;
    let dir_rel = rel_of(&query.path);
    let mut saved = 0u32;
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
        let dest = tokio::task::spawn_blocking(move || files::prepare_upload(&root, &dir, &filename))
            .await
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload"))?
            .map_err(ApiError::from)?;
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
        saved += 1;
    }
    if saved == 0 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose at least one file"));
    }
    Ok(Json(json!({ "saved": saved })))
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
