//! JNI bridge for the Android host application.

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

use jni::objects::{JClass, JString};
use jni::sys::jint;
use jni::JNIEnv;
use tokio::sync::oneshot;

use crate::{auth, content_index, db, files, http, thumbs, update};

struct ServerHandle {
    stop: oneshot::Sender<()>,
    port: u16,
}

static SERVER: OnceLock<Mutex<Option<ServerHandle>>> = OnceLock::new();

fn server_slot() -> &'static Mutex<Option<ServerHandle>> {
    SERVER.get_or_init(|| Mutex::new(None))
}

fn start_server(
    root: String,
    data: String,
    username: String,
    password: String,
) -> Result<u16, String> {
    let mut slot = server_slot().lock().unwrap_or_else(|err| err.into_inner());
    if let Some(server) = slot.as_ref() {
        return Ok(server.port);
    }

    let root = PathBuf::from(root.trim());
    let data = PathBuf::from(data.trim());
    let (root, data) = files::prepare_paths(&root, &data)?;
    auth::validate_username(&username).map_err(|err| err.to_string())?;
    auth::validate_password(&password).map_err(|err| err.to_string())?;

    let state = Arc::new(http::new_state(
        root,
        data,
        false,
        false,
        false,
        false,
        update::UpdateConfig::default(),
    )?);
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        if db::count_users(&conn)? == 0 {
            db::create_user(&conn, &username, &password)?;
        }
    }

    let (stop, stop_receiver) = oneshot::channel();
    let (ready_sender, ready_receiver) = mpsc::channel();
    let thread_state = state;
    std::thread::Builder::new()
        .name("ownnas-android-server".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready_sender
                        .send(Err(format!("Could not start the async runtime: {error}")));
                    return;
                }
            };
            let shutdown = async move {
                let _ = stop_receiver.await;
            };
            let _ = runtime.block_on(http::serve_with_shutdown(
                thread_state,
                "127.0.0.1:8787",
                false,
                Some(ready_sender),
                shutdown,
            ));
        })
        .map_err(|error| format!("Could not start the OwnNAS server thread: {error}"))?;

    let bound = ready_receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "Timed out waiting for the OwnNAS server to start".to_string())??;
    let port = bound.port();
    *slot = Some(ServerHandle { stop, port });
    Ok(port)
}

fn stop_server() {
    let mut slot = server_slot().lock().unwrap_or_else(|err| err.into_inner());
    if let Some(server) = slot.take() {
        let _ = server.stop.send(());
    }
}

fn jstring_to_string(env: &mut JNIEnv<'_>, value: JString<'_>) -> Result<String, String> {
    env.get_string(&value)
        .map(|value| value.to_string_lossy().into_owned())
        .map_err(|_| "Could not read an Android string".to_string())
}

fn throw_error(env: &mut JNIEnv<'_>, message: String) -> jint {
    let _ = env.throw_new("java/lang/IllegalStateException", message);
    0
}

#[no_mangle]
pub extern "system" fn Java_com_ownnas_MainActivity_startOwnnas(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    root: JString<'_>,
    data: JString<'_>,
    username: JString<'_>,
    password: JString<'_>,
) -> jint {
    let values = (|| {
        Ok::<_, String>((
            jstring_to_string(&mut env, root)?,
            jstring_to_string(&mut env, data)?,
            jstring_to_string(&mut env, username)?,
            jstring_to_string(&mut env, password)?,
        ))
    })();
    let (root, data, username, password) = match values {
        Ok(values) => values,
        Err(error) => return throw_error(&mut env, error),
    };
    match start_server(root, data, username, password) {
        Ok(port) => port as jint,
        Err(error) => throw_error(&mut env, error),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_ownnas_MainActivity_stopOwnnas(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    stop_server();
}

// Keep these imports linked into Android builds even when capability probes are
// optimized away on a device without ffmpeg/tesseract.
#[allow(dead_code)]
fn _android_capabilities_are_intentionally_disabled() {
    let _ = (content_index::tesseract_available, thumbs::ffmpeg_available);
}
