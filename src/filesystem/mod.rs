//! Filesystem implementation, split by responsibility.
//!
//! Read the modules in this order:
//! 1. [`disk`] moves bytes and manages allocation.
//! 2. [`directory`] maps path names to inode numbers.
//! 3. [`file`] exposes regular-file operations.

mod directory;
mod disk;
mod error;
mod file;

#[cfg(test)]
mod tests;

pub use error::{FsError, Result};

use crate::layout::{
    DirEntry, FileKind, Inode, Superblock, BLOCK_BITMAP_BLOCK, BLOCK_SIZE, DATA_BLOCK_START,
    INODE_BITMAP_BLOCK, INODE_COUNT, MAX_IMAGE_SIZE, ROOT_INODE,
};
use disk::{count_set_bits, set_bit};
use std::fs::{File, OpenOptions};
use std::path::Path;

/// One entry returned by a directory listing.
#[derive(Clone, Debug)]
pub struct DirEntryInfo {
    pub inode: u32,
    pub kind: FileKind,
    pub name: String,
    pub size: u64,
}

/// The metadata shown by `stat`.
#[derive(Clone, Debug)]
pub struct InodeInfo {
    pub inode: u32,
    pub kind: FileKind,
    pub size: u64,
    pub blocks: usize,
}

/// Allocation totals shown by `info`.
#[derive(Clone, Debug)]
pub struct FsInfo {
    pub total_blocks: u32,
    pub used_blocks: u32,
    pub total_inodes: u32,
    pub used_inodes: u32,
}

/// An opened Rustyfile image.
///
/// The image is accessed directly: there is no cache or journal between an
/// operation and its disk writes.
pub struct FileSystem {
    pub(super) file: File,
    pub(super) superblock: Superblock,
}

impl FileSystem {
    /// Format an image, optionally creating or resizing it first.
    pub fn format(path: impl AsRef<Path>, size: Option<u64>) -> Result<Self> {
        let path = path.as_ref();
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        if size.is_some() {
            options.create(true);
        }
        let file = options.open(path)?;

        // A supplied size turns the host file into our block device.
        if let Some(size) = size {
            file.set_len(size)?;
        }
        let image_size = file.metadata()?.len();
        Self::validate_image_size(image_size)?;

        let superblock = Superblock {
            total_blocks: (image_size / BLOCK_SIZE as u64) as u32,
        };
        let mut fs = Self { file, superblock };

        // Reset metadata; unreachable old data blocks need not be erased.
        for block in 0..DATA_BLOCK_START {
            fs.write_block(block, &[0; BLOCK_SIZE])?;
        }
        fs.write_block(0, &fs.superblock.encode())?;

        // Metadata blocks always own their corresponding block bits.
        let mut block_bitmap = [0; BLOCK_SIZE];
        for block in 0..DATA_BLOCK_START {
            set_bit(&mut block_bitmap, block, true);
        }
        fs.write_block(BLOCK_BITMAP_BLOCK, &block_bitmap)?;

        // Inode zero is permanently the root directory.
        let mut inode_bitmap = [0; BLOCK_SIZE];
        set_bit(&mut inode_bitmap, ROOT_INODE, true);
        fs.write_block(INODE_BITMAP_BLOCK, &inode_bitmap)?;

        // Root starts with the same `.` and `..` entries as any directory.
        let mut root = Inode::empty(FileKind::Directory);
        fs.write_inode(ROOT_INODE, &root)?;
        let entries = vec![
            DirEntry {
                inode: ROOT_INODE,
                kind: FileKind::Directory,
                name: ".".into(),
            },
            DirEntry {
                inode: ROOT_INODE,
                kind: FileKind::Directory,
                name: "..".into(),
            },
        ];
        fs.write_directory(ROOT_INODE, &mut root, &entries)?;
        fs.file.sync_all()?;
        Ok(fs)
    }

    /// Open and validate an existing Rustyfile image.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let image_size = file.metadata()?.len();
        let mut fs = Self {
            file,
            // Block zero must be read before the real count is known.
            superblock: Superblock { total_blocks: 0 },
        };

        let bytes = fs.read_block(0)?;
        let superblock = Superblock::decode(&bytes)?;
        if image_size != superblock.total_blocks as u64 * BLOCK_SIZE as u64 {
            return Err(FsError::InvalidImage(
                "image length differs from its superblock".into(),
            ));
        }
        fs.superblock = superblock;
        Ok(fs)
    }

    /// Flush all pending host-file writes.
    pub fn sync(&mut self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }

    /// Count allocated blocks and inodes.
    pub fn info(&mut self) -> Result<FsInfo> {
        let block_bitmap = self.read_block(BLOCK_BITMAP_BLOCK)?;
        let inode_bitmap = self.read_block(INODE_BITMAP_BLOCK)?;
        Ok(FsInfo {
            total_blocks: self.superblock.total_blocks,
            used_blocks: count_set_bits(&block_bitmap, self.superblock.total_blocks),
            total_inodes: INODE_COUNT,
            used_inodes: count_set_bits(&inode_bitmap, INODE_COUNT),
        })
    }

    /// Check that an image fits the fixed, one-bitmap layout.
    fn validate_image_size(image_size: u64) -> Result<()> {
        if !image_size.is_multiple_of(BLOCK_SIZE as u64) {
            return Err(FsError::InvalidImage(format!(
                "size must be a multiple of {BLOCK_SIZE} bytes"
            )));
        }
        if image_size < (DATA_BLOCK_START as u64 + 1) * BLOCK_SIZE as u64 {
            return Err(FsError::InvalidImage("image is too small".into()));
        }
        if image_size > MAX_IMAGE_SIZE {
            return Err(FsError::InvalidImage(format!(
                "image exceeds the educational format's {} MiB limit",
                MAX_IMAGE_SIZE / 1024 / 1024
            )));
        }
        Ok(())
    }
}
