//! `ownnas-cloud-agent`: keeps the OwnNAS places in Explorer in sync, in the background (no
//! console window). Started at sign-in to Windows by `ownnas-cloud setup`. What it does goes to
//! `%LOCALAPPDATA%\OwnNAS\cloud.log`.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    if let Err(err) = ownnas_cloud::sync::run_all() {
        ownnas_cloud::log(&format!("stopped: {err}"));
    }
    #[cfg(not(windows))]
    eprintln!("ownnas-cloud-agent works on Windows.");
}
