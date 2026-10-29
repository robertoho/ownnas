//! Reusable OwnNAS server library.
//!
//! The desktop CLI remains in `main.rs`. This library exposes the same Rust
//! modules for tests and provides the Android JNI entry points when compiled
//! for Android.

pub mod agents;
pub mod archives;
pub mod audio_info;
pub mod albums;
pub mod workspaces;
pub mod auth;
pub mod books;
pub mod changes;
pub mod content_index;
pub mod data_tables;
pub mod db;
pub mod db_watch;
pub mod documents;
pub mod ebook;
pub mod file_index;
pub mod files;
pub mod font_info;
pub mod helper_tools;
pub mod text_index;
pub mod versions;
pub mod duplicates;
pub mod video_meta;
pub mod health;
pub mod image_edit;
pub mod video_edit;
pub mod media_index;
pub mod media_stream;
pub mod smart;
#[cfg(ownnas_ml)]
pub mod ml;
pub mod http;
pub mod mail;
pub mod music;
pub mod nas_backup;
pub mod map_tiles;
pub mod pdf_ops;
pub mod photo_details;
pub mod photo_formats;
pub mod terminal;
pub mod preview;
pub mod sqlite_view;
pub mod storage;
pub mod thumbs;
pub mod tls;
pub mod update;

#[cfg(target_os = "android")]
mod android;
