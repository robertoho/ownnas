//! Reusable OwnNAS server library.
//!
//! The desktop CLI remains in `main.rs`. This library exposes the same Rust
//! modules for tests and provides the Android JNI entry points when compiled
//! for Android.

pub mod auth;
pub mod content_index;
pub mod db;
pub mod files;
pub mod http;
pub mod pdf_ops;
pub mod preview;
pub mod thumbs;
pub mod tls;
pub mod update;

#[cfg(target_os = "android")]
mod android;
