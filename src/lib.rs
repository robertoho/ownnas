//! Reusable OwnNAS server library.
//!
//! The desktop CLI remains in `main.rs`. This library exposes the same Rust
//! modules for tests and provides the Android JNI entry points when compiled
//! for Android.

pub mod agents;
pub mod albums;
pub mod auth;
pub mod content_index;
pub mod db;
pub mod files;
pub mod media_index;
pub mod smart;
#[cfg(not(target_os = "android"))]
pub mod ml;
pub mod http;
pub mod pdf_ops;
pub mod terminal;
pub mod preview;
pub mod sqlite_view;
pub mod thumbs;
pub mod tls;
pub mod update;

#[cfg(target_os = "android")]
mod android;
