//! Errors shared by every filesystem layer.

use crate::layout::MAX_NAME_LEN;
use std::fmt;

pub type Result<T> = std::result::Result<T, FsError>;

/// Every expected failure from parsing or operating on an image.
#[derive(Debug)]
pub enum FsError {
    Io(std::io::Error),
    InvalidImage(String),
    Corrupt(String),
    NotFound(String),
    AlreadyExists(String),
    NotDirectory(String),
    IsDirectory(String),
    DirectoryNotEmpty(String),
    NoSpace,
    NameTooLong(String),
    InvalidPath(String),
    FileTooLarge { size: usize, maximum: usize },
}

impl fmt::Display for FsError {
    /// Turn structured failures into concise CLI messages.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::InvalidImage(message) => write!(f, "invalid filesystem image: {message}"),
            Self::Corrupt(message) => write!(f, "filesystem is corrupt: {message}"),
            Self::NotFound(path) => write!(f, "not found: {path}"),
            Self::AlreadyExists(path) => write!(f, "already exists: {path}"),
            Self::NotDirectory(path) => write!(f, "not a directory: {path}"),
            Self::IsDirectory(path) => write!(f, "is a directory: {path}"),
            Self::DirectoryNotEmpty(path) => write!(f, "directory is not empty: {path}"),
            Self::NoSpace => write!(f, "filesystem has no free space"),
            Self::NameTooLong(name) => {
                write!(f, "name is longer than {MAX_NAME_LEN} bytes: {name}")
            }
            Self::InvalidPath(path) => write!(f, "invalid path: {path}"),
            Self::FileTooLarge { size, maximum } => {
                write!(f, "file is {size} bytes; maximum is {maximum} bytes")
            }
        }
    }
}

impl std::error::Error for FsError {}

impl From<std::io::Error> for FsError {
    /// Let `?` convert host I/O failures into filesystem failures.
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
