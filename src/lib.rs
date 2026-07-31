//! `rustyfile` is a deliberately small filesystem stored inside one ordinary file.
//!
//! The public API is intentionally compact. Most learners will want to start with
//! [`layout`], then read [`FileSystem::format`] and the operations in `filesystem.rs`.

pub mod layout;

mod filesystem;

pub use filesystem::{DirEntryInfo, FileSystem, FsError, FsInfo, InodeInfo, Result};
