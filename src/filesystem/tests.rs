//! Behavior tests for the filesystem layers working together.

use super::*;
use crate::layout::{BLOCK_SIZE, MAX_FILE_SIZE};
use std::time::{SystemTime, UNIX_EPOCH};

/// Give each test an isolated image path.
fn image_path(name: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("rustyfile-{name}-{unique}.img"))
}

/// Data and directory entries survive closing and reopening the image.
#[test]
fn format_and_reopen() {
    let path = image_path("reopen");
    {
        let mut fs = FileSystem::format(&path, Some(2 * 1024 * 1024)).unwrap();
        fs.create_dir(ROOT_INODE, "/notes").unwrap();
        fs.write_file(ROOT_INODE, "/notes/hello.txt", b"hello filesystem")
            .unwrap();
    }
    {
        let mut fs = FileSystem::open(&path).unwrap();
        assert_eq!(
            fs.read_file(ROOT_INODE, "/notes/hello.txt").unwrap(),
            b"hello filesystem"
        );
    }
    std::fs::remove_file(path).unwrap();
}

/// Directory `..` entries support relative traversal.
#[test]
fn relative_paths_and_parent_entries_work() {
    let path = image_path("paths");
    let mut fs = FileSystem::format(&path, Some(2 * 1024 * 1024)).unwrap();
    let a = fs.create_dir(ROOT_INODE, "a").unwrap();
    let b = fs.create_dir(a, "b").unwrap();
    fs.write_file(b, "../answer", b"42").unwrap();
    assert_eq!(fs.read_file(ROOT_INODE, "/a/answer").unwrap(), b"42");
    assert_eq!(fs.resolve_path(b, "../..").unwrap(), ROOT_INODE);
    drop(fs);
    std::fs::remove_file(path).unwrap();
}

/// Removing a file returns both allocation kinds to their prior counts.
#[test]
fn remove_reclaims_blocks_and_inodes() {
    let path = image_path("remove");
    let mut fs = FileSystem::format(&path, Some(2 * 1024 * 1024)).unwrap();
    let before = fs.info().unwrap();
    fs.write_file(ROOT_INODE, "temporary", &vec![7; BLOCK_SIZE + 10])
        .unwrap();
    fs.remove_file(ROOT_INODE, "temporary").unwrap();
    let after = fs.info().unwrap();
    assert_eq!(before.used_blocks, after.used_blocks);
    assert_eq!(before.used_inodes, after.used_inodes);
    drop(fs);
    std::fs::remove_file(path).unwrap();
}

/// `rmdir` protects directories that still contain user entries.
#[test]
fn nonempty_directory_cannot_be_removed() {
    let path = image_path("rmdir");
    let mut fs = FileSystem::format(&path, Some(2 * 1024 * 1024)).unwrap();
    fs.create_dir(ROOT_INODE, "docs").unwrap();
    fs.create_file(ROOT_INODE, "docs/readme").unwrap();
    assert!(matches!(
        fs.remove_dir(ROOT_INODE, "docs"),
        Err(FsError::DirectoryNotEmpty(_))
    ));
    drop(fs);
    std::fs::remove_file(path).unwrap();
}

/// Direct pointers impose the documented per-file limit.
#[test]
fn maximum_file_size_is_enforced() {
    let path = image_path("large");
    let mut fs = FileSystem::format(&path, Some(2 * 1024 * 1024)).unwrap();
    assert!(matches!(
        fs.write_file(ROOT_INODE, "huge", &vec![0; MAX_FILE_SIZE + 1]),
        Err(FsError::FileTooLarge { .. })
    ));
    drop(fs);
    std::fs::remove_file(path).unwrap();
}

/// Reusing directory blocks lets deletion recover a completely full image.
#[test]
fn deletion_still_works_when_every_block_is_used() {
    let path = image_path("full-delete");
    // Metadata uses blocks 0..=10, root uses 11, and the file uses 12.
    let mut fs = FileSystem::format(&path, Some(13 * BLOCK_SIZE as u64)).unwrap();
    fs.write_file(ROOT_INODE, "last-block", b"x").unwrap();
    let full = fs.info().unwrap();
    assert_eq!(full.used_blocks, full.total_blocks);

    fs.remove_file(ROOT_INODE, "last-block").unwrap();
    let recovered = fs.info().unwrap();
    assert_eq!(recovered.used_blocks + 1, recovered.total_blocks);
    assert!(matches!(
        fs.read_file(ROOT_INODE, "last-block"),
        Err(FsError::NotFound(_))
    ));
    drop(fs);
    std::fs::remove_file(path).unwrap();
}
