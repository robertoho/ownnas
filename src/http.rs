use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use std::future::Future;

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
use crate::pdf_ops;
use crate::preview;
use crate::thumbs;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const DOCX_EDITOR_CSS: &str = include_str!("../web/docx-editor.css");
const DOCX_EDITOR_JS: &str = include_str!("../web/docx-editor.js");
const ODT_EDITOR_JS: &str = include_str!("../web/odt-editor.js");
const XLSX_EDITOR_JS: &str = include_str!("../web/xlsx-editor.js");
const SQLITE_VIEWER_JS: &str = include_str!("../web/sqlite-viewer.js");
const THEME_JS: &str = include_str!("../web/theme.js");
const GALLERY_JS: &str = include_str!("../web/gallery.js");
const APP_JS: &str = include_str!("../web/app.js");
const MODEL_VIEWER_JS: &str = include_str!("../web/model-viewer.js");
const DXF_VIEWER_JS: &str = include_str!("../web/dxf-viewer.js");
const THREE_MODULE_JS: &str = include_str!("../web/vendor/three/three.module.min.js");
const THREE_ORBIT_CONTROLS_JS: &str = include_str!("../web/vendor/three/addons/controls/OrbitControls.js");
const THREE_OBJ_LOADER_JS: &str = include_str!("../web/vendor/three/addons/loaders/OBJLoader.js");
const THREE_STL_LOADER_JS: &str = include_str!("../web/vendor/three/addons/loaders/STLLoader.js");
const THREE_GLTF_LOADER_JS: &str = include_str!("../web/vendor/three/addons/loaders/GLTFLoader.js");
const THREE_PLY_LOADER_JS: &str = include_str!("../web/vendor/three/addons/loaders/PLYLoader.js");
const THREE_3MF_LOADER_JS: &str = include_str!("../web/vendor/three/addons/loaders/3MFLoader.js");
const THREE_FFLATE_JS: &str = include_str!("../web/vendor/three/addons/libs/fflate.module.js");
const THREE_BUFFER_GEOMETRY_UTILS_JS: &str =
    include_str!("../web/vendor/three/addons/utils/BufferGeometryUtils.js");
const OCCT_IMPORT_JS: &str = include_str!("../web/vendor/occt/occt-import-js.js");
const OCCT_WORKER_JS: &str = include_str!("../web/vendor/occt/ownnas-occt-worker.js");
const OCCT_WASM: &[u8] = include_bytes!("../web/vendor/occt/occt-import-js.wasm");
const TERMINAL_HTML: &str = include_str!("../web/terminal.html");
const TERMINAL_JS: &str = include_str!("../web/terminal.js");
const TERMINAL_PANEL_JS: &str = include_str!("../web/terminal-panel.js");
const TERMINAL_CSS: &str = include_str!("../web/terminal.css");
const XTERM_JS: &str = include_str!("../web/vendor/xterm/xterm.mjs");
const XTERM_FIT_JS: &str = include_str!("../web/vendor/xterm/addon-fit.mjs");
const XTERM_CSS: &str = include_str!("../web/vendor/xterm/xterm.css");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");
const ICON_192: &[u8] = include_bytes!("../web/icons/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../web/icons/icon-512.png");
const ICON_180: &[u8] = include_bytes!("../web/icons/icon-180.png");

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub root: PathBuf,
    pub thumbs: PathBuf,
    pub text_cache: PathBuf,
    /// Files pasted into terminals, one folder per terminal.
    pub terminal_uploads: PathBuf,
    pub readonly: bool,
    pub secure_cookie: bool,
    pub ffmpeg: bool,
    pub ocr: bool,
    pub update: crate::update::UpdateConfig,
    pub update_status: Mutex<Option<serde_json::Value>>,
    pub restart_notify: tokio::sync::Notify,
    pub restart_requested: std::sync::atomic::AtomicBool,
    pub supervisor_restart: bool,
    pub attempts: Mutex<AttemptGate>,
    /// Served over native HTTPS (`--tls-cert`).
    pub https: bool,
    pub terminals: crate::terminal::Terminals,
    /// Loopback URL that agent hooks inside terminals report to.
    pub agent_url: std::sync::OnceLock<String>,
    /// The Gallery's photo index, kept up to date in the background once started.
    pub media: std::sync::Arc<crate::media_index::Indexer>,
    /// How full the disk is, and how much the library and OwnNAS's data take.
    pub storage: std::sync::Arc<crate::storage::Storage>,
    /// The Gallery's machine-learning models (downloaded when switched on).
    #[cfg(ownnas_ml)]
    pub models: std::sync::Arc<crate::ml::Models>,
    pub db_path: PathBuf,
}

impl AppState {
    /// Starts indexing the library's photos and videos for the Gallery timeline.
    pub fn start_media_index(&self) {
        let speed = setting(self, crate::media_index::SETTING_SPEED)
            .and_then(|value| crate::media_index::Speed::parse(&value))
            .unwrap_or(crate::media_index::Speed::Medium);
        self.media.set_speed(speed);
        #[cfg(ownnas_ml)]
        let _ = self.media.models.set(self.models.clone());
        self.media.start(self.db_path.clone(), self.root.clone(), self.thumbs.clone(), self.ffmpeg);
    }
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
    ocr: bool,
    update: crate::update::UpdateConfig,
) -> Result<AppState, String> {
    let db_path = data_dir.join("ownnas.db");
    let conn = db::open(&db_path)?;
    let thumbs = crate::thumbs::prepare_cache(&data_dir.join("thumbs"))?;
    let text_cache = crate::content_index::ensure_cache_dir(&data_dir)?;
    // Pasted files belong to terminals, which do not survive a restart.
    let terminal_uploads = data_dir.join("terminal-uploads");
    let _ = std::fs::remove_dir_all(&terminal_uploads);
    Ok(AppState {
        db: Mutex::new(conn),
        root,
        thumbs,
        text_cache,
        terminal_uploads,
        readonly,
        secure_cookie,
        ffmpeg,
        ocr,
        update,
        update_status: Mutex::new(None),
        restart_notify: tokio::sync::Notify::new(),
        restart_requested: std::sync::atomic::AtomicBool::new(false),
        supervisor_restart: false,
        attempts: Mutex::new(AttemptGate::new()),
        https: false,
        terminals: crate::terminal::Terminals::default(),
        agent_url: std::sync::OnceLock::new(),
        media: std::sync::Arc::default(),
        storage: {
            let storage = std::sync::Arc::new(crate::storage::Storage::default());
            storage.load(&db_path);
            storage
        },
        #[cfg(ownnas_ml)]
        models: std::sync::Arc::new(crate::ml::Models::new(&data_dir)),
        db_path,
    })
}

pub fn router(state: std::sync::Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/app.css", get(css))
        .route("/assets/app.js", get(javascript))
        .route("/assets/docx-editor.js", get(docx_editor_javascript))
        .route("/assets/odt-editor.js", get(odt_editor_javascript))
        .route("/assets/xlsx-editor.js", get(xlsx_editor_javascript))
        .route("/assets/sqlite-viewer.js", get(sqlite_viewer_javascript))
        .route("/assets/theme.js", get(theme_javascript))
        .route("/assets/gallery.js", get(gallery_javascript))
        .route("/assets/docx-editor.css", get(docx_editor_css))
        .route("/assets/model-viewer.js", get(model_viewer_javascript))
        .route("/assets/dxf-viewer.js", get(dxf_viewer_javascript))
        .route("/assets/vendor/three/three.module.min.js", get(three_module))
        .route(
            "/assets/vendor/three/addons/controls/OrbitControls.js",
            get(three_orbit_controls),
        )
        .route(
            "/assets/vendor/three/addons/loaders/OBJLoader.js",
            get(three_obj_loader),
        )
        .route(
            "/assets/vendor/three/addons/loaders/STLLoader.js",
            get(three_stl_loader),
        )
        .route(
            "/assets/vendor/three/addons/loaders/GLTFLoader.js",
            get(three_gltf_loader),
        )
        .route(
            "/assets/vendor/three/addons/loaders/PLYLoader.js",
            get(three_ply_loader),
        )
        .route(
            "/assets/vendor/three/addons/loaders/3MFLoader.js",
            get(three_3mf_loader),
        )
        .route(
            "/assets/vendor/three/addons/libs/fflate.module.js",
            get(three_fflate),
        )
        .route(
            "/assets/vendor/three/addons/utils/BufferGeometryUtils.js",
            get(three_buffer_geometry_utils),
        )
        .route(
            "/assets/vendor/occt/occt-import-js.js",
            get(occt_import_javascript),
        )
        .route(
            "/assets/vendor/occt/ownnas-occt-worker.js",
            get(occt_worker_javascript),
        )
        .route("/assets/vendor/occt/occt-import-js.wasm", get(occt_wasm))
        .route("/api/health", get(health))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/update/check", get(update_check))
        .route("/api/update/status", get(update_status))
        .route("/api/update/apply", post(update_apply))
        .route("/api/password", post(change_password))
        .route("/api/users", get(list_users).post(create_managed_user))
        .route("/api/users/password", post(admin_set_password))
        .route("/api/users/admin", post(set_user_admin))
        .route("/api/users/remove", post(remove_managed_user))
        .route("/api/list", get(list))
        .route("/api/meta", get(meta))
        .route("/api/raw", get(raw))
        .route("/api/docx", get(read_docx))
        .route("/api/odt", get(read_odt))
        .route("/api/xlsx", get(read_xlsx))
        .route("/api/sqlite", get(sqlite_overview))
        .route("/api/sqlite/rows", get(sqlite_rows))
        .route("/api/sqlite/query", post(sqlite_query))
        .route("/api/sqlite/value", get(sqlite_value))
        .route("/api/thumb", get(thumb))
        .route("/api/mkdir", post(mkdir))
        .route("/api/create", post(create_entry))
        .route("/api/write", post(write_entry))
        .route("/api/write-docx", post(write_docx_entry).layer(DefaultBodyLimit::max(36 * 1024 * 1024)))
        .route("/api/write-odt", post(write_odt_entry).layer(DefaultBodyLimit::max(36 * 1024 * 1024)))
        .route("/api/write-xlsx", post(write_xlsx_entry).layer(DefaultBodyLimit::max(36 * 1024 * 1024)))
        .route("/api/write-image", post(write_image_entry))
        .route("/api/convert-image", post(convert_image_entry))
        .route("/api/pdf/info", get(pdf_info))
        .route("/api/pdf/extract", post(pdf_extract))
        .route("/api/pdf/split", post(pdf_split))
        .route("/api/pdf/merge", post(pdf_merge))
        .route("/api/pdf/rotate", post(pdf_rotate))
        .route("/api/rename", post(rename))
        .route("/api/upload", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/api/upload/conflicts", post(upload_conflicts))
        .route("/api/upload/directories", post(upload_directories))
        .route("/api/entry", axum::routing::delete(delete_entry))
        .route("/api/restore", post(restore_item))
        .route("/api/trash/empty", post(empty_trash))
        .route("/api/bookmarks", get(list_bookmarks).post(add_bookmark).delete(remove_bookmark))
        .route("/api/recent", get(list_recent).post(touch_recent))
        .route("/api/search", get(search))
        .route("/api/usage", get(folder_usage))
        .route("/api/hash", get(file_hash))
        .route("/api/duplicates", get(find_duplicates))
        .route("/api/activity", get(activity))
        .route("/api/zip", get(zip_folder))
        .route("/api/download", post(download_selection))
        .route("/api/compress", post(compress_entries))
        .route("/api/move", post(move_item))
        .route("/api/undo-move", post(undo_move_item))
        .route("/api/copy", post(copy_item))
        .route("/api/duplicate", post(duplicate_item))
        .route("/api/folder-appearance", get(get_folder_appearance).post(set_folder_appearance))
        .route("/api/annotations", get(get_annotations).post(ack_annotations))
        .route("/api/tags", post(add_tag).delete(remove_tag))
        .route("/api/tags/all", get(all_tags))
        .route("/api/gallery/status", get(gallery_status))
        .route("/api/gallery/details", get(photo_details))
        .route("/api/gallery/rescan", post(gallery_rescan))
        .route("/api/gallery/timeline", get(gallery_timeline))
        .route("/api/gallery/days", get(gallery_days))
        .route("/api/ml", get(ml_status).post(set_ml_settings))
        .route("/api/backup", get(backup_config).post(set_backup_config))
        .route("/api/backup/upload", post(backup_upload).layer(DefaultBodyLimit::disable()))
        .route("/api/backup/has", get(backup_has))
        .route("/api/images/resize", post(resize_images))
        .route("/api/storage", get(storage_status))
        .route("/api/storage/measure", post(measure_storage))
        .route("/api/videos/convert", post(convert_video))
        .route("/api/videos/job", get(video_job).delete(cancel_video_job))
        .route("/api/backup/blocked", get(blocked_backups).post(block_backups).delete(unblock_backup))
        .route("/api/backup/blocked/preview", get(blocked_preview))
        .route("/api/ml/forget-faces", post(forget_faces))
        .route("/api/gallery/search", get(gallery_search))
        .route("/api/gallery/similar", get(gallery_similar).post(gallery_similar).layer(DefaultBodyLimit::max(60 * 1024 * 1024)))
        .route("/api/people", get(list_people).post(change_person))
        .route("/api/people/photos", get(person_photos))
        .route("/api/faces/move", post(move_faces))
        .route("/api/faces/crop", get(face_crop))
        .route("/api/faces/in", get(faces_in_photo))
        .route("/api/gallery/settings", get(gallery_settings).post(set_gallery_settings))
        .route("/api/gallery/hidden", get(gallery_hidden).post(set_gallery_hidden).layer(DefaultBodyLimit::max(8 * 1024 * 1024)))
        .route("/api/gallery/tag", get(gallery_tag))
        .route("/api/albums", get(get_album).post(create_album).patch(update_album).delete(delete_album))
        .route("/api/albums/tree", get(album_tree))
        .route("/api/albums/items", post(album_items).layer(DefaultBodyLimit::max(8 * 1024 * 1024)))
        .route("/api/tags/bulk", post(bulk_tags))
        .route("/api/comments", post(add_comment).delete(remove_comment))
        .route("/manifest.webmanifest", get(manifest))
        .route("/assets/icons/icon-192.png", get(|| async { png(ICON_192) }))
        .route("/assets/icons/icon-512.png", get(|| async { png(ICON_512) }))
        .route("/assets/icons/icon-180.png", get(|| async { png(ICON_180) }))
        .route("/terminal", get(terminal_page))
        .route("/assets/terminal.js", get(terminal_javascript))
        .route("/assets/terminal-panel.js", get(terminal_panel_javascript))
        .route("/assets/terminal.css", get(terminal_css))
        .route("/assets/vendor/xterm/xterm.mjs", get(xterm_module))
        .route("/assets/vendor/xterm/addon-fit.mjs", get(xterm_fit_module))
        .route("/assets/vendor/xterm/xterm.css", get(xterm_css))
        .route("/api/terminal/config", get(terminal_config).post(set_terminal_config))
        .route("/api/terminal/unlock", post(terminal_unlock))
        .route("/api/terminal/lock", post(terminal_lock))
        .route("/api/terminals", get(list_terminals).post(open_terminal).delete(close_terminal))
        .route("/api/terminals/ws", get(terminal_socket))
        .route("/api/terminals/seen", post(terminal_seen))
        .route("/api/terminals/rename", post(rename_terminal))
        .route("/api/terminals/cwd", get(terminal_cwd))
        .route("/api/terminals/paths", post(terminal_paths))
        .route(
            "/api/terminals/upload",
            post(terminal_upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/terminals/changes", get(terminal_changes))
        .route("/api/terminals/diff", get(terminal_diff))
        .route("/api/agents/stream", get(agent_stream))
        .route("/api/agents/integrations", get(agent_integrations).post(set_agent_integration))
        .route("/api/sync/tree", get(sync_tree))
        .route(
            "/api/sync/file",
            get(sync_download)
                .put(sync_upload)
                .layer(DefaultBodyLimit::disable()),
        )
        .route("/api/sync/mkdir", post(sync_mkdir))
        .route("/api/sync/delete", post(sync_delete))
        .with_state(state)
}

pub async fn serve(state: std::sync::Arc<AppState>, addr: &str, open_browser: bool) -> Result<(), String> {
    let shutdown_state = state.clone();
    serve_with_shutdown(state, addr, open_browser, None, async move { shutdown_state.restart_notify.notified().await }).await
}

/// Serve HTTPS directly using a supplied PEM certificate chain and private key.
pub async fn serve_https(
    state: std::sync::Arc<AppState>,
    addr: &str,
    open_browser: bool,
    cert: &std::path::Path,
    key: &std::path::Path,
) -> Result<(), String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .map_err(|err| format!("Could not load HTTPS certificate/key: {err}"))?;
    let listener = std::net::TcpListener::bind(addr)
        .map_err(|err| format!("Could not listen on {addr}: {err}"))?;
    listener.set_nonblocking(true).map_err(|err| err.to_string())?;
    let bound = listener.local_addr().map_err(|err| err.to_string())?;
    print_banner(&state, bound, "https");
    if open_browser {
        open_url(&format!("https://127.0.0.1:{}", bound.port()));
    }
    start_agent_services(state.clone()).await;
    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    let shutdown_state = state.clone();
    let shutdown = tokio::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = shutdown_state.restart_notify.notified() => {},
        }
        shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
    });
    let result = axum_server::from_tcp_rustls(listener, config)
        .map_err(|err| err.to_string())?
        .handle(handle)
        .serve(router(state).into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(|err| format!("HTTPS server stopped: {err}"));
    shutdown.abort();
    result
}

/// Start the HTTP server and optionally report the bound address before serving.
///
/// The Android bridge uses this variant so Java can wait until the loopback
/// listener is ready before loading the WebView. Desktop callers should use
/// `serve`, which keeps the original Ctrl+C-driven lifecycle.
pub async fn serve_with_shutdown<F>(
    state: std::sync::Arc<AppState>,
    addr: &str,
    open_browser: bool,
    ready: Option<std::sync::mpsc::Sender<Result<SocketAddr, String>>>,
    shutdown: F,
) -> Result<(), String>
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|_| {
            let error = format!("Could not listen on {addr}. The port may already be in use.");
            if let Some(sender) = &ready {
                let _ = sender.send(Err(error.clone()));
            }
            error
        })?;
    let bound = listener
        .local_addr()
        .map_err(|_| {
            let error = "Could not read the listen address".to_string();
            if let Some(sender) = &ready {
                let _ = sender.send(Err(error.clone()));
            }
            error
        })?;
    if let Some(sender) = ready {
        let _ = sender.send(Ok(bound));
    }
    print_banner(&state, bound, "http");
    start_agent_services(state.clone()).await;
    if open_browser {
        let url = format!("http://127.0.0.1:{}", bound.port());
        open_url(&url);
    }
    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|_| "The server stopped because of a network error".to_string())
}

fn print_banner(state: &AppState, bound: SocketAddr, scheme: &str) {
    let local = match bound.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => format!("{scheme}://127.0.0.1:{}", bound.port()),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => format!("{scheme}://[::1]:{}", bound.port()),
        _ => format!("{scheme}://{bound}"),
    };
    println!("OwnNAS is listening on {scheme}://{bound}");
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
    if state.ocr {
        println!("Image OCR search: tesseract");
    } else {
        println!("Image OCR search: install tesseract and restart OwnNAS to enable it");
    }
    if crate::content_index::pdftotext_available() {
        println!("PDF text search: pdftotext");
    } else {
        println!("PDF text search: built-in extractor (install poppler/pdftotext for better PDF search)");
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
            FileError::Cancelled => ApiError::new(StatusCode::BAD_REQUEST, "Cancelled"),
            FileError::Rejected(message) => ApiError::new(StatusCode::BAD_REQUEST, message),
            FileError::Io(message) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message),
        }
    }
}

/// Sets a cancel flag when the HTTP handler is dropped (client abort / disconnect).
struct CancelOnDrop(files::CancelFlag);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
    /// Native clients ask for the session token in the response body and send it back as
    /// `Authorization: Bearer <token>` instead of a cookie.
    #[serde(default)]
    token: bool,
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
    /// The upload's modification time in seconds, compared by "archive older".
    modified: Option<i64>,
    /// The upload's modification time in milliseconds since the Unix epoch, given to the saved file.
    mtime: Option<i64>,
}

#[derive(Deserialize)]
struct ConflictBody {
    path: Option<String>,
    names: Vec<String>,
}

#[derive(Deserialize)]
struct UploadDirectoriesBody {
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
struct WriteImageBody {
    path: String,
    /// Base64-encoded image bytes (optionally data-URL prefixed).
    data: String,
}

#[derive(Deserialize)]
struct ConvertImageBody {
    path: String,
    /// Target format: jpeg, png, webp, gif, bmp, tiff
    format: String,
}

#[derive(Deserialize)]
struct PdfExtractBody {
    path: String,
    /// Page list like "1,3,5-8"
    pages: String,
    /// "combined" (one PDF) or "separate" (one file per page)
    mode: String,
}

#[derive(Deserialize)]
struct PdfPathBody {
    path: String,
}

#[derive(Deserialize)]
struct PdfMergeBody {
    paths: Vec<String>,
}

#[derive(Deserialize)]
struct PdfRotateBody {
    path: String,
    /// Empty = all pages. Otherwise "1,3,5-8".
    pages: Option<String>,
    /// 90, 180, or 270 (clockwise).
    degrees: i64,
}

#[derive(Deserialize)]
struct RenameBody {
    path: String,
    name: String,
}

#[derive(Deserialize)]
struct BulkTagsBody {
    paths: Vec<String>,
    tag: String,
    /// "add" (default) or "remove"
    action: Option<String>,
}

fn flag(value: &Option<String>) -> bool {
    matches!(value.as_deref(), Some("1") | Some("true") | Some("yes"))
}

fn rel_of(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

fn check_csrf(headers: &HeaderMap) -> Result<(), ApiError> {
    // Browsers never attach a bearer token on their own, so it cannot be forged cross-site.
    if bearer_token(headers).is_some() {
        return Ok(());
    }
    match headers.get("x-ownnas").and_then(|v| v.to_str().ok()) {
        Some("1") => Ok(()),
        _ => Err(ApiError::new(StatusCode::FORBIDDEN, "Missing request header")),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = raw.strip_prefix("Bearer ")?.trim();
    (!token.is_empty()).then(|| token.to_string())
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = bearer_token(headers) {
        return Some(token);
    }
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
                "default-src 'self'; img-src 'self'; media-src 'self'; frame-src 'self'; style-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; base-uri 'none'; form-action 'self'",
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

fn module_javascript(body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body,
    )
}

fn app_module_javascript(body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
}

async fn docx_editor_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], DOCX_EDITOR_CSS)
}

async fn docx_editor_javascript() -> impl IntoResponse {
    app_module_javascript(DOCX_EDITOR_JS)
}

async fn odt_editor_javascript() -> impl IntoResponse {
    app_module_javascript(ODT_EDITOR_JS)
}

async fn xlsx_editor_javascript() -> impl IntoResponse {
    app_module_javascript(XLSX_EDITOR_JS)
}

async fn sqlite_viewer_javascript() -> impl IntoResponse {
    app_module_javascript(SQLITE_VIEWER_JS)
}

async fn theme_javascript() -> impl IntoResponse {
    app_module_javascript(THEME_JS)
}

async fn gallery_javascript() -> impl IntoResponse {
    app_module_javascript(GALLERY_JS)
}

async fn model_viewer_javascript() -> impl IntoResponse {
    app_module_javascript(MODEL_VIEWER_JS)
}
async fn dxf_viewer_javascript() -> impl IntoResponse {
    app_module_javascript(DXF_VIEWER_JS)
}
async fn three_module() -> impl IntoResponse {
    module_javascript(THREE_MODULE_JS)
}
async fn three_orbit_controls() -> impl IntoResponse {
    module_javascript(THREE_ORBIT_CONTROLS_JS)
}
async fn three_obj_loader() -> impl IntoResponse {
    module_javascript(THREE_OBJ_LOADER_JS)
}
async fn three_stl_loader() -> impl IntoResponse {
    module_javascript(THREE_STL_LOADER_JS)
}
async fn three_gltf_loader() -> impl IntoResponse {
    module_javascript(THREE_GLTF_LOADER_JS)
}
async fn three_ply_loader() -> impl IntoResponse {
    module_javascript(THREE_PLY_LOADER_JS)
}
async fn three_3mf_loader() -> impl IntoResponse {
    module_javascript(THREE_3MF_LOADER_JS)
}
async fn three_fflate() -> impl IntoResponse {
    module_javascript(THREE_FFLATE_JS)
}
async fn three_buffer_geometry_utils() -> impl IntoResponse {
    module_javascript(THREE_BUFFER_GEOMETRY_UTILS_JS)
}
async fn occt_import_javascript() -> impl IntoResponse {
    module_javascript(OCCT_IMPORT_JS)
}
async fn occt_worker_javascript() -> impl IntoResponse {
    app_module_javascript(OCCT_WORKER_JS)
}
async fn occt_wasm() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/wasm"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        OCCT_WASM,
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
        Json(if body.token {
            json!({ "username": user.username, "token": token })
        } else {
            json!({ "username": user.username })
        }),
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
        state.terminals.lock(Some(&auth::token_hash(&token)));
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
    use sha2::Digest;
    let library_id = hex::encode(sha2::Sha256::digest(state.root.to_string_lossy().as_bytes()));
    Ok(Json(json!({
        "libraryId": library_id,
        "username": user.username,
        "admin": user.is_admin,
        "readonly": state.readonly,
        "version": env!("CARGO_PKG_VERSION"),
        "rootName": files::root_label(&state.root),
        "ffmpeg": state.ffmpeg,
        "ocr": state.ocr,
        "updatesConfigured": state.update.url.is_some(),
        "updatesSigned": state.update.pubkey.is_some(),
        "updateTarget": crate::update::current_target(),
        "terminal": user.is_admin && terminal_enabled(&state),
    })))
}

async fn update_check(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    if state.update.url.is_none() {
        return Ok(Json(json!({
            "current": env!("CARGO_PKG_VERSION"),
            "latest": env!("CARGO_PKG_VERSION"),
            "notes": "",
            "available": false,
            "target": crate::update::current_target(),
            "artifactUrl": null,
            "signed": false,
            "updateUrl": null,
            "configured": false,
        })));
    }
    let config = state.update.clone();
    let result = tokio::task::spawn_blocking(move || crate::update::check(&config))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Update check failed"))?
        .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, err))?;
    let value = json!({
        "current": result.current,
        "latest": result.latest,
        "notes": result.notes,
        "available": result.available,
        "target": result.target,
        "artifactUrl": result.artifact_url,
        "signed": result.signed,
        "updateUrl": result.update_url,
        "configured": true,
        "checkedAt": format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()),
    });
    *state.update_status.lock().unwrap_or_else(|e| e.into_inner()) = Some(value.clone());
    Ok(Json(value))
}

async fn update_status(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    let result = state.update_status.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(Json(result.unwrap_or_else(|| json!({"configured": state.update.url.is_some(), "available": false, "checkedAt": null}))))
}

async fn update_apply(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    if state.update.url.is_none() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Updates are not configured. Pass --update-url when starting OwnNAS.",
        ));
    }
    let config = state.update.clone();
    let version = tokio::task::spawn_blocking(move || crate::update::download_and_replace(&config))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Update failed"))?
        .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, err))?;
    record(&state, &user, "update", &format!("installed {version}"));
    state.restart_requested.store(true, std::sync::atomic::Ordering::SeqCst);
    state.restart_notify.notify_one();
    Ok(Json(json!({
        "ok": true,
        "version": version,
        "restarting": true,
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
    let mut payload = serde_json::to_value(listing).map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not list folder"))?;
    decorate_folder_appearance(&state, &mut payload["entries"])?;
    Ok(Json(payload))
}

async fn meta(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let ffmpeg = state.ffmpeg;
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
        let resizable = crate::image_edit::resizable(&name);
        let shown = if files::kind_of(&name) == "image" { crate::image_edit::shown_size(&resolved.full) } else { None };
        let video = if ffmpeg && files::kind_of(&name) == "video" { crate::video_edit::probe(&resolved.full) } else { None };
        Ok(json!({
            "duration": video.map(|(seconds, _)| seconds),
            "hasAudio": video.map(|(_, audio)| audio),
            "width": shown.map(|(w, _)| w),
            "height": shown.map(|(_, h)| h),
            "resizable": resizable,
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

async fn read_docx(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Response, ApiError> {
    use sha2::Digest;
    require_user(&state, &headers)?;
    let target = files::resolve(&state.root, &rel_of(&query.path))?;
    if !target.full.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("docx")) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Expected a DOCX file"));
    }
    let mut file = tokio::fs::File::open(target.full).await.map_err(|_| ApiError::from(FileError::NotFound))?;
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    (&mut file).take(25 * 1024 * 1024 + 1).read_to_end(&mut bytes).await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read document"))?;
    if bytes.len() > 25 * 1024 * 1024 { return Err(ApiError::new(StatusCode::BAD_REQUEST, "Documents up to 25 MB are supported")); }
    let hash = hex::encode(sha2::Sha256::digest(&bytes));
    Ok(([(header::CONTENT_TYPE.as_str(), "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        ("x-docx-hash", hash.as_str()), ("cache-control", "no-store")], bytes).into_response())
}

async fn read_odt(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Response, ApiError> {
    use sha2::Digest;
    require_user(&state, &headers)?;
    let target = files::resolve(&state.root, &rel_of(&query.path))?;
    if !target.full.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("odt")) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Expected an ODT file"));
    }
    let mut file = tokio::fs::File::open(target.full).await.map_err(|_| ApiError::from(FileError::NotFound))?;
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    (&mut file).take(25 * 1024 * 1024 + 1).read_to_end(&mut bytes).await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read document"))?;
    if bytes.len() > 25 * 1024 * 1024 { return Err(ApiError::new(StatusCode::BAD_REQUEST, "Documents up to 25 MB are supported")); }
    let hash = hex::encode(sha2::Sha256::digest(&bytes));
    Ok(([(header::CONTENT_TYPE.as_str(), "application/vnd.oasis.opendocument.text"),
        ("x-odt-hash", hash.as_str()), ("cache-control", "no-store")], bytes).into_response())
}

async fn read_xlsx(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Response, ApiError> {
    use sha2::Digest;
    require_user(&state, &headers)?;
    let target = files::resolve(&state.root, &rel_of(&query.path))?;
    if !target.full.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("xlsx")) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Expected an XLSX file"));
    }
    let mut file = tokio::fs::File::open(target.full).await.map_err(|_| ApiError::from(FileError::NotFound))?;
    let mut bytes = Vec::new();
    (&mut file).take(25 * 1024 * 1024 + 1).read_to_end(&mut bytes).await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read spreadsheet"))?;
    if bytes.len() > 25 * 1024 * 1024 { return Err(ApiError::new(StatusCode::BAD_REQUEST, "Spreadsheets up to 25 MB are supported")); }
    let hash = hex::encode(sha2::Sha256::digest(&bytes));
    Ok(([(header::CONTENT_TYPE.as_str(), "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        ("x-xlsx-hash", hash.as_str()), ("cache-control", "no-store")], bytes).into_response())
}

impl From<crate::sqlite_view::ViewError> for ApiError {
    fn from(err: crate::sqlite_view::ViewError) -> Self {
        use crate::sqlite_view::ViewError;
        let status = match err {
            ViewError::NotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_REQUEST,
        };
        ApiError::new(status, err.to_string())
    }
}

/// Opens the SQLite database at a library path (read-only) and runs `work` on it.
async fn with_sqlite<T: Send + 'static>(
    state: &AppState,
    path: &Option<String>,
    work: impl FnOnce(&rusqlite::Connection) -> Result<T, crate::sqlite_view::ViewError> + Send + 'static,
) -> Result<T, ApiError> {
    let target = files::resolve(&state.root, &rel_of(path))?;
    if !target.full.is_file() {
        return Err(FileError::NotFound.into());
    }
    tokio::task::spawn_blocking(move || work(&crate::sqlite_view::open(&target.full)?))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the database"))?
        .map_err(ApiError::from)
}

async fn sqlite_overview(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    Ok(Json(with_sqlite(&state, &query.path, crate::sqlite_view::overview).await?))
}

#[derive(Deserialize)]
struct SqliteRowsQuery {
    path: Option<String>,
    table: String,
    offset: Option<usize>,
    limit: Option<usize>,
    order: Option<String>,
    desc: Option<String>,
    filter: Option<String>,
}

async fn sqlite_rows(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SqliteRowsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let path = query.path.clone();
    let result = with_sqlite(&state, &path, move |conn| {
        crate::sqlite_view::rows(conn, &crate::sqlite_view::Page {
            table: &query.table,
            offset: query.offset.unwrap_or(0),
            limit: query.limit.unwrap_or(100),
            order: query.order.as_deref().filter(|order| !order.is_empty()),
            descending: query.desc.as_deref() == Some("1"),
            filter: query.filter.as_deref().unwrap_or(""),
        })
    })
    .await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct SqliteQueryBody {
    path: Option<String>,
    sql: String,
}

async fn sqlite_query(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<SqliteQueryBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    require_user(&state, &headers)?;
    if body.sql.len() > 100_000 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "That query is too long"));
    }
    let sql = body.sql;
    Ok(Json(with_sqlite(&state, &body.path, move |conn| crate::sqlite_view::query(conn, &sql)).await?))
}

#[derive(Deserialize)]
struct SqliteValueQuery {
    path: Option<String>,
    table: String,
    rowid: i64,
    column: String,
}

/// One whole value from a table: text to read, or a blob to download.
async fn sqlite_value(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SqliteValueQuery>,
) -> Result<Response, ApiError> {
    require_user(&state, &headers)?;
    let name = format!("{}-{}-{}", query.table, query.rowid, query.column);
    let (bytes, text) = with_sqlite(&state, &query.path.clone(), move |conn| {
        crate::sqlite_view::value(conn, &query.table, query.rowid, &query.column)
    })
    .await?;
    let response = Response::builder()
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff");
    let response = if text {
        response.header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
    } else {
        response
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_DISPOSITION, files::content_disposition(&format!("{name}.bin"), true))
    };
    response
        .body(Body::from(bytes))
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not send the value"))
}

impl From<crate::albums::AlbumError> for ApiError {
    fn from(err: crate::albums::AlbumError) -> Self {
        use crate::albums::AlbumError;
        let status = match err {
            AlbumError::NotFound => StatusCode::NOT_FOUND,
            AlbumError::Invalid(_) => StatusCode::BAD_REQUEST,
            AlbumError::Conflict(_) => StatusCode::CONFLICT,
            AlbumError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError::new(status, err.to_string())
    }
}

/// Media items as the Gallery shows them: with their kind, and whether the file is still there
/// (it may have been moved or deleted outside OwnNAS).
fn gallery_items(state: &AppState, items: Vec<crate::albums::MediaItem>, check_missing: bool) -> Vec<serde_json::Value> {
    items
        .into_iter()
        .map(|mut item| {
            if item.kind.is_none() {
                let name = item.path.rsplit('/').next().unwrap_or(&item.path);
                item.kind = Some(files::kind_of(name).to_string());
            }
            let mut value = serde_json::to_value(&item).unwrap_or_default();
            if check_missing && !state.root.join(&item.path).exists() {
                value["missing"] = json!(true);
            }
            value
        })
        .collect()
}

async fn gallery_days(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let (days, hidden) = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        (crate::albums::days(&conn)?, crate::albums::hidden_count(&conn)?)
    };
    Ok(Json(json!({
        "days": days.into_iter().map(|(day, count)| json!({ "day": day, "count": count })).collect::<Vec<_>>(),
        "hidden": hidden,
        "status": state.media.status(),
    })))
}

async fn gallery_settings(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    Ok(Json(json!({ "speed": state.media.speed(), "status": state.media.status() })))
}

#[derive(Deserialize)]
struct GallerySettingsBody {
    speed: String,
}

/// How hard background indexing works the machine (administrators only).
async fn set_gallery_settings(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<GallerySettingsBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    let speed = crate::media_index::Speed::parse(&body.speed)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "Choose slow, medium, or fast"))?;
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::set_setting(&conn, crate::media_index::SETTING_SPEED, Some(speed.as_str()))
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    }
    state.media.set_speed(speed);
    record(&state, &user, "index-speed", speed.as_str());
    Ok(Json(json!({ "speed": speed, "status": state.media.status() })))
}

// ---- Smart search and people (on-device machine learning) ----

#[cfg(ownnas_ml)]
fn ml_feature_on(state: &AppState, feature: crate::ml::Feature) -> bool {
    setting(state, feature.setting()).as_deref() == Some("1")
}

/// What the smart features are doing, for Settings and the Gallery.
async fn ml_status(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    #[cfg(not(ownnas_ml))]
    {
        return Ok(Json(json!({ "available": false })));
    }
    #[cfg(ownnas_ml)]
    {
        use crate::ml::Feature;
        let progress = {
            let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
            crate::smart::progress_with_photos(&conn).unwrap_or_default()
        };
        let feature = |feature: Feature| {
            json!({
                "enabled": ml_feature_on(&state, feature),
                "ready": state.models.ready(feature),
                "download": state.models.download_state(feature),
                "size": feature.download_size(),
            })
        };
        Ok(Json(json!({
            "available": true,
            "search": feature(Feature::Search),
            "faces": feature(Feature::Faces),
            "photos": progress.0,
            "described": progress.1,
            "scanned": progress.2,
            "images": progress.3,
            "status": state.media.status(),
        })))
    }
}

#[derive(Deserialize)]
struct MlSettingsBody {
    search: Option<bool>,
    faces: Option<bool>,
}

/// Turns smart search or faces on (downloading their models first) or off (administrators).
async fn set_ml_settings(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<MlSettingsBody>,
) -> Result<Response, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    #[cfg(not(ownnas_ml))]
    {
        let _ = body;
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Smart search is not available on this device"));
    }
    #[cfg(ownnas_ml)]
    {
        use crate::ml::Feature;
        for (feature, on) in [(Feature::Search, body.search), (Feature::Faces, body.faces)] {
            let Some(on) = on else { continue };
            {
                let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
                db::set_setting(&conn, feature.setting(), Some(if on { "1" } else { "0" }))
                    .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
            }
            record(&state, &user, if on { "ml-on" } else { "ml-off" }, &format!("{feature:?}"));
            if on {
                let media = state.media.clone();
                if state.models.ready(feature) {
                    media.rescan();
                } else {
                    state.models.download(feature, move || media.rescan());
                }
            }
        }
        Ok(ml_status(State(state), headers).await?.into_response())
    }
}

/// Deletes every face and person, and the face models (administrators). Photos are untouched.
async fn forget_faces(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        crate::smart::forget_faces(&conn).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
        #[cfg(ownnas_ml)]
        db::set_setting(&conn, crate::ml::Feature::Faces.setting(), Some("0")).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    }
    #[cfg(ownnas_ml)]
    state.models.remove(crate::ml::Feature::Faces);
    let _ = std::fs::remove_dir_all(state.thumbs.join("faces"));
    record(&state, &user, "faces-forget", "all faces and people");
    Ok(Json(json!({ "ok": true })))
}

#[cfg(ownnas_ml)]
fn search_ready(state: &AppState) -> Result<(), ApiError> {
    if !ml_feature_on(state, crate::ml::Feature::Search) || !state.models.ready(crate::ml::Feature::Search) {
        return Err(ApiError::new(StatusCode::CONFLICT, "Smart search is off. An administrator can turn it on in Settings → Gallery."));
    }
    Ok(())
}

/// Photos for a list of (path, score), best first.
fn scored_items(state: &AppState, found: Vec<(String, f32)>) -> Vec<serde_json::Value> {
    let paths: Vec<String> = found.iter().map(|(path, _)| path.clone()).collect();
    let items = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        crate::albums::media_items(&conn, &paths).unwrap_or_default()
    };
    gallery_items(state, items, false)
        .into_iter()
        .zip(found)
        .map(|(mut item, (_, score))| {
            item["score"] = json!(score);
            item
        })
        .collect()
}

#[derive(Deserialize)]
struct SmartSearchQuery {
    q: String,
}

/// Photos that show what the words describe ("dog on a beach").
async fn gallery_search(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SmartSearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    #[cfg(not(ownnas_ml))]
    {
        let _ = query;
        return Err::<Json<serde_json::Value>, _>(ApiError::new(StatusCode::BAD_REQUEST, "Smart search is not available on this device"));
    }
    #[cfg(ownnas_ml)]
    {
        search_ready(&state)?;
        let text = query.q.trim().chars().take(300).collect::<String>();
        if text.is_empty() {
            return Ok(Json(json!({ "items": [] })));
        }
        let worker = state.clone();
        let found = tokio::task::spawn_blocking(move || -> Result<Vec<(String, f32)>, String> {
            let vector = worker.models.describe_text(&text)?;
            let neutral = worker.models.neutral_text()?;
            let descriptions = {
                let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
                worker.media.descriptions.get(&conn)?
            };
            Ok(crate::smart::matching_text(&descriptions, &vector, &neutral, 300))
        })
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not search"))?
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
        Ok(Json(json!({ "items": scored_items(&state, found) })))
    }
}

#[derive(Deserialize)]
struct SimilarQuery {
    path: Option<String>,
}

/// Photos that look like one in the library (`?path=`) or like an image sent in the body.
async fn gallery_similar(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SimilarQuery>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    #[cfg(not(ownnas_ml))]
    {
        let _ = (query, body);
        return Err::<Json<serde_json::Value>, _>(ApiError::new(StatusCode::BAD_REQUEST, "Smart search is not available on this device"));
    }
    #[cfg(ownnas_ml)]
    {
        search_ready(&state)?;
        let sample = match &query.path {
            Some(path) => Some(files::resolve(&state.root, path)?),
            None if body.is_empty() => return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a photo, or drop one in")),
            None => None,
        };
        let worker = state.clone();
        let skip = sample.as_ref().map(|target| target.rel.clone());
        let found = tokio::task::spawn_blocking(move || -> Result<Vec<(String, f32)>, String> {
            let stored = match &sample {
                Some(target) => {
                    let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
                    crate::smart::description(&conn, &target.rel)?.filter(|vector| !vector.is_empty())
                }
                None => None,
            };
            let vector = match stored {
                Some(vector) => vector,
                None => {
                    let image = match &sample {
                        Some(target) => crate::thumbs::decode_for_analysis(&target.full, 640)?,
                        None => {
                            // Written to a file first, so HEIC and the like go through the converter too.
                            let temp = std::env::temp_dir().join(format!("ownnas-sample-{}", rand::random::<u64>()));
                            let kind = image::guess_format(&body).ok().and_then(|format| format.extensions_str().first().copied()).unwrap_or("heic");
                            let file = temp.with_extension(kind);
                            std::fs::write(&file, &body).map_err(|_| "Could not read the image".to_string())?;
                            let decoded = crate::thumbs::decode_for_analysis(&file, 640);
                            let _ = std::fs::remove_file(&file);
                            decoded.map_err(|_| "That does not look like a picture OwnNAS can read".to_string())?
                        }
                    };
                    worker.models.describe_image(&image, 2)?
                }
            };
            let descriptions = {
                let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
                worker.media.descriptions.get(&conn)?
            };
            let found = crate::smart::nearest(&descriptions, &vector, 200, skip.as_deref());
            Ok(found.into_iter().filter(|(_, score)| *score >= 0.6).collect())
        })
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not search"))?
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
        Ok(Json(json!({ "items": scored_items(&state, found) })))
    }
}

#[derive(Deserialize)]
struct PeopleQuery {
    hidden: Option<String>,
}

async fn list_people(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PeopleQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let hidden = query.hidden.as_deref() == Some("1");
    let people = crate::smart::people(&conn, hidden).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    let hidden_count = if hidden { 0 } else { crate::smart::people(&conn, true).map(|list| list.len()).unwrap_or(0) };
    Ok(Json(json!({ "people": people, "hiddenCount": hidden_count })))
}

#[derive(Deserialize)]
struct PersonQuery {
    id: i64,
}

async fn person_photos(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PersonQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let (person, photos) = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let person = crate::smart::person(&conn, query.id)
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That person is no longer there"))?;
        (person, crate::smart::person_photos(&conn, query.id).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?)
    };
    let face_ids: Vec<i64> = photos.iter().map(|(_, face)| *face).collect();
    let items = gallery_items(&state, photos.into_iter().map(|(item, _)| item).collect(), true)
        .into_iter()
        .zip(face_ids)
        .map(|(mut item, face)| {
            item["faceId"] = json!(face);
            item
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "person": person, "items": items })))
}

#[derive(Deserialize)]
struct PersonChangeBody {
    id: i64,
    name: Option<String>,
    hidden: Option<bool>,
    /// Merge this person into another (they are the same).
    into: Option<i64>,
}

async fn change_person(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PersonChangeBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let fail = |err: String| ApiError::new(StatusCode::BAD_REQUEST, err);
    if let Some(name) = &body.name {
        crate::smart::rename_person(&conn, body.id, name).map_err(fail)?;
    }
    if let Some(hidden) = body.hidden {
        crate::smart::hide_person(&conn, body.id, hidden).map_err(fail)?;
    }
    let id = match body.into {
        Some(into) => {
            crate::smart::merge_people(&conn, body.id, into).map_err(fail)?;
            into
        }
        None => body.id,
    };
    drop(conn);
    record(&state, &user, "person-change", &id.to_string());
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    Ok(Json(json!({ "person": crate::smart::person(&conn, id).map_err(fail)? })))
}

#[derive(Deserialize)]
struct MoveFacesBody {
    faces: Vec<i64>,
    /// Another person, or null for "not this person" (a new person of their own).
    to: Option<i64>,
}

async fn move_faces(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<MoveFacesBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    if body.faces.is_empty() || body.faces.len() > 10_000 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose some faces"));
    }
    let to = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        crate::smart::move_faces(&conn, &body.faces, body.to).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?
    };
    record(&state, &user, "faces-move", &format!("{} faces", body.faces.len()));
    Ok(Json(json!({ "to": to })))
}

#[derive(Deserialize)]
struct FaceQuery {
    id: i64,
}

/// A face cut out of its photo (square, 192 pixels), for the People view.
async fn face_crop(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FaceQuery>,
) -> Result<Response, ApiError> {
    require_user(&state, &headers)?;
    let found = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        crate::smart::face_box(&conn, query.id).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?
    };
    let (path, x, y, w, h) = found.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "No such face"))?;
    let target = files::resolve(&state.root, &path)?;
    let cache = state.thumbs.join("faces");
    let id = query.id;
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let meta = std::fs::metadata(&target.full).map_err(|_| "The photo is gone".to_string())?;
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let file = cache.join(format!("{id}-{}-{modified}.jpg", meta.len()));
        if let Ok(bytes) = std::fs::read(&file) {
            return Ok(bytes);
        }
        let image = crate::thumbs::decode_for_analysis(&target.full, 1600)?;
        let (iw, ih) = (image.width() as f32, image.height() as f32);
        // A square around the face, with some room for hair and chin.
        let side = (w * iw).max(h * ih) * 1.6;
        let cx = (x + w / 2.0) * iw;
        let cy = (y + h / 2.0) * ih;
        let left = (cx - side / 2.0).clamp(0.0, (iw - 1.0).max(0.0));
        let top = (cy - side / 2.0).clamp(0.0, (ih - 1.0).max(0.0));
        let side = side.min(iw - left).min(ih - top).max(1.0);
        let crop = image.crop_imm(left as u32, top as u32, side as u32, side as u32).resize_exact(192, 192, image::imageops::FilterType::CatmullRom);
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(&crop.to_rgb8())
            .map_err(|_| "Could not make the picture".to_string())?;
        let _ = std::fs::create_dir_all(&cache);
        let _ = std::fs::write(&file, &out);
        Ok(out)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not make the picture"))?
    .map_err(|err| ApiError::new(StatusCode::NOT_FOUND, err))?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "private, max-age=86400")], bytes).into_response())
}

/// The people in one photo, for the preview.
async fn faces_in_photo(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let path = rel_of(&query.path);
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let faces = crate::smart::faces_in(&conn, &path).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!({ "faces": faces.into_iter().map(|(id, person, name)| json!({ "id": id, "personId": person, "name": name })).collect::<Vec<_>>() })))
}

// ---- Phone backups: a media upload folder that phones back their photos up to ----

const SETTING_BACKUP_FOLDER: &str = "backup_folder";

fn backup_folder(state: &AppState) -> Option<String> {
    setting(state, SETTING_BACKUP_FOLDER).filter(|folder| !folder.is_empty())
}

async fn backup_config(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    Ok(Json(json!({ "folder": backup_folder(&state) })))
}

#[derive(Deserialize)]
struct BackupConfigBody {
    /// A folder in the library ("" or null turns phone backups off).
    folder: Option<String>,
}

/// Sets where phones back their photos up to (administrators). The folder is made if needed.
async fn set_backup_config(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BackupConfigBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    require_write(&state)?;
    let folder = body.folder.unwrap_or_default().trim().trim_matches('/').to_string();
    if !folder.is_empty() {
        if files::is_under_trash(&folder) {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a folder outside the Trash"));
        }
        // Made now (like a folder upload would), so it shows in the library straight away.
        let root = state.root.clone();
        let probe = format!("{folder}/.ownnas-probe");
        tokio::task::spawn_blocking(move || files::prepare_upload(&root, "", &probe, Some(files::UploadChoice::KeepBoth)))
            .await
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not make the folder"))?
            .map_err(ApiError::from)?;
    }
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::set_setting(&conn, SETTING_BACKUP_FOLDER, (!folder.is_empty()).then_some(folder.as_str()))
            .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    }
    record(&state, &user, "backup-folder", if folder.is_empty() { "off" } else { &folder });
    Ok(Json(json!({ "folder": (!folder.is_empty()).then_some(folder) })))
}

#[derive(Deserialize)]
struct BackupUploadQuery {
    /// The phone's name: each phone gets its own folder.
    device: String,
    name: String,
    /// When the photo was taken or last changed, in milliseconds (also picks the month folder).
    mtime: i64,
    size: Option<u64>,
    /// The file's SHA-256 (hex). Without it (older apps) a same-named file of the same size counts
    /// as the same photo.
    #[serde(default)]
    sha256: Option<String>,
}

/// Where a photo goes: `<backup folder>/<phone>/<year>/<month>/<name>`.
fn backup_target(state: &AppState, query: &BackupUploadQuery) -> Result<String, ApiError> {
    let folder = backup_folder(state)
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "Phone backups are off. An administrator can choose a media upload folder in Settings → Gallery."))?;
    let device = safe_name(&query.device, "Phone");
    let name = safe_name(&query.name, "photo.jpg");
    let (year, month) = year_month(query.mtime / 1000);
    Ok(format!("{folder}/{device}/{year:04}/{month:02}/{name}"))
}

/// Whether `candidate` is `wanted` or a numbered copy of it ("IMG_1 (2).jpg" for "IMG_1.jpg").
fn same_or_numbered(candidate: &str, wanted: &str) -> bool {
    if candidate == wanted {
        return true;
    }
    let (stem, ext) = match wanted.rfind('.') {
        Some(dot) if dot > 0 => (&wanted[..dot], &wanted[dot..]),
        _ => (wanted, ""),
    };
    let Some(rest) = candidate.strip_prefix(stem).and_then(|rest| rest.strip_prefix(" (")) else {
        return false;
    };
    let Some(number) = rest.strip_suffix(ext).and_then(|rest| rest.strip_suffix(')')) else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
}

/// A file's SHA-256 (hex), from the cache while its size and time are unchanged.
fn file_sha256(state: &AppState, rel: &str, full: &std::path::Path, meta: &std::fs::Metadata) -> Option<String> {
    use sha2::Digest;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(hash) = db::cached_hash(&conn, rel, meta.len(), mtime) {
            return Some(hash);
        }
    }
    let mut file = std::fs::File::open(full).ok()?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    let hash = hex::encode(hasher.finalize());
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::store_hash(&conn, rel, meta.len(), mtime, &hash);
    Some(hash)
}

/// The photo already in its month folder, under its name or a numbered copy of it: the same size
/// and contents. Without a hash, only a file with its exact name and size counts.
fn find_backup_copy(state: &AppState, target: &str, size: Option<u64>, sha256: Option<&str>) -> Option<String> {
    let (dir, wanted) = target.rsplit_once('/')?;
    let found = files::resolve(&state.root, dir).ok()?;
    let Some(sha256) = sha256 else {
        let meta = std::fs::metadata(found.full.join(wanted)).ok()?;
        return (meta.is_file() && Some(meta.len()) == size).then(|| target.to_string());
    };
    for entry in std::fs::read_dir(&found.full).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !same_or_numbered(&name, wanted) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() || size.is_some_and(|size| size != meta.len()) {
            continue;
        }
        let rel = format!("{dir}/{name}");
        if file_sha256(state, &rel, &entry.path(), &meta).is_some_and(|hash| hash.eq_ignore_ascii_case(sha256)) {
            return Some(rel);
        }
    }
    None
}

/// A smaller copy of the photo with this hash (or a copy of a copy) that is still in the library:
/// a phone with the original need not send it again.
fn find_resized_copy(state: &AppState, sha256: &str) -> Option<String> {
    let candidates: Vec<(String, u64, i64)> = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::later_versions(&conn, sha256).iter().flat_map(|version| db::paths_with_hash(&conn, &version.sha256)).collect()
    };
    candidates.into_iter().find_map(|(path, size, mtime)| {
        if files::is_under_trash(&path) {
            return None;
        }
        let found = files::resolve(&state.root, &path).ok()?;
        let meta = std::fs::metadata(&found.full).ok()?;
        let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
        (meta.is_file() && meta.len() == size && modified == mtime).then_some(path)
    })
}

fn backup_blocked(state: &AppState, sha256: &str) -> bool {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::is_backup_blocked(&conn, sha256)
}

/// A small JPEG of a photo or video, kept with a deleted photo so people can see what it was.
fn small_preview(state: &AppState, full: &std::path::Path, name: &str) -> Option<Vec<u8>> {
    if !files::can_thumb(name, state.ffmpeg) {
        return None;
    }
    let cached = thumbs::ensure(full, &state.thumbs, files::kind_of(name) == "video").ok()?;
    let bytes = thumbs::read_cached(&cached).ok()?;
    let picture = image::load_from_memory(&bytes).ok()?.thumbnail(192, 192).to_rgb8();
    let mut out = Vec::new();
    picture.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).ok()?;
    Some(out)
}

#[derive(Deserialize)]
struct BlockBody {
    paths: Vec<String>,
    /// Also delete the files (into Trash, or for good when already there). On by default.
    #[serde(default = "default_true")]
    delete: bool,
}

fn default_true() -> bool {
    true
}

/// Remembers photos (by SHA-256) so phones never back them up again, and deletes them.
async fn block_backups(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BlockBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let worker = state.clone();
    let (blocked, problems) = tokio::task::spawn_blocking(move || {
        let state = worker;
        let mut blocked = 0;
        let mut problems = Vec::new();
        for raw in body.paths {
            let rel = rel_of(&Some(raw.clone()));
            let outcome = (|| -> Result<(), ApiError> {
                let resolved = files::resolve(&state.root, &rel)?;
                let meta = std::fs::metadata(&resolved.full).map_err(|_| ApiError::from(FileError::NotFound))?;
                if !meta.is_file() {
                    return Err(ApiError::new(StatusCode::BAD_REQUEST, "Only photos and videos can be blocked, not folders"));
                }
                let name = resolved.full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let sha256 = file_sha256(&state, &resolved.rel, &resolved.full, &meta)
                    .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the file"))?;
                let preview = small_preview(&state, &resolved.full, &name);
                {
                    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
                    let taken_at = conn
                        .query_row("SELECT taken_at FROM media_index WHERE path = ?1", [&resolved.rel], |r| r.get::<_, Option<i64>>(0))
                        .ok()
                        .flatten()
                        .or_else(|| meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64));
                    let photo = db::BlockedPhoto {
                        sha256,
                        size: meta.len() as i64,
                        name,
                        path: resolved.rel.clone(),
                        taken_at,
                        blocked_at: db::now_secs(),
                        blocked_by: user.username.clone(),
                    };
                    db::block_backup(&conn, &photo, preview.as_deref()).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
                }
                if body.delete {
                    let (action, new_path) = files::trash_or_delete(&state.root, &resolved.rel)?;
                    match new_path {
                        Some(to) => rewrite_meta(&state, &resolved.rel, &to),
                        None => delete_meta(&state, &resolved.rel),
                    }
                    record(&state, &user, action, &resolved.rel);
                }
                record(&state, &user, "backup-block", &resolved.rel);
                Ok(())
            })();
            match outcome {
                Ok(()) => blocked += 1,
                Err(err) => problems.push(json!({ "path": raw, "error": err.body.get("error").cloned().unwrap_or_default() })),
            }
        }
        (blocked, problems)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not delete the photos"))?;
    if blocked > 0 {
        state.media.rescan_soon();
    }
    Ok(Json(json!({ "blocked": blocked, "problems": problems })))
}

/// Photos deleted from the server that phones will not back up again.
async fn blocked_backups(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let photos = db::blocked_backups(&conn).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!({ "photos": photos })))
}

#[derive(Deserialize)]
struct ShaQuery {
    sha256: String,
}

async fn blocked_preview(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ShaQuery>,
) -> Result<Response, ApiError> {
    require_user(&state, &headers)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let bytes = db::blocked_preview(&conn, &query.sha256).ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "No preview"))?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "private, max-age=86400")], bytes).into_response())
}

/// Lets phones back a deleted photo up again.
async fn unblock_backup(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ShaQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let found = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        db::unblock_backup(&conn, &query.sha256).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?
    };
    if found {
        record(&state, &user, "backup-unblock", &query.sha256);
    }
    Ok(Json(json!({ "ok": found })))
}

/// Whether a phone's photo is on the server already, so it need not be sent.
async fn backup_has(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<BackupUploadQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let target = backup_target(&state, &query)?;
    if query.sha256.as_deref().is_some_and(|hash| backup_blocked(&state, hash)) {
        return Ok(Json(json!({ "path": null, "blocked": true })));
    }
    let found = tokio::task::spawn_blocking(move || {
        find_backup_copy(&state, &target, query.size, query.sha256.as_deref())
            .or_else(|| query.sha256.as_deref().and_then(|hash| find_resized_copy(&state, hash)))
    })
    .await
    .unwrap_or(None);
    Ok(Json(json!({ "path": found })))
}

/// A name safe to use as one folder or file name.
fn safe_name(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() { fallback.to_string() } else { cleaned.chars().take(120).collect() }
}

/// One photo or video from a phone, filed as `<backup folder>/<phone>/<year>/<month>/<name>`.
/// A copy already there (same contents, under its name or a numbered one) is not stored again
/// (`skipped`); a different photo with the same name is kept too, under a numbered name. The
/// original time is kept, and a file whose contents don't match its hash is refused.
async fn backup_upload(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<BackupUploadQuery>,
    body: Body,
) -> Result<impl IntoResponse, ApiError> {
    use sha2::Digest;
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let rel = backup_target(&state, &query)?;
    let sha256 = query.sha256.as_ref().map(|hash| hash.to_ascii_lowercase());
    if sha256.as_ref().is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "The photo's hash is not valid"));
    }
    let probe = (state.clone(), rel.clone(), query.size, sha256.clone());
    let already = tokio::task::spawn_blocking(move || {
        find_backup_copy(&probe.0, &probe.1, probe.2, probe.3.as_deref())
            .or_else(|| probe.3.as_deref().and_then(|hash| find_resized_copy(&probe.0, hash)))
    })
    .await
    .unwrap_or(None);
    if let Some(path) = already {
        return Ok(Json(json!({ "path": path, "skipped": true })));
    }
    if sha256.as_deref().is_some_and(|hash| backup_blocked(&state, hash)) {
        return Ok(Json(json!({ "path": "", "skipped": true, "blocked": true })));
    }
    let root = state.root.clone();
    let target = rel.clone();
    let prepared = tokio::task::spawn_blocking(move || files::prepare_upload(&root, "", &target, Some(files::UploadChoice::KeepBoth)))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload"))?
        .map_err(ApiError::from)?;
    let files::PreparedUpload::Write(dest) = prepared else {
        return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload"));
    };
    let partial = dest.with_file_name(format!(".{}.part", dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let written = async {
        let mut out = tokio::fs::File::create(&partial).await.ok()?;
        let mut stream = body.into_data_stream();
        let mut total = 0u64;
        let mut hasher = sha2::Sha256::new();
        while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
            let chunk = chunk.ok()?;
            total += chunk.len() as u64;
            hasher.update(&chunk);
            tokio::io::AsyncWriteExt::write_all(&mut out, &chunk).await.ok()?;
        }
        tokio::io::AsyncWriteExt::flush(&mut out).await.ok()?;
        let file = out.into_std().await;
        if query.mtime > 0 {
            let _ = file.set_modified(std::time::UNIX_EPOCH + Duration::from_millis(query.mtime as u64));
        }
        Some((total, hex::encode(hasher.finalize())))
    }
    .await;
    // Only a complete file takes the photo's name, so an interrupted upload is never mistaken for one.
    let complete = match (&written, query.size) {
        (Some((got, _)), Some(want)) => *got == want,
        (Some(_), None) => true,
        (None, _) => false,
    };
    let damaged = matches!((&written, &sha256), (Some((_, got)), Some(want)) if got != want);
    // Older apps send no hash: a deleted photo is only recognised once it has arrived.
    if complete && !damaged && matches!(&written, Some((_, got)) if backup_blocked(&state, got)) {
        let _ = tokio::fs::remove_file(&partial).await;
        return Ok(Json(json!({ "path": "", "skipped": true, "blocked": true })));
    }
    if !complete || damaged || tokio::fs::rename(&partial, &dest).await.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
        let message = if damaged { "The photo arrived damaged or changed while it was sent" } else { "The upload was interrupted" };
        return Err(ApiError::new(StatusCode::BAD_REQUEST, message));
    }
    let saved = dest
        .strip_prefix(state.root.canonicalize().unwrap_or_else(|_| state.root.clone()))
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or(rel);
    // The next phone asking about this photo needn't read it again.
    if let (Some((size, hash)), Ok(meta)) = (&written, std::fs::metadata(&dest)) {
        if let Some(mtime) = meta.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()) {
            let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
            let _ = db::store_hash(&conn, &saved, *size, mtime.as_millis() as i64, hash);
        }
    }
    invalidate_sizes(&state, &parent_rel_path(&saved));
    record(&state, &user, "backup", &saved);
    state.media.rescan_soon();
    Ok(Json(json!({ "path": saved, "skipped": false })))
}

/// The year and month (UTC) of a Unix time.
fn year_month(secs: i64) -> (i64, i64) {
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month)
}

async fn gallery_hidden(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let items = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        crate::albums::hidden(&conn)?
    };
    Ok(Json(json!({ "items": gallery_items(&state, items, true) })))
}

#[derive(Deserialize)]
struct HiddenBody {
    paths: Vec<String>,
    hidden: bool,
}

/// Leaves photos out of the Photos timeline, or puts them back. The files are not touched.
async fn set_gallery_hidden(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<HiddenBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let mut paths = Vec::new();
    for raw in &body.paths {
        let path = raw.trim().trim_start_matches('/').to_string();
        if body.hidden {
            let target = files::resolve(&state.root, &path)?;
            if !target.full.is_file() || !matches!(files::kind_of(&target.rel), "image" | "video" | "svg") {
                return Err(ApiError::new(StatusCode::BAD_REQUEST, "Only photos and videos can be hidden from Photos"));
            }
            paths.push(target.rel);
        } else {
            paths.push(path);
        }
    }
    let changed = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        crate::albums::set_hidden(&conn, &paths, body.hidden, &user.username)?
    };
    record(&state, &user, if body.hidden { "photos-hide" } else { "photos-show" }, &format!("{changed} photos"));
    Ok(Json(json!({ "changed": changed })))
}

async fn gallery_status(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    Ok(Json(state.media.status()))
}

async fn gallery_rescan(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    require_user(&state, &headers)?;
    state.media.rescan();
    Ok(Json(state.media.status()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimelineQuery {
    before: Option<i64>,
    before_path: Option<String>,
    limit: Option<usize>,
    /// With `to`, the photos taken in `[from, to)` instead of a page.
    from: Option<i64>,
    to: Option<i64>,
}

async fn gallery_timeline(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TimelineQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let ranged = query.from.zip(query.to);
    let limit = query.limit.unwrap_or(200).clamp(1, if ranged.is_some() { 50_000 } else { 1000 });
    let before = query.before.map(|at| (at, query.before_path.unwrap_or_default()));
    let items = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        match ranged {
            Some((from, to)) => crate::albums::taken_between(&conn, from, to, limit)?,
            None => crate::albums::timeline(&conn, before, limit)?,
        }
    };
    let more = items.len() == limit;
    Ok(Json(json!({ "items": gallery_items(&state, items, false), "more": more, "status": state.media.status() })))
}

#[derive(Deserialize)]
struct AlbumQuery {
    id: Option<i64>,
}

async fn get_album(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AlbumQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let (album, breadcrumb, children, items) = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        match query.id {
            Some(id) => (
                Some(crate::albums::get(&conn, id)?),
                crate::albums::breadcrumb(&conn, id)?,
                crate::albums::children(&conn, Some(id))?,
                crate::albums::items(&conn, id)?,
            ),
            None => (None, Vec::new(), crate::albums::children(&conn, None)?, Vec::new()),
        }
    };
    Ok(Json(json!({
        "album": album,
        "breadcrumb": breadcrumb.into_iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
        "albums": children,
        "items": gallery_items(&state, items, true),
    })))
}

async fn album_tree(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    Ok(Json(json!({ "albums": crate::albums::all(&conn)? })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAlbumBody {
    name: String,
    parent_id: Option<i64>,
}

async fn create_album(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateAlbumBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let album = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let id = crate::albums::create(&conn, body.parent_id, &body.name)?;
        crate::albums::get(&conn, id)?
    };
    record(&state, &user, "album-create", &album.name);
    Ok(Json(json!({ "album": album })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAlbumBody {
    id: i64,
    name: Option<String>,
    /// With `move: true`, the album goes inside `parentId` (or to the top when it is null).
    #[serde(rename = "move", default)]
    move_album: bool,
    parent_id: Option<i64>,
    /// With `setCover: true`, `cover` becomes the album's cover (null goes back to automatic).
    #[serde(default)]
    set_cover: bool,
    cover: Option<String>,
}

async fn update_album(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UpdateAlbumBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let album = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(name) = &body.name {
            crate::albums::rename(&conn, body.id, name)?;
        }
        if body.move_album {
            crate::albums::move_to(&conn, body.id, body.parent_id)?;
        }
        if body.set_cover {
            let cover = body.cover.as_deref().map(|path| path.trim().trim_start_matches('/'));
            crate::albums::set_cover(&conn, body.id, cover)?;
        }
        crate::albums::get(&conn, body.id)?
    };
    record(&state, &user, "album-update", &album.name);
    Ok(Json(json!({ "album": album })))
}

async fn delete_album(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AlbumQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let id = query.id.ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "Choose an album"))?;
    let name = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let name = crate::albums::get(&conn, id)?.name;
        crate::albums::delete(&conn, id)?;
        name
    };
    record(&state, &user, "album-delete", &name);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlbumItemsBody {
    album_id: i64,
    paths: Vec<String>,
    action: Option<String>,
}

async fn album_items(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AlbumItemsBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let add = body.action.as_deref() != Some("remove");
    let mut paths = Vec::new();
    for raw in &body.paths {
        let path = raw.trim().trim_start_matches('/').to_string();
        if add {
            // Only photos and videos that are really in the library go into albums.
            let target = files::resolve(&state.root, &path)?;
            let kind = files::kind_of(&target.rel);
            if !target.full.is_file() || !matches!(kind, "image" | "video" | "svg") {
                return Err(ApiError::new(StatusCode::BAD_REQUEST, "Only photos and videos can go in albums"));
            }
            paths.push(target.rel);
        } else {
            paths.push(path);
        }
    }
    let (changed, album) = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let changed = crate::albums::change_items(&conn, body.album_id, &paths, add, &user.username)?;
        (changed, crate::albums::get(&conn, body.album_id)?)
    };
    record(&state, &user, if add { "album-add" } else { "album-remove" }, &format!("{} ({changed})", album.name));
    Ok(Json(json!({ "changed": changed, "album": album })))
}

async fn all_tags(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let tags = crate::albums::tags(&conn)?;
    Ok(Json(json!({ "tags": tags.into_iter().map(|(tag, count)| json!({ "tag": tag, "count": count })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct GalleryTagQuery {
    tag: String,
}

async fn gallery_tag(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<GalleryTagQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let items = {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        crate::albums::tagged(&conn, query.tag.trim())?
    };
    let items: Vec<_> = gallery_items(&state, items, true)
        .into_iter()
        .filter(|item| matches!(item["kind"].as_str(), Some("image" | "video" | "svg")))
        .collect();
    Ok(Json(json!({ "items": items })))
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
    if extension_of(name).as_str() == "dxf" {
        return "text/plain; charset=utf-8".to_string();
    }
    if kind == "text" || matches!(extension_of(name).as_str(), "html" | "htm" | "xhtml" | "xml" | "js" | "mjs")
    {
        return "text/plain; charset=utf-8".to_string();
    }
    if kind == "svg" {
        return "image/svg+xml".to_string();
    }
    if kind == "model3d" {
        return match extension_of(name).as_str() {
            "obj" => "model/obj".to_string(),
            "stl" => "model/stl".to_string(),
            "gltf" => "model/gltf+json".to_string(),
            "glb" => "model/gltf-binary".to_string(),
            "ply" => "model/ply".to_string(),
            "3mf" => "model/3mf".to_string(),
            _ => "application/octet-stream".to_string(),
        };
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
    let (content_type, image) = tokio::task::spawn_blocking(move || -> Result<(&'static str, Vec<u8>), String> {
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
        Ok((thumbs::content_type(&cached), thumbs::read_cached(&cached)?))
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not make a thumbnail"))?
    .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "No thumbnail"))?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
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

#[derive(Deserialize)]
struct WriteDocxBody {
    path: String,
    data: String,
    expected_hash: String,
}

async fn write_docx_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WriteDocxBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let bytes = base64_decode(&body.data)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "Invalid document data"))?;
    let root = state.root.clone();
    let path = body.path.clone();
    let hash = tokio::task::spawn_blocking(move || files::write_docx_bytes(&root, &path, &bytes, &body.expected_hash))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not save document"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&body.path));
    record(&state, &user, "edit-document", &body.path);
    Ok(Json(json!({ "ok": true, "hash": hash })))
}

#[derive(Deserialize)]
struct WriteOdtBody {
    path: String,
    data: String,
    expected_hash: String,
}

async fn write_odt_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WriteOdtBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let bytes = base64_decode(&body.data)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "Invalid document data"))?;
    let root = state.root.clone();
    let path = body.path.clone();
    let expected_hash = body.expected_hash.clone();
    let hash = tokio::task::spawn_blocking(move || files::write_odt_bytes(&root, &path, &bytes, &expected_hash))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not save document"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&body.path));
    record(&state, &user, "edit-document", &body.path);
    Ok(Json(json!({ "ok": true, "hash": hash })))
}

#[derive(Deserialize)]
struct WriteXlsxBody {
    path: String,
    data: String,
    expected_hash: String,
}

async fn write_xlsx_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WriteXlsxBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let bytes = base64_decode(&body.data)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "Invalid spreadsheet data"))?;
    let root = state.root.clone();
    let path = body.path.clone();
    let expected_hash = body.expected_hash.clone();
    let hash = tokio::task::spawn_blocking(move || files::write_xlsx_bytes(&root, &path, &bytes, &expected_hash))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not save spreadsheet"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&body.path));
    record(&state, &user, "edit-spreadsheet", &body.path);
    Ok(Json(json!({ "ok": true, "hash": hash })))
}

async fn write_image_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WriteImageBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let raw = body.data.trim();
    let b64 = raw
        .rsplit_once(',')
        .map(|(_, data)| data)
        .unwrap_or(raw);
    let bytes = base64_decode(b64)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "Image data is not valid base64"))?;
    let root = state.root.clone();
    let path = body.path.clone();
    tokio::task::spawn_blocking(move || files::write_image_bytes(&root, &path, &bytes))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the image"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&body.path));
    record(&state, &user, "edit-image", &body.path);
    Ok(Json(json!({ "ok": true })))
}

async fn convert_image_entry(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ConvertImageBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let path = body.path.clone();
    let format = body.format.clone();
    let created = tokio::task::spawn_blocking(move || files::convert_image(&root, &path, &format))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not convert the image"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&created));
    record(
        &state,
        &user,
        "convert-image",
        &format!("{} → {created}", body.path),
    );
    Ok(Json(json!({ "ok": true, "path": created })))
}

async fn pdf_info(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (_user, _) = require_user(&state, &headers)?;
    let root = state.root.clone();
    let path = rel_of(&query.path);
    let info = tokio::task::spawn_blocking(move || pdf_ops::pdf_info(&root, &path))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the PDF"))?
        .map_err(ApiError::from)?;
    Ok(Json(info))
}

async fn pdf_extract(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PdfExtractBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let path = body.path.clone();
    let pages = body.pages.clone();
    let mode = body.mode.clone();
    let created = tokio::task::spawn_blocking(move || pdf_ops::pdf_extract(&root, &path, &pages, &mode))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not extract PDF pages"))?
        .map_err(ApiError::from)?;
    if let Some(first) = created.first() {
        invalidate_sizes(&state, &parent_rel_path(first));
    }
    record(
        &state,
        &user,
        "pdf-extract",
        &format!("{} → {} file(s)", body.path, created.len()),
    );
    Ok(Json(json!({ "ok": true, "paths": created })))
}

async fn pdf_split(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PdfPathBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let path = body.path.clone();
    let created = tokio::task::spawn_blocking(move || pdf_ops::pdf_split_all(&root, &path))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not split the PDF"))?
        .map_err(ApiError::from)?;
    if let Some(first) = created.first() {
        invalidate_sizes(&state, &parent_rel_path(first));
    }
    record(
        &state,
        &user,
        "pdf-split",
        &format!("{} → {} page(s)", body.path, created.len()),
    );
    Ok(Json(json!({ "ok": true, "paths": created })))
}

async fn pdf_merge(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PdfMergeBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    if body.paths.len() < 2 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Select at least two PDFs"));
    }
    let root = state.root.clone();
    let paths = body.paths.clone();
    let created = tokio::task::spawn_blocking(move || pdf_ops::pdf_merge(&root, &paths))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not join the PDFs"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&created));
    record(
        &state,
        &user,
        "pdf-merge",
        &format!("{} PDFs → {created}", body.paths.len()),
    );
    Ok(Json(json!({ "ok": true, "path": created })))
}

async fn pdf_rotate(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PdfRotateBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let path = body.path.clone();
    let pages = body.pages.clone().unwrap_or_default();
    let degrees = body.degrees;
    let created = tokio::task::spawn_blocking(move || pdf_ops::pdf_rotate(&root, &path, &pages, degrees))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not rotate the PDF"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&created));
    record(
        &state,
        &user,
        "pdf-rotate",
        &format!("{} → {created}", body.path),
    );
    Ok(Json(json!({ "ok": true, "path": created })))
}

fn base64_decode(input: &str) -> Result<Vec<u8>, ()> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let clean: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if clean.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in &clean {
        if c == b'=' {
            break;
        }
        let val = TABLE.iter().position(|&x| x == c).ok_or(())? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

async fn find_duplicates(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (_user, _) = require_user(&state, &headers)?;
    let root = state.root.clone();
    let path = rel_of(&query.path);
    let cancel = files::CancelFlag::new();
    let _guard = CancelOnDrop(cancel.clone());
    let groups = tokio::task::spawn_blocking(move || files::find_duplicates(&root, &path, &cancel))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not scan for duplicates"))?
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "groups": groups })))
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
    if let Some(ref to) = new_path {
        rewrite_meta(&state, &label, &to);
    } else {
        delete_meta(&state, &label);
    }
    record(&state, &user, action, &label);
    Ok(Json(json!({ "ok": true, "action": action, "path": new_path })))
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

async fn upload_directories(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UploadDirectoriesBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    if body.names.len() > files::MAX_LIST {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "That folder contains too many directories"));
    }
    let root = state.root.clone();
    let parent = rel_of(&body.path);
    let parent_for_create = parent.clone();
    let names = body.names;
    let count = names.len();
    tokio::task::spawn_blocking(move || files::ensure_upload_dirs(&root, &parent_for_create, &names))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not create uploaded folders"))?
        .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent);
    if count > 0 {
        record(&state, &user, "upload-folders", &format!("{count} folder(s)"));
    }
    Ok(Json(json!({ "ok": true })))
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
        // Keep the file's original modification time when the client sends it.
        if let Some(mtime) = query.mtime.filter(|mtime| *mtime > 0) {
            tokio::io::AsyncWriteExt::flush(&mut out)
                .await
                .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload"))?;
            let file = out.into_std().await;
            let _ = file.set_modified(std::time::UNIX_EPOCH + Duration::from_millis(mtime as u64));
        } else {
            drop(out);
        }
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

#[derive(Deserialize)]
struct SyncQuery {
    path: Option<String>,
    /// Expected current version: `none`, `dir`, or `<size>:<mtime-ms>`.
    base: Option<String>,
    /// Modification time (ms since the Unix epoch) to give an uploaded file.
    mtime: Option<i64>,
}

#[derive(Deserialize)]
struct SyncPathBody {
    path: String,
    base: Option<String>,
}

fn sync_conflict(current: Option<files::SyncEntry>) -> ApiError {
    ApiError {
        status: StatusCode::CONFLICT,
        body: json!({ "error": "The item changed on the server", "current": current }),
    }
}

async fn sync_tree(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SyncQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let entries = tokio::task::spawn_blocking(move || files::sync_tree(&root, &rel))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not list the folder"))?
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "readonly": state.readonly, "entries": entries })))
}

async fn sync_download(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SyncQuery>,
) -> Result<Response, ApiError> {
    require_user(&state, &headers)?;
    let resolved = files::resolve(&state.root, &rel_of(&query.path))?;
    if resolved.full.is_dir() {
        return Err(FileError::IsADirectory.into());
    }
    let file = tokio::fs::File::open(&resolved.full)
        .await
        .map_err(|_| ApiError::from(FileError::NotFound))?;
    // Stamp from the open handle so the version matches the bytes being sent.
    let meta = file
        .metadata()
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the file"))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let len = meta.len();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, len)
        .header("x-ownnas-size", len)
        .header("x-ownnas-mtime", mtime)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(ReaderStream::new(file.take(len))))
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not send the file"))
}

async fn sync_upload(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SyncQuery>,
    body: Body,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let base = files::SyncBase::parse(query.base.as_deref())?;
    let root = state.root.clone();
    let rel = rel_of(&query.path);
    let (rel, dest, temp) = tokio::task::spawn_blocking(move || files::sync_prepare_write(&root, &rel))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload"))?
        .map_err(ApiError::from)?;
    let stored = async {
        let mut out = tokio::fs::File::create(&temp).await?;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
            let chunk = chunk.map_err(|err| io::Error::new(io::ErrorKind::ConnectionAborted, err))?;
            tokio::io::AsyncWriteExt::write_all(&mut out, &chunk).await?;
        }
        tokio::io::AsyncWriteExt::flush(&mut out).await?;
        out.sync_all().await
    }
    .await;
    if let Err(err) = stored {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(if err.kind() == io::ErrorKind::ConnectionAborted {
            ApiError::new(StatusCode::BAD_REQUEST, "The upload was interrupted")
        } else {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload")
        });
    }
    let mtime = query.mtime;
    let committed_rel = rel.clone();
    let outcome = tokio::task::spawn_blocking(move || files::sync_commit_write(&committed_rel, &dest, &temp, base, mtime))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the upload"))?
        .map_err(ApiError::from)?;
    match outcome {
        files::SyncOutcome::Done(entry) => {
            invalidate_sizes(&state, &rel);
            record(&state, &user, "sync-upload", &rel);
            Ok(Json(json!({ "entry": entry })))
        }
        files::SyncOutcome::Conflict(current) => Err(sync_conflict(current)),
    }
}

async fn sync_mkdir(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<SyncPathBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let entry = tokio::task::spawn_blocking(move || files::sync_mkdir(&root, &body.path))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not create the folder"))?
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "entry": entry })))
}

async fn sync_delete(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<SyncPathBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let base = files::SyncBase::parse(body.base.as_deref())?;
    let root = state.root.clone();
    let rel = body.path.clone();
    let outcome = tokio::task::spawn_blocking(move || files::sync_delete(&root, &rel, base))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not delete the item"))?
        .map_err(ApiError::from)?;
    match outcome {
        files::SyncOutcome::Done(trashed) => {
            match &trashed {
                Some(to) => rewrite_meta(&state, &body.path, to),
                None => delete_meta(&state, &body.path),
            }
            invalidate_sizes(&state, &body.path);
            record(&state, &user, "sync-delete", &body.path);
            Ok(Json(json!({ "ok": true, "trashed": trashed })))
        }
        files::SyncOutcome::Conflict(current) => Err(sync_conflict(current)),
    }
}

// ---------------------------------------------------------------------------
// Terminals
// ---------------------------------------------------------------------------

/// The client's socket address (absent only in tests without a listener).
struct Peer(Option<SocketAddr>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Peer(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalConfigBody {
    account_password: String,
    enabled: Option<bool>,
    /// New terminal password; empty removes it.
    password: Option<String>,
    /// Shell command; empty uses the default.
    shell: Option<String>,
}

#[derive(Deserialize)]
struct TerminalUnlockBody {
    password: String,
}

#[derive(Deserialize)]
struct TerminalOpenBody {
    path: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
struct TerminalIdQuery {
    id: String,
}

fn setting(state: &AppState, key: &str) -> Option<String> {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::get_setting(&conn, key).ok().flatten()
}

fn terminal_enabled(state: &AppState) -> bool {
    !state.readonly
        && crate::terminal::available()
        && setting(state, crate::terminal::SETTING_ENABLED).as_deref() == Some("1")
}

/// Loopback, private LAN, link-local, and CGNAT (Tailscale and similar VPNs) addresses.
fn local_network(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return local_network(std::net::IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// Terminals are refused over plain HTTP from the open internet. A reverse proxy on this
/// machine is trusted to report the real client and scheme.
fn terminal_transport_ok(state: &AppState, headers: &HeaderMap, peer: &Peer) -> bool {
    if state.https {
        return true;
    }
    let Some(peer) = peer.0 else { return false };
    if peer.ip().is_loopback() {
        let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        if header("x-forwarded-proto").is_some_and(|proto| proto.eq_ignore_ascii_case("https")) {
            return true;
        }
        if let Some(forwarded) = header("x-forwarded-for") {
            return forwarded
                .split(',')
                .next()
                .and_then(|ip| ip.trim().parse().ok())
                .is_some_and(local_network);
        }
        return true;
    }
    local_network(peer.ip())
}

fn terminal_locked_error() -> ApiError {
    ApiError {
        status: StatusCode::FORBIDDEN,
        body: json!({ "error": "Enter the terminal password", "locked": true }),
    }
}

/// Checks everything needed to use terminals and returns the user and their token hash.
fn terminal_access(state: &AppState, headers: &HeaderMap, peer: &Peer) -> Result<(User, String), ApiError> {
    let (user, token) = require_user(state, headers)?;
    require_admin(&user)?;
    if !crate::terminal::available() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "Terminals are not available on this platform"));
    }
    require_write(state)?;
    if !terminal_enabled(state) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Terminals are turned off. An administrator can turn them on in Settings.",
        ));
    }
    if !terminal_transport_ok(state, headers, peer) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Terminals need HTTPS when you connect from outside your network or VPN",
        ));
    }
    let token_hash = auth::token_hash(&token);
    if setting(state, crate::terminal::SETTING_PASSWORD).is_some() && !state.terminals.is_unlocked(&token_hash) {
        return Err(terminal_locked_error());
    }
    Ok((user, token_hash))
}

async fn terminal_config(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
) -> Result<impl IntoResponse, ApiError> {
    let (user, token) = require_user(&state, &headers)?;
    require_admin(&user)?;
    Ok(Json(json!({
        "available": crate::terminal::available() && !state.readonly,
        "enabled": terminal_enabled(&state),
        "passwordSet": setting(&state, crate::terminal::SETTING_PASSWORD).is_some(),
        "unlocked": state.terminals.is_unlocked(&auth::token_hash(&token)),
        "secureTransport": terminal_transport_ok(&state, &headers, &peer),
        "shell": setting(&state, crate::terminal::SETTING_SHELL).unwrap_or_default(),
        "defaultShell": crate::terminal::default_shell(),
    })))
}

async fn set_terminal_config(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TerminalConfigBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_admin(&user)?;
    if state.readonly || !crate::terminal::available() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "Terminals are not available on this server"));
    }
    // A stolen session cookie must not be enough to turn on a shell or change its password.
    let gate_key = format!("terminal-settings:{}", user.username);
    {
        let mut gate = state.attempts.lock().unwrap_or_else(|err| err.into_inner());
        if !login_allowed(&mut gate, &gate_key) {
            return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "Too many attempts. Wait a few minutes and try again."));
        }
    }
    if !auth::verify_password(&body.account_password, &user.password_hash) {
        let mut gate = state.attempts.lock().unwrap_or_else(|err| err.into_inner());
        login_failed(&mut gate, &gate_key);
        return Err(ApiError::new(StatusCode::FORBIDDEN, "Your account password is not correct"));
    }
    login_succeeded(&mut state.attempts.lock().unwrap_or_else(|err| err.into_inner()), &gate_key);
    let password_hash = match body.password.as_deref() {
        Some("") => Some(None),
        Some(password) => {
            auth::validate_password(password).map_err(|message| ApiError::new(StatusCode::BAD_REQUEST, message))?;
            Some(Some(auth::hash_password(password).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?))
        }
        None => None,
    };
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let save = |key: &str, value: Option<&str>| {
            db::set_setting(&conn, key, value).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))
        };
        if let Some(enabled) = body.enabled {
            save(crate::terminal::SETTING_ENABLED, Some(if enabled { "1" } else { "0" }))?;
        }
        if let Some(hash) = &password_hash {
            save(crate::terminal::SETTING_PASSWORD, hash.as_deref())?;
        }
        if let Some(shell) = &body.shell {
            let shell = shell.trim();
            save(crate::terminal::SETTING_SHELL, (!shell.is_empty()).then_some(shell))?;
        }
    }
    if body.enabled == Some(false) {
        state.terminals.kill_all();
    }
    if password_hash.is_some() || body.enabled == Some(false) {
        state.terminals.lock(None);
    }
    record(&state, &user, "terminal-settings", if terminal_enabled(&state) { "on" } else { "off" });
    Ok(Json(json!({ "ok": true })))
}

async fn terminal_unlock(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TerminalUnlockBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, token) = require_user(&state, &headers)?;
    require_admin(&user)?;
    let Some(hash) = setting(&state, crate::terminal::SETTING_PASSWORD) else {
        return Ok(Json(json!({ "ok": true })));
    };
    let gate_key = format!("terminal:{}", user.username);
    {
        let mut gate = state.attempts.lock().unwrap_or_else(|err| err.into_inner());
        if !login_allowed(&mut gate, &gate_key) {
            return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "Too many attempts. Wait a few minutes and try again."));
        }
    }
    if !auth::verify_password(&body.password, &hash) {
        let mut gate = state.attempts.lock().unwrap_or_else(|err| err.into_inner());
        login_failed(&mut gate, &gate_key);
        record(&state, &user, "terminal-unlock-failed", "");
        return Err(ApiError::new(StatusCode::FORBIDDEN, "That terminal password is not correct"));
    }
    login_succeeded(&mut state.attempts.lock().unwrap_or_else(|err| err.into_inner()), &gate_key);
    state.terminals.unlock(&auth::token_hash(&token));
    Ok(Json(json!({ "ok": true })))
}

async fn terminal_lock(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (_, token) = require_user(&state, &headers)?;
    state.terminals.lock(Some(&auth::token_hash(&token)));
    Ok(Json(json!({ "ok": true })))
}

async fn list_terminals(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
) -> Result<impl IntoResponse, ApiError> {
    terminal_access(&state, &headers, &peer)?;
    Ok(Json(json!({ "terminals": state.terminals.list() })))
}

async fn open_terminal(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Json(body): Json<TerminalOpenBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = terminal_access(&state, &headers, &peer)?;
    let resolved = files::resolve(&state.root, &rel_of(&body.path))?;
    if !resolved.full.is_dir() {
        return Err(FileError::NotADirectory.into());
    }
    let shell = setting(&state, crate::terminal::SETTING_SHELL);
    let agent_url = state.agent_url.get().cloned();
    let label = resolved.rel.clone();
    let username = user.username.clone();
    let worker = state.clone();
    let info = tokio::task::spawn_blocking(move || {
        worker.terminals.spawn(
            &resolved.full,
            &resolved.rel,
            shell.as_deref(),
            &username,
            body.cols.unwrap_or(100),
            body.rows.unwrap_or(30),
            agent_url.as_deref(),
        )
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not open a terminal"))?
    .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    record(&state, &user, "terminal-open", if label.is_empty() { "/" } else { &label });
    Ok(Json(json!({ "terminal": info })))
}

async fn close_terminal(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Query(query): Query<TerminalIdQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = terminal_access(&state, &headers, &peer)?;
    let session = state
        .terminals
        .remove(&query.id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That terminal is already closed"))?;
    let path = session.info().path;
    record(&state, &user, "terminal-close", if path.is_empty() { "/" } else { &path });
    Ok(Json(json!({ "ok": true })))
}

/// Browsers send cookies on cross-site WebSocket requests, so the Origin must match.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let origin_host = origin.split_once("://").map(|(_, host)| host).unwrap_or(origin);
    ["x-forwarded-host", "host"].iter().any(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|host| host.eq_ignore_ascii_case(origin_host))
    })
}

async fn terminal_socket(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Query(query): Query<TerminalIdQuery>,
    upgrade: axum::extract::ws::WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // A browser never attaches a bearer token by itself, so only cookie sign-ins need the Origin
    // check. Native apps send `Authorization: Bearer` on the handshake.
    if bearer_token(&headers).is_none() && !same_origin(&headers) {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "Cross-site terminal connections are not allowed"));
    }
    terminal_access(&state, &headers, &peer)?;
    let session = state
        .terminals
        .get(&query.id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That terminal is closed"))?;
    Ok(upgrade.on_upgrade(move |socket| terminal_pump(socket, session)))
}

fn exit_message(code: Option<u32>) -> axum::extract::ws::Message {
    axum::extract::ws::Message::Text(json!({ "type": "exit", "code": code }).to_string().into())
}

async fn terminal_pump(mut socket: axum::extract::ws::WebSocket, session: std::sync::Arc<crate::terminal::Session>) {
    use axum::extract::ws::Message;
    use crate::terminal::Event;
    use tokio::sync::broadcast::error::RecvError;

    let (scrollback, exit, mut events) = session.attach();
    if socket.send(Message::Binary(scrollback.into())).await.is_err() {
        return;
    }
    if let Some(code) = exit {
        let _ = socket.send(exit_message(code)).await;
        return;
    }
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(Event::Output(bytes)) => {
                    if socket.send(Message::Binary(bytes.as_ref().clone().into())).await.is_err() {
                        break;
                    }
                }
                Ok(Event::Exit(code)) => {
                    let _ = socket.send(exit_message(code)).await;
                    break;
                }
                Err(RecvError::Lagged(_)) => {
                    // This browser fell behind: redraw it from the scrollback.
                    let (scrollback, _, fresh) = session.attach();
                    events = fresh;
                    let reset = Message::Text(json!({ "type": "reset" }).to_string().into());
                    if socket.send(reset).await.is_err() || socket.send(Message::Binary(scrollback.into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Binary(bytes))) => session.write(bytes.to_vec()),
                Some(Ok(Message::Text(text))) => {
                    let Ok(control) = serde_json::from_str::<serde_json::Value>(text.as_str()) else { continue };
                    match control["type"].as_str() {
                        Some("input") => {
                            if let Some(data) = control["data"].as_str() {
                                session.write(data.as_bytes().to_vec());
                            }
                        }
                        Some("resize") => {
                            let size = |key: &str| control[key].as_u64().map(|value| value.min(u16::MAX as u64) as u16);
                            if let (Some(cols), Some(rows)) = (size("cols"), size("rows")) {
                                session.resize(cols, rows);
                            }
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Lets phones install OwnNAS as a home-screen app.
async fn manifest() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/manifest+json"), (header::CACHE_CONTROL, "no-cache")], MANIFEST)
}

fn png(bytes: &'static [u8]) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], bytes)
}

async fn terminal_page() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::X_FRAME_OPTIONS, "SAMEORIGIN"),
            (
                header::CONTENT_SECURITY_POLICY,
                // xterm.js writes its colors and cell sizes into <style> elements. This page
                // shows only terminal output, so inline styles are allowed here and nowhere else.
                "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; frame-ancestors 'self'; base-uri 'none'; form-action 'none'",
            ),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        TERMINAL_HTML,
    )
}

async fn terminal_javascript() -> impl IntoResponse {
    app_module_javascript(TERMINAL_JS)
}

async fn terminal_panel_javascript() -> impl IntoResponse {
    app_module_javascript(TERMINAL_PANEL_JS)
}

async fn terminal_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], TERMINAL_CSS)
}

async fn xterm_module() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=86400")],
        XTERM_JS,
    )
}

async fn xterm_fit_module() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=86400")],
        XTERM_FIT_JS,
    )
}

async fn xterm_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=86400")], XTERM_CSS)
}

// ---------------------------------------------------------------------------
// Coding agents in terminals
// ---------------------------------------------------------------------------

/// Starts the loopback-only listener that agent hooks report to, and the poller that
/// detects agents running in terminals. Hooks never see HTTPS or the network this way.
pub async fn start_agent_services(state: std::sync::Arc<AppState>) {
    if !crate::terminal::available() || state.agent_url.get().is_some() {
        return;
    }
    match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => {
            if let Ok(addr) = listener.local_addr() {
                let _ = state.agent_url.set(format!("http://{addr}/agent-events"));
            }
            let app = Router::new()
                .route("/agent-events", post(agent_event))
                .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
                .with_state(state.clone());
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
        }
        Err(err) => eprintln!("Agent status in terminals is unavailable: {err}"),
    }
    let poller = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let state = poller.clone();
            let _ = tokio::task::spawn_blocking(move || state.terminals.poll_agents()).await;
        }
    });
}

/// Maps a path an agent reported to a path in the library, if it is inside it.
fn library_path(root: &std::path::Path, cwd: &std::path::Path, path: &str) -> Option<String> {
    let path = std::path::Path::new(path);
    let full = if path.is_absolute() { path.to_path_buf() } else { cwd.join(path) };
    // A deleted file cannot be canonicalized, but its folder usually can.
    let full = full.canonicalize().ok().or_else(|| {
        let parent = full.parent()?.canonicalize().ok()?;
        Some(parent.join(full.file_name()?))
    })?;
    let root = root.canonicalize().ok()?;
    let rel = full.strip_prefix(&root).ok()?;
    let parts: Vec<String> = rel.components().map(|part| part.as_os_str().to_string_lossy().to_string()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

async fn agent_event(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> StatusCode {
    let Some(token) = headers.get("x-ownnas-agent-token").and_then(|value| value.to_str().ok()) else {
        return StatusCode::UNAUTHORIZED;
    };
    let Some(session) = state.terminals.by_hook_token(token) else {
        return StatusCode::UNAUTHORIZED;
    };
    let root = state.root.clone();
    let rel = |path: &str| library_path(&root, session.cwd(), path);
    let event = &body["event"];
    match body["agent"].as_str() {
        Some("claude") => session.update_agent(|agent| agent.apply_claude(event, &rel)),
        Some("opencode") => session.update_agent(|agent| agent.apply_opencode(event, &rel)),
        _ => return StatusCode::BAD_REQUEST,
    }
    StatusCode::NO_CONTENT
}

/// Server-sent events with the full terminal list whenever a terminal or its agent changes.
async fn agent_stream(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
) -> Result<impl IntoResponse, ApiError> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    terminal_access(&state, &headers, &peer)?;
    let changes = state.terminals.subscribe();
    let stream = futures_util::stream::unfold((state, changes, true), |(state, mut changes, first)| async move {
        if !first {
            match changes.recv().await {
                Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
            // Coalesce bursts (a tool call fires several hooks at once).
            tokio::time::sleep(Duration::from_millis(150)).await;
            while changes.try_recv().is_ok() {}
        }
        if !terminal_enabled(&state) {
            return None;
        }
        let payload = json!({ "terminals": state.terminals.list() }).to_string();
        Some((Ok::<_, std::convert::Infallible>(Event::default().event("terminals").data(payload)), (state, changes, false)))
    });
    Ok(([(header::CACHE_CONTROL, "no-store")], Sse::new(stream).keep_alive(KeepAlive::default())))
}

#[derive(Deserialize)]
struct TerminalIdBody {
    id: String,
}

async fn terminal_seen(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Json(body): Json<TerminalIdBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    terminal_access(&state, &headers, &peer)?;
    if let Some(session) = state.terminals.get(&body.id) {
        session.seen();
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TerminalRenameBody {
    id: String,
    name: String,
}

async fn rename_terminal(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Json(body): Json<TerminalRenameBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    terminal_access(&state, &headers, &peer)?;
    let session = state
        .terminals
        .get(&body.id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That terminal is closed"))?;
    session.rename(&body.name);
    Ok(Json(json!({ "terminal": session.info() })))
}

/// The library folder a terminal is in now, for showing it in the file browser.
async fn terminal_cwd(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Query(query): Query<TerminalIdQuery>,
) -> Result<impl IntoResponse, ApiError> {
    terminal_access(&state, &headers, &peer)?;
    let session = state
        .terminals
        .get(&query.id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That terminal is closed"))?;
    let root = state.root.clone();
    let (current, start) = tokio::task::spawn_blocking(move || {
        let current = session.current_dir();
        (current, session.cwd().to_path_buf())
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the folder"))?;
    let inside = |dir: &std::path::Path| {
        let root_canon = root.canonicalize().ok()?;
        let dir = dir.canonicalize().ok()?;
        if dir == root_canon {
            return Some(String::new());
        }
        library_path(&root, &dir, &dir.to_string_lossy())
    };
    match inside(&current) {
        Some(path) => Ok(Json(json!({ "path": path, "outside": false }))),
        None => Ok(Json(json!({
            "path": inside(&start).unwrap_or_default(),
            "outside": true,
            "current": current.display().to_string(),
        }))),
    }
}

#[derive(Deserialize)]
struct TerminalPathsBody {
    paths: Vec<String>,
}

/// Turns library paths (files dragged from the file list) into paths to type in a terminal:
/// relative to the folder the shell is in when they are inside it, absolute otherwise.
async fn terminal_paths(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Query(query): Query<TerminalIdQuery>,
    Json(body): Json<TerminalPathsBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    terminal_access(&state, &headers, &peer)?;
    let session = state
        .terminals
        .get(&query.id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That terminal is closed"))?;
    if body.paths.len() > 1000 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Too many files"));
    }
    let root = state.root.clone();
    let paths = tokio::task::spawn_blocking(move || -> Result<Vec<String>, FileError> {
        let here = session.current_dir().canonicalize().ok();
        body.paths
            .iter()
            .map(|rel| {
                let full = files::resolve(&root, rel)?.full;
                let relative = here.as_ref().and_then(|dir| full.strip_prefix(dir).ok());
                Ok(match relative {
                    Some(inside) if inside.as_os_str().is_empty() => ".".to_string(),
                    Some(inside) => inside.to_string_lossy().into_owned(),
                    None => full.to_string_lossy().into_owned(),
                })
            })
            .collect()
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not find the files"))??;
    Ok(Json(json!({ "paths": paths })))
}

#[derive(Deserialize)]
struct TerminalUploadQuery {
    id: String,
    name: Option<String>,
}

/// File name safe to type into any shell without quoting.
fn pasted_file_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let clean = clean.trim_matches('.').chars().take(120).collect::<String>();
    if clean.trim_matches('_').is_empty() { "pasted-file".to_string() } else { clean }
}

/// Folder, in the terminal's folder, that holds files pasted into it until the terminal closes.
const PASTED_DIR: &str = "pasted";

/// Stores a file pasted or dropped into a terminal and returns its path on the server.
async fn terminal_upload(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Query(query): Query<TerminalUploadQuery>,
    body: Body,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    terminal_access(&state, &headers, &peer)?;
    let session = state
        .terminals
        .get(&query.id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That terminal is closed"))?;
    // A `pasted` folder where the terminal is, so agents running there can find and read the
    // files: many only look inside their own folder.
    let located = {
        let root = state.root.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || {
            let root = root.canonicalize().ok()?;
            [session.current_dir(), session.cwd().to_path_buf()].into_iter().find_map(|candidate| {
                let dir = candidate.canonicalize().ok()?;
                let rel = dir.strip_prefix(&root).ok()?.to_string_lossy().replace('\\', "/");
                if files::is_under_trash(&rel) {
                    return None;
                }
                let rel = if rel.is_empty() { PASTED_DIR.to_string() } else { format!("{rel}/{PASTED_DIR}") };
                Some((dir.join(PASTED_DIR), rel))
            })
        })
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the pasted file"))?
    };
    // Outside the library (or in the Trash), fall back to the server's own folder.
    let (dir, library_dir) = match located {
        Some((dir, rel)) => (dir, Some(rel)),
        None => (state.terminal_uploads.join(&query.id), None),
    };
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the pasted file"))?;
    if library_dir.is_some() {
        // Keep pasted files out of `git status`, so agents do not commit them.
        let ignore = dir.join(".gitignore");
        if !tokio::fs::try_exists(&ignore).await.unwrap_or(true) {
            let _ = tokio::fs::write(&ignore, crate::terminal::PASTED_GITIGNORE).await;
        }
    }
    let name = pasted_file_name(query.name.as_deref().unwrap_or(""));
    let (stem, ext) = match name.rfind('.') {
        Some(index) if index > 0 => (&name[..index], &name[index..]),
        _ => (name.as_str(), ""),
    };
    let mut dest = dir.join(&name);
    let mut counter = 2;
    while tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        dest = dir.join(format!("{stem}-{counter}{ext}"));
        counter += 1;
    }
    const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    let stored = async {
        let mut out = tokio::fs::File::create(&dest).await?;
        let mut written = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
            let chunk = chunk.map_err(|err| io::Error::new(io::ErrorKind::ConnectionAborted, err))?;
            written += chunk.len() as u64;
            if written > MAX_BYTES {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "too large"));
            }
            tokio::io::AsyncWriteExt::write_all(&mut out, &chunk).await?;
        }
        tokio::io::AsyncWriteExt::flush(&mut out).await
    }
    .await;
    if let Err(err) = stored {
        let _ = tokio::fs::remove_file(&dest).await;
        return Err(match err.kind() {
            io::ErrorKind::InvalidData => ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "Files pasted into a terminal can be up to 2 GB"),
            io::ErrorKind::ConnectionAborted => ApiError::new(StatusCode::BAD_REQUEST, "The upload was interrupted"),
            _ => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the pasted file"),
        });
    }
    session.track_upload(&dest);
    if let Some(rel) = library_dir {
        invalidate_sizes(&state, &rel);
    }
    Ok(Json(json!({ "path": dest.display().to_string() })))
}

/// `git status` for the repository a terminal's folder belongs to.
async fn terminal_changes(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Query(query): Query<TerminalIdQuery>,
) -> Result<impl IntoResponse, ApiError> {
    terminal_access(&state, &headers, &peer)?;
    let session = state
        .terminals
        .get(&query.id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That terminal is closed"))?;
    let root = state.root.clone();
    // Where the terminal is now (after any `cd`), as Show folder does, not where it started.
    let changes = tokio::task::spawn_blocking(move || git_changes(&root, &session.current_dir()))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the changes"))?
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    Ok(Json(changes))
}

fn git_changes(root: &std::path::Path, cwd: &std::path::Path) -> Result<serde_json::Value, String> {
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|_| "Git is not installed on the server".to_string())
    };
    let top = git(cwd, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Ok(json!({ "repo": false }));
    }
    let top = std::path::PathBuf::from(String::from_utf8_lossy(&top.stdout).trim().to_string());
    let branch = git(&top, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let summary = git(&top, &["diff", "--shortstat", "HEAD"])
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|text| !text.is_empty());
    let status = git(&top, &["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    if !status.status.success() {
        return Err("Git could not read this repository".into());
    }
    // Lines added and removed per file, against the last commit (staged or not).
    let mut counts: std::collections::HashMap<String, (Option<u64>, Option<u64>)> = std::collections::HashMap::new();
    if let Ok(out) = git(&top, &["diff", "--numstat", "-z", "--no-renames", "HEAD"]) {
        if out.status.success() {
            for record in String::from_utf8_lossy(&out.stdout).split('\0').filter(|r| !r.is_empty()) {
                let mut fields = record.splitn(3, '\t');
                if let (Some(added), Some(deleted), Some(path)) = (fields.next(), fields.next(), fields.next()) {
                    // Binary files count as "-".
                    counts.insert(path.to_string(), (added.parse().ok(), deleted.parse().ok()));
                }
            }
        }
    }
    let raw = String::from_utf8_lossy(&status.stdout).to_string();
    let mut parts = raw.split('\0').filter(|part| !part.is_empty());
    let mut files = Vec::new();
    let mut total = 0usize;
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let code = entry[..2].trim().to_string();
        let path = entry[3..].to_string();
        if code.starts_with('R') || code.starts_with('C') {
            parts.next(); // the original name
        }
        total += 1;
        if files.len() < 300 {
            let full = top.join(&path);
            let (added, deleted) = match counts.get(&path) {
                Some(&(added, deleted)) => (added, deleted),
                // A new file: all its lines are added (if it is text and not huge).
                None if code == "??" => (
                    std::fs::metadata(&full)
                        .ok()
                        .filter(|meta| meta.is_file() && meta.len() <= 2 * 1024 * 1024)
                        .and_then(|_| std::fs::read(&full).ok())
                        .filter(|bytes| !bytes.contains(&0))
                        .map(|bytes| bytecount_lines(&bytes)),
                    Some(0),
                ),
                None => (None, None),
            };
            files.push(json!({
                "status": if code == "??" { "new".to_string() } else { code },
                "name": path,
                "path": library_path(root, &top, &full.to_string_lossy()),
                "added": added,
                "deleted": deleted,
            }));
        }
    }
    Ok(json!({
        "repo": true,
        "repoName": top.file_name().map(|name| name.to_string_lossy().into_owned()),
        "branch": branch,
        "summary": summary,
        "total": total,
        "files": files,
    }))
}

fn bytecount_lines(bytes: &[u8]) -> u64 {
    let breaks = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    breaks + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n"))
}

#[derive(Deserialize)]
struct TerminalDiffQuery {
    id: String,
    /// A path inside the repository, as the changes list gives it.
    file: String,
}

/// What changed in one file of the repository a terminal is in: `git diff` against the last
/// commit, or the whole file for one Git does not know yet.
async fn terminal_diff(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Query(query): Query<TerminalDiffQuery>,
) -> Result<impl IntoResponse, ApiError> {
    terminal_access(&state, &headers, &peer)?;
    let session = state
        .terminals
        .get(&query.id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That terminal is closed"))?;
    let diff = tokio::task::spawn_blocking(move || git_diff(&session.current_dir(), &query.file))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the changes"))?
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    Ok(Json(diff))
}

fn git_diff(cwd: &std::path::Path, file: &str) -> Result<serde_json::Value, String> {
    const LIMIT: usize = 512 * 1024;
    let inside = !file.is_empty() && std::path::Path::new(file).components().all(|part| matches!(part, std::path::Component::Normal(_)));
    if !inside {
        return Err("Choose a file in the repository".into());
    }
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|_| "Git is not installed on the server".to_string())
    };
    let top = git(cwd, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Err("The terminal is no longer in a Git repository".into());
    }
    let top = std::path::PathBuf::from(String::from_utf8_lossy(&top.stdout).trim().to_string());
    let mut out = git(&top, &["diff", "--no-color", "--no-ext-diff", "HEAD", "--", file])?.stdout;
    if out.is_empty() {
        // Not tracked yet (or no commit yet): the whole file, as added.
        out = git(&top, &["diff", "--no-color", "--no-ext-diff", "--no-index", "--", "/dev/null", file])?.stdout;
    }
    let truncated = out.len() > LIMIT;
    if truncated {
        let cut = out[..LIMIT].iter().rposition(|&b| b == b'\n').map_or(LIMIT, |at| at + 1);
        out.truncate(cut);
    }
    Ok(json!({ "diff": String::from_utf8_lossy(&out), "truncated": truncated }))
}

#[derive(Deserialize)]
struct IntegrationBody {
    agent: String,
    installed: bool,
}

async fn agent_integrations(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
) -> Result<impl IntoResponse, ApiError> {
    terminal_access(&state, &headers, &peer)?;
    let list = tokio::task::spawn_blocking(crate::agents::integrations)
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the agent settings"))?;
    Ok(Json(json!({ "integrations": list })))
}

async fn set_agent_integration(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    peer: Peer,
    Json(body): Json<IntegrationBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = terminal_access(&state, &headers, &peer)?;
    let exe = std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not find the OwnNAS program"))?;
    let agent = body.agent.clone();
    tokio::task::spawn_blocking(move || {
        if body.installed {
            crate::agents::install(&body.agent, &exe)
        } else {
            crate::agents::uninstall(&body.agent)
        }
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not change the agent settings"))?
    .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    record(&state, &user, if body.installed { "agent-integration-on" } else { "agent-integration-off" }, &agent);
    Ok(Json(json!({ "ok": true })))
}

/// Where a photo came from and the smaller copies made from it (by SHA-256), with where each copy
/// is now, for the preview.
fn versions_of(conn: &rusqlite::Connection, state: &AppState, path: &str) -> serde_json::Value {
    let Ok(hash) = conn.query_row("SELECT sha256 FROM file_hashes WHERE path = ?1", [path], |r| r.get::<_, String>(0)) else {
        return json!({ "earlier": [], "later": [] });
    };
    // Where a photo with that hash is now: where it was made, if it is still there, or else any
    // other file with the same contents.
    let here = |sha: &str, made_at: &str| -> Option<String> {
        let mut paths: Vec<String> = db::paths_with_hash(conn, sha).into_iter().map(|(p, _, _)| p).collect();
        paths.sort_by_key(|p| p != made_at);
        paths.into_iter().find(|p| !files::is_under_trash(p) && files::resolve(&state.root, p).is_ok_and(|f| f.full.is_file()))
    };
    let earlier: Vec<serde_json::Value> = db::earlier_versions(conn, &hash)
        .into_iter()
        .map(|v| {
            let now = here(&v.parent, &v.parent_path);
            json!({ "version": v, "parentNow": now })
        })
        .collect();
    let later: Vec<serde_json::Value> = db::later_versions(conn, &hash)
        .into_iter()
        .map(|v| {
            let now = here(&v.sha256, &v.path);
            json!({ "version": v, "now": now })
        })
        .collect();
    json!({ "sha256": hash, "earlier": earlier, "later": later })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResizeBody {
    paths: Vec<String>,
    /// A share of the size (1–99), or a box to fit in (either side alone is fine).
    percent: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default = "default_quality")]
    quality: u8,
    /// Put the smaller photo in place of the original (which goes to the Trash), instead of
    /// beside it. Albums, tags, and comments then stay with it.
    #[serde(default)]
    replace: bool,
}

fn default_quality() -> u8 {
    88
}

/// Makes photos smaller, and remembers which photo each one came from.
async fn resize_images(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ResizeBody>,
) -> Result<impl IntoResponse, ApiError> {
    use crate::image_edit::{self, Size};
    use sha2::Digest;
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    const ANY: u32 = 1_000_000;
    let size = match (body.percent, body.width, body.height) {
        (Some(percent), _, _) => Size::Percent(percent),
        (None, Some(width), height) => Size::Fit { width, height: height.unwrap_or(ANY) },
        (None, None, Some(height)) => Size::Fit { width: ANY, height },
        (None, None, None) => return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a size")),
    };
    if body.paths.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose photos to resize"));
    }
    let worker = state.clone();
    let (done, problems) = tokio::task::spawn_blocking(move || {
        let state = worker;
        let mut done = Vec::new();
        let mut problems = Vec::new();
        for raw in body.paths {
            let rel = rel_of(&Some(raw.clone()));
            let outcome = (|| -> Result<serde_json::Value, ApiError> {
                let found = files::resolve(&state.root, &rel)?;
                if files::is_under_trash(&found.rel) {
                    return Err(ApiError::new(StatusCode::BAD_REQUEST, "Restore it from the Trash first"));
                }
                let meta = std::fs::metadata(&found.full).map_err(|_| ApiError::from(FileError::NotFound))?;
                let name = found.full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if !meta.is_file() || !image_edit::resizable(&name) {
                    return Err(ApiError::new(StatusCode::BAD_REQUEST, "Only JPEG, PNG, WebP, BMP, and TIFF photos can be resized"));
                }
                let original = std::fs::read(&found.full).map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the photo"))?;
                let resized = image_edit::resize(&original, &name, size, body.quality).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
                let parent_sha = hex::encode(sha2::Sha256::digest(&original));
                let sha = hex::encode(sha2::Sha256::digest(&resized.bytes));
                let modified = meta.modified().ok();
                let millis = |time: Option<std::time::SystemTime>| time.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
                let dir = found.full.parent().ok_or_else(|| ApiError::from(FileError::NotFound))?.to_path_buf();
                // Written beside it first, so a full disk or a crash never leaves half a photo.
                let partial = dir.join(format!(".{name}.resize.part"));
                let write = || -> Result<(), ApiError> {
                    std::fs::write(&partial, &resized.bytes).map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the smaller photo"))?;
                    // The original's time, so it sorts with it (photos without a camera date).
                    if let (Some(time), Ok(file)) = (modified, std::fs::File::options().write(true).open(&partial)) {
                        let _ = file.set_modified(time);
                    }
                    Ok(())
                };
                write()?;
                let (dest, dest_rel, trashed) = if body.replace {
                    let trashed = match files::trash_entry(&state.root, &found.rel) {
                        Ok(path) => path,
                        Err(err) => {
                            let _ = std::fs::remove_file(&partial);
                            return Err(err.into());
                        }
                    };
                    (found.full.clone(), found.rel.clone(), Some(trashed))
                } else {
                    let dest = files::unique_path(dir.join(image_edit::copy_name(&name, resized.to)));
                    let dest_name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let parent = parent_rel_path(&found.rel);
                    (dest, if parent.is_empty() { dest_name } else { format!("{parent}/{dest_name}") }, None)
                };
                if std::fs::rename(&partial, &dest).is_err() {
                    let _ = std::fs::remove_file(&partial);
                    return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the smaller photo"));
                }
                let written = std::fs::metadata(&dest).ok();
                {
                    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
                    if let Some(trashed) = &trashed {
                        let _ = db::store_hash(&conn, trashed, meta.len(), millis(modified), &parent_sha);
                    } else {
                        let _ = db::store_hash(&conn, &found.rel, meta.len(), millis(modified), &parent_sha);
                    }
                    if let Some(written) = &written {
                        let _ = db::store_hash(&conn, &dest_rel, written.len(), millis(written.modified().ok()), &sha);
                    }
                    let version = db::ImageVersion {
                        sha256: sha.clone(),
                        parent: parent_sha.clone(),
                        operation: size.describe(),
                        width: Some(resized.to.0.into()),
                        height: Some(resized.to.1.into()),
                        parent_width: Some(resized.from.0.into()),
                        parent_height: Some(resized.from.1.into()),
                        path: dest_rel.clone(),
                        parent_path: found.rel.clone(),
                        created_at: db::now_secs(),
                        created_by: user.username.clone(),
                    };
                    db::record_version(&conn, &version).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
                }
                if let Some(trashed) = &trashed {
                    record(&state, &user, "trashed", &found.rel);
                    let _ = trashed;
                }
                record(&state, &user, "resize", &dest_rel);
                invalidate_sizes(&state, &parent_rel_path(&found.rel));
                Ok(json!({
                    "path": found.rel,
                    "saved": dest_rel,
                    "trashed": trashed,
                    "width": resized.to.0,
                    "height": resized.to.1,
                    "fromWidth": resized.from.0,
                    "fromHeight": resized.from.1,
                    "size": resized.bytes.len(),
                    "fromSize": meta.len(),
                    "sha256": sha,
                }))
            })();
            match outcome {
                Ok(item) => done.push(item),
                Err(err) => problems.push(json!({ "path": raw, "error": err.body.get("error").cloned().unwrap_or_default() })),
            }
        }
        (done, problems)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not resize the photos"))?;
    if !done.is_empty() {
        state.media.rescan_soon();
    }
    Ok(Json(json!({ "resized": done, "problems": problems })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertVideoBody {
    path: String,
    /// "same" keeps the format.
    #[serde(default = "same_format")]
    format: String,
    #[serde(default = "normal_speed")]
    speed: f64,
    height: Option<u32>,
    fps: Option<u32>,
    #[serde(default = "default_true")]
    audio: bool,
}

fn same_format() -> String {
    "same".into()
}

fn normal_speed() -> f64 {
    1.0
}

/// Starts converting a video (another format, smaller, faster or slower) into a new file beside
/// it; `/api/videos/job` follows it.
async fn convert_video(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ConvertVideoBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    if !state.ffmpeg {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Converting videos needs ffmpeg on the server"));
    }
    let plan = crate::video_edit::Plan { format: body.format.to_ascii_lowercase(), speed: body.speed, height: body.height, fps: body.fps, audio: body.audio };
    plan.check().map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let found = files::resolve(&state.root, &rel_of(&Some(body.path.clone())))?;
    let name = found.full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !found.full.is_file() || files::kind_of(&name) != "video" {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose a video"));
    }
    if files::is_under_trash(&found.rel) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Restore it from the Trash first"));
    }
    let (stem, original_ext) = name.rsplit_once('.').unwrap_or((&name, ""));
    let ext = plan.extension(original_ext);
    let tag = plan.tag();
    let wanted = if tag.is_empty() { format!("{stem}.{ext}") } else { format!("{stem} ({tag}).{ext}") };
    let dir = found.full.parent().ok_or_else(|| ApiError::from(FileError::NotFound))?;
    let output_full = files::unique_path(dir.join(wanted));
    let output_name = output_full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let parent = parent_rel_path(&found.rel);
    let output = if parent.is_empty() { output_name } else { format!("{parent}/{output_name}") };
    let operation = plan.describe(&ext);
    let worker = state.clone();
    let (source, made) = (found.rel.clone(), output.clone());
    let source_full = found.full.clone();
    let job = crate::video_edit::start(found.rel.clone(), found.full.clone(), output.clone(), output_full, plan, move |written| {
        // Which video this one came from, by SHA-256, as for resized photos.
        let hash = |path: &std::path::Path| -> Option<(String, u64, i64)> {
            use sha2::Digest;
            let meta = std::fs::metadata(path).ok()?;
            let mut file = std::fs::File::open(path).ok()?;
            let mut hasher = sha2::Sha256::new();
            std::io::copy(&mut file, &mut hasher).ok()?;
            let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
            Some((hex::encode(hasher.finalize()), meta.len(), mtime))
        };
        if let (Some((parent_sha, parent_size, parent_mtime)), Some((sha, size, mtime))) = (hash(&source_full), hash(written)) {
            let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
            let _ = db::store_hash(&conn, &source, parent_size, parent_mtime, &parent_sha);
            let _ = db::store_hash(&conn, &made, size, mtime, &sha);
            let _ = db::record_version(&conn, &db::ImageVersion {
                sha256: sha,
                parent: parent_sha,
                operation,
                width: None,
                height: None,
                parent_width: None,
                parent_height: None,
                path: made.clone(),
                parent_path: source.clone(),
                created_at: db::now_secs(),
                created_by: user.username.clone(),
            });
        }
        record(&worker, &user, "convert-video", &format!("{source} → {made}"));
        invalidate_sizes(&worker, &parent_rel_path(&made));
        worker.media.rescan_soon();
    });
    Ok(Json(json!({ "job": job.status() })))
}

#[derive(Deserialize)]
struct JobQuery {
    id: String,
}

async fn video_job(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap, Query(query): Query<JobQuery>) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let job = crate::video_edit::job(&query.id).ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "That conversion is over"))?;
    Ok(Json(json!({ "job": job.status() })))
}

async fn cancel_video_job(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap, Query(query): Query<JobQuery>) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    require_user(&state, &headers)?;
    if let Some(job) = crate::video_edit::job(&query.id) {
        job.cancel();
    }
    Ok(Json(json!({ "ok": true })))
}

/// The library's disk (size, used, free) and what the library, its Trash, and OwnNAS's own data
/// take. Starts measuring again when the last measurement is old.
async fn storage_status(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let root = state.root.clone();
    let disk = tokio::task::spawn_blocking(move || crate::storage::disk(&root)).await.ok().flatten();
    if state.storage.stale() {
        state.storage.measure(state.root.clone(), state.db_path.clone());
    }
    Ok(Json(storage_json(&state, disk)))
}

fn storage_json(state: &AppState, disk: Option<crate::storage::Disk>) -> serde_json::Value {
    json!({
        "disk": disk,
        "library": state.storage.last(),
        "measuring": state.storage.running(),
        "measuredSoFar": state.storage.running().then(|| state.storage.seen()),
    })
}

/// Measures the library again now.
async fn measure_storage(State(state): State<std::sync::Arc<AppState>>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    require_user(&state, &headers)?;
    state.storage.measure(state.root.clone(), state.db_path.clone());
    let root = state.root.clone();
    let disk = tokio::task::spawn_blocking(move || crate::storage::disk(&root)).await.ok().flatten();
    Ok(Json(storage_json(&state, disk)))
}

fn record(state: &AppState, user: &User, action: &str, detail: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::log_event(&conn, Some(user.id), &user.username, action, detail);
}

fn rewrite_meta(state: &AppState, from: &str, to: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::rewrite_path_meta(&conn, from, to);
    state.media.descriptions.invalidate();
}

fn delete_meta(state: &AppState, path: &str) {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let _ = db::delete_path_meta(&conn, path);
    state.media.descriptions.invalidate();
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
struct CompressBody {
    paths: Vec<String>,
    format: String,
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

    let mut hits = if tag_only {
        Vec::new()
    } else {
        let root = root.clone();
        let rel = rel.clone();
        let needle = needle.clone();
        let ocr = state.ocr;
        let cache_dir = state.text_cache.clone();
        tokio::task::spawn_blocking(move || {
            files::search(
                &root,
                &rel,
                &needle,
                &files::SearchOptions { ocr, cache_dir },
            )
        })
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
                let Ok(meta) = resolved.full.metadata() else { continue };
                out.push(files::SearchHit {
                    kind: if dir {
                        "folder".to_string()
                    } else {
                        files::kind_of(&name).to_string()
                    },
                    name,
                    path,
                    dir,
                    size: if dir { 0 } else { meta.len() },
                    modified: meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0),
                    matched_tag: Some(tag),
                    matched_content: None,
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

    let mut payload = json!({ "hits": hits });
    decorate_folder_appearance(&state, &mut payload["hits"])?;
    Ok(Json(payload))
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
    let cancel = files::CancelFlag::new();
    let _guard = CancelOnDrop(cancel.clone());
    let usage = tokio::task::spawn_blocking({
        let root = root.clone();
        let rel = rel.clone();
        move || {
            if slow {
                files::usage_paced(&root, &rel, true, &cancel)
            } else {
                files::usage_paced(&root, &rel, false, &cancel)
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

async fn undo_move_item(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<MoveBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let root = state.root.clone();
    let from = body.path.clone();
    let to = tokio::task::spawn_blocking(move || files::move_exact(&root, &body.path, &body.dest))
        .await.map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not undo the move"))?
        .map_err(ApiError::from)?;
    rewrite_meta(&state, &from, &to);
    record(&state, &user, "undo", &from);
    Ok(Json(json!({"path": to})))
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

fn decorate_folder_appearance(state: &AppState, entries: &mut serde_json::Value) -> Result<(), ApiError> {
    let Some(entries) = entries.as_array_mut() else { return Ok(()); };
    let paths: Vec<String> = entries.iter().filter(|entry| entry["dir"] == true).filter_map(|entry| entry["path"].as_str().map(String::from)).collect();
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let appearances = db::folder_appearance_for_paths(&conn, &paths).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    for entry in entries {
        if let Some(appearance) = entry["path"].as_str().and_then(|path| appearances.get(path)) {
            entry["folderAppearance"] = json!(appearance);
        }
    }
    Ok(())
}

async fn get_folder_appearance(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let folder = files::resolve(&state.root, &rel_of(&query.path))?;
    if !folder.full.is_dir() { return Err(FileError::NotADirectory.into()); }
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let appearance = db::folder_appearance_for_paths(&conn, &[folder.rel.clone()])
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(Json(json!(appearance.get(&folder.rel))))
}

#[derive(Deserialize)]
struct FolderAppearanceBody { paths: Vec<String>, color: String, icon: String }

async fn set_folder_appearance(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<FolderAppearanceBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    if body.paths.is_empty() || body.paths.len() > 200 { return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose between 1 and 200 folders")); }
    let mut paths = Vec::new();
    for path in &body.paths {
        let folder = files::resolve(&state.root, &path)?;
        if folder.rel.is_empty() || folder.rel == files::TRASH_DIR || folder.rel.starts_with(&format!("{}/", files::TRASH_DIR)) || folder.rel == "Trash" || folder.rel.starts_with("Trash/") || folder.rel == ".ownnas-archive" || folder.rel == "Archive" {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose an ordinary folder outside Trash"));
        }
        if !folder.full.is_dir() { return Err(FileError::NotADirectory.into()); }
        paths.push(folder.rel);
    }
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    db::set_folder_appearance(&conn, &paths, &body.color, &body.icon)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    drop(conn);
    record(&state, &user, "folder-appearance", &paths.join(", "));
    Ok(Json(json!({ "ok": true })))
}

/// Everything known about one photo or video (see `photo_details`).
async fn photo_details(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_user(&state, &headers)?;
    let rel = rel_of(&query.path);
    let resolved = files::resolve(&state.root, &rel).map_err(ApiError::from)?;
    if !resolved.full.is_file() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "That photo is not there any more"));
    }
    let payload = tokio::task::spawn_blocking(move || {
        crate::photo_details::details(&state.db, &resolved.rel, &resolved.full)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the photo"))?;
    Ok(Json(payload))
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
    let mut notes = serde_json::to_value(notes).unwrap_or_default();
    // The albums this file is in, for the preview.
    notes["albums"] = json!(crate::albums::albums_for_path(&conn, &path)?
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect::<Vec<_>>());
    notes["versions"] = versions_of(&conn, &state, &path);
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

async fn bulk_tags(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BulkTagsBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    let tag = body.tag.trim();
    if tag.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Enter a tag"));
    }
    if body.paths.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Select at least one item"));
    }
    let remove = matches!(body.action.as_deref(), Some("remove"));
    let mut applied = 0usize;
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        for raw in &body.paths {
            let path = raw.trim().trim_start_matches('/').to_string();
            if path.is_empty() {
                continue;
            }
            if remove {
                db::remove_tag(&conn, &path, tag)
                    .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
            } else {
                let _ = db::add_tag(&conn, &path, tag)
                    .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
                if let Ok((size, modified)) = files::entry_stats(&state.root, &path) {
                    let _ = db::touch_fingerprint(&conn, &path, size as i64, modified);
                }
            }
            applied += 1;
        }
    }
    let action = if remove { "untag-bulk" } else { "tag-bulk" };
    record(
        &state,
        &user,
        action,
        &format!("{applied} items #{tag}"),
    );
    Ok(Json(json!({ "ok": true, "count": applied })))
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
    let cancel = files::CancelFlag::new();
    let _guard = CancelOnDrop(cancel.clone());
    let name = tokio::task::spawn_blocking(move || files::write_zip(&root, &packed, &output, &cancel))
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
    let cancel = files::CancelFlag::new();
    let _guard = CancelOnDrop(cancel.clone());
    let name = tokio::task::spawn_blocking(move || files::write_zip_selection(&root, &paths, &output, &cancel))
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

async fn compress_entries(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CompressBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let (user, _) = require_user(&state, &headers)?;
    require_write(&state)?;
    if body.paths.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Choose at least one item"));
    }
    let root = state.root.clone();
    let paths = body.paths.clone();
    let format = body.format.clone();
    let cancel = files::CancelFlag::new();
    let _guard = CancelOnDrop(cancel.clone());
    let created = tokio::task::spawn_blocking(move || {
        files::compress_selection(&root, &paths, &format, &cancel)
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Could not create the archive"))?
    .map_err(ApiError::from)?;
    invalidate_sizes(&state, &parent_rel_path(&created));
    record(
        &state,
        &user,
        "compress",
        &format!("{} → {created}", body.format),
    );
    Ok(Json(json!({ "ok": true, "path": created })))
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
mod backup_name_tests {
    #[test]
    fn months_and_names() {
        assert_eq!(super::year_month(0), (1970, 1));
        assert_eq!(super::year_month(1_700_000_000), (2023, 11));
        assert_eq!(super::year_month(951_825_600), (2000, 2), "29 Feb 2000");
        assert_eq!(super::safe_name("../Pixel: 8?", "Phone"), "_Pixel_ 8_");
        assert_eq!(super::safe_name("  ", "Phone"), "Phone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_copies_are_the_name_or_a_numbered_one() {
        assert!(same_or_numbered("IMG_1.jpg", "IMG_1.jpg"));
        assert!(same_or_numbered("IMG_1 (2).jpg", "IMG_1.jpg"));
        assert!(same_or_numbered("IMG_1 (12).jpg", "IMG_1.jpg"));
        assert!(!same_or_numbered("IMG_1 ().jpg", "IMG_1.jpg"));
        assert!(!same_or_numbered("IMG_1 (2).png", "IMG_1.jpg"));
        assert!(!same_or_numbered("IMG_12.jpg", "IMG_1.jpg"));
        assert!(!same_or_numbered("IMG_1 (x).jpg", "IMG_1.jpg"));
        assert!(same_or_numbered("notes (3)", "notes"));
    }
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn scratch() -> (PathBuf, PathBuf) {
        // The clock alone can repeat between tests running side by side (macOS has microseconds).
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "ownnas-http-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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
        let state = Arc::new(new_state(
            root.clone(),
            data,
            false,
            false,
            false,
            false,
            crate::update::UpdateConfig::default(),
        ).unwrap());
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
    async fn uploads_ask_on_conflict_and_keep_the_original_time() {
        let (app, root) = app().await;
        let login = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header("content-type", "application/json")
            .header("x-ownnas", "1")
            .body(Body::from(r#"{"username":"ada","password":"longenough"}"#))
            .unwrap();
        let response = app.clone().oneshot(login).await.unwrap();
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
        let upload = |query: &str, bytes: &str| {
            let body = format!(
                "--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"photo.jpg\"\r\nContent-Type: application/octet-stream\r\n\r\n{bytes}\r\n--B--\r\n"
            );
            Request::builder()
                .method("POST")
                .uri(format!("/api/upload?path=&{query}"))
                .header("cookie", &cookie)
                .header("x-ownnas", "1")
                .header("content-type", "multipart/form-data; boundary=B")
                .body(Body::from(body))
                .unwrap()
        };
        let original = 1_600_000_000_123i64;
        let response = app.clone().oneshot(upload(&format!("mtime={original}"), "first")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let saved = std::fs::metadata(root.join("photo.jpg")).unwrap().modified().unwrap();
        assert_eq!(saved.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64, original);

        // A taken name is a question for the client, which then picks one of the web app's choices.
        let response = app.clone().oneshot(upload("", "second")).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = app.clone().oneshot(upload("conflict=keep", "second")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(std::fs::read_to_string(root.join("photo (2).jpg")).unwrap(), "second");
        assert_eq!(std::fs::read_to_string(root.join("photo.jpg")).unwrap(), "first");
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

        files::make_dir(&root, "", "Projects").unwrap();
        let appearance = json!({"paths":["Projects"], "color":"#ABCDEF", "icon":"work"}).to_string();
        let response = app.clone().oneshot(Request::builder().method("POST")
            .uri("/api/folder-appearance").header("cookie", &cookie).header("content-type", "application/json")
            .body(Body::from(appearance.clone())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = app.clone().oneshot(Request::builder().method("POST")
            .uri("/api/folder-appearance").header("cookie", &cookie).header("x-ownnas", "1")
            .header("content-type", "application/json").body(Body::from(appearance)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app.clone().oneshot(Request::builder().uri("/api/list").header("cookie", &cookie)
            .body(Body::empty()).unwrap()).await.unwrap();
        let listing: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
        let folder = listing["entries"].as_array().unwrap().iter().find(|entry| entry["name"] == "Projects").unwrap();
        assert_eq!(folder["folderAppearance"]["color"], "#abcdef");
        assert_eq!(folder["folderAppearance"]["icon"], "work");
        for (paths, color, icon, status) in [
            (vec!["Projects"], "invalid", "work", StatusCode::BAD_REQUEST),
            (vec!["Projects"], "", "<script>", StatusCode::BAD_REQUEST),
            (vec!["hello.txt"], "", "folder", StatusCode::BAD_REQUEST),
            (vec!["../outside"], "", "folder", StatusCode::FORBIDDEN),
            (vec![""], "", "folder", StatusCode::BAD_REQUEST),
            (vec!["Projects", "missing"], "#123456", "folder", StatusCode::NOT_FOUND),
        ] {
            let response = app.clone().oneshot(Request::builder().method("POST")
                .uri("/api/folder-appearance").header("cookie", &cookie).header("x-ownnas", "1")
                .header("content-type", "application/json")
                .body(Body::from(json!({"paths":paths,"color":color,"icon":icon}).to_string())).unwrap()).await.unwrap();
            assert_eq!(response.status(), status);
        }
        let response = app.clone().oneshot(Request::builder().uri("/api/folder-appearance?path=Projects")
            .header("cookie", &cookie).body(Body::empty()).unwrap()).await.unwrap();
        let appearance: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert_eq!(appearance["color"], "#abcdef", "invalid batch must leave all folders untouched");

        let response = app.clone().oneshot(Request::builder().uri("/api/search?q=Projects")
            .header("cookie", &cookie).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let search: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert_eq!(search["hits"][0]["folderAppearance"]["icon"], "work");

        let readonly_app = router(Arc::new(new_state(root.clone(), root.parent().unwrap().join("data"), true, false, false, false, crate::update::UpdateConfig::default()).unwrap()));
        let response = readonly_app.oneshot(Request::builder().method("POST")
            .uri("/api/folder-appearance").header("cookie", &cookie).header("x-ownnas", "1")
            .header("content-type", "application/json")
            .body(Body::from(json!({"paths":["Projects"],"color":"","icon":"folder"}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        files::create_file(&root, "", "Document", "docx").unwrap();
        let response = app.clone().oneshot(Request::builder()
            .uri("/api/docx?path=Document.docx").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app.clone().oneshot(Request::builder()
            .uri("/api/docx?path=Document.docx").header("cookie", &cookie)
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let hash = response.headers().get("x-docx-hash").unwrap().to_str().unwrap().to_string();
        let bytes = body_bytes(response).await;
        // Blank document compressed bytes remain unchanged when saved back.
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::new();
        for chunk in bytes.chunks(3) {
            let value = ((chunk[0] as u32) << 16) | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8) | chunk.get(2).copied().unwrap_or(0) as u32;
            encoded.push(alphabet[((value >> 18) & 63) as usize] as char);
            encoded.push(alphabet[((value >> 12) & 63) as usize] as char);
            encoded.push(if chunk.len() > 1 { alphabet[((value >> 6) & 63) as usize] as char } else { '=' });
            encoded.push(if chunk.len() > 2 { alphabet[(value & 63) as usize] as char } else { '=' });
        }
        let payload = json!({"path": "Document.docx", "data": encoded, "expected_hash": hash}).to_string();
        let response = app.clone().oneshot(Request::builder().method("POST")
            .uri("/api/write-docx").header("cookie", &cookie).header("content-type", "application/json")
            .body(Body::from(payload.clone())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = app.clone().oneshot(Request::builder().method("POST")
            .uri("/api/write-docx").header("cookie", &cookie).header("x-ownnas", "1")
            .header("content-type", "application/json").body(Body::from(payload)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(std::fs::read(root.join("Document.docx")).unwrap(), bytes);

        files::create_file(&root, "", "Notes", "odt").unwrap();
        let response = app.clone().oneshot(Request::builder()
            .uri("/api/odt?path=Notes.odt").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app.clone().oneshot(Request::builder()
            .uri("/api/odt?path=Notes.odt").header("cookie", &cookie)
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-odt-hash").is_some());
        assert!(String::from_utf8_lossy(&body_bytes(response).await).starts_with("PK"));

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
