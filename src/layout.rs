//! On-disk data structures and their byte encoding.
//!
//! Real filesystems cannot write Rust structs directly: padding, alignment and
//! endianness could change. Rustyfile explicitly encodes every integer as
//! little-endian bytes. This also makes the disk format easy to inspect.

use crate::{FsError, Result};

pub const BLOCK_SIZE: usize = 4096;
pub const MAGIC: [u8; 8] = *b"RUSTYFS\0";
pub const VERSION: u32 = 1;

pub const INODE_COUNT: u32 = 256;
pub const INODE_SIZE: usize = 128;
pub const INODES_PER_BLOCK: u32 = (BLOCK_SIZE / INODE_SIZE) as u32;
pub const INODE_TABLE_BLOCKS: u32 = INODE_COUNT / INODES_PER_BLOCK;

pub const SUPERBLOCK_BLOCK: u32 = 0;
pub const INODE_BITMAP_BLOCK: u32 = 1;
pub const BLOCK_BITMAP_BLOCK: u32 = 2;
pub const INODE_TABLE_START: u32 = 3;
pub const DATA_BLOCK_START: u32 = INODE_TABLE_START + INODE_TABLE_BLOCKS;
pub const ROOT_INODE: u32 = 0;

/// One bitmap block contains 32768 bits. At 4 KiB per block that addresses
/// images up to 128 MiB, which comfortably covers the project's 100 MiB goal.
pub const MAX_BLOCKS: u32 = (BLOCK_SIZE * 8) as u32;
pub const MAX_IMAGE_SIZE: u64 = MAX_BLOCKS as u64 * BLOCK_SIZE as u64;

pub const DIRECT_POINTERS: usize = 12;
pub const MAX_FILE_SIZE: usize = DIRECT_POINTERS * BLOCK_SIZE;

pub const DIR_ENTRY_SIZE: usize = 64;
pub const DIR_NAME_BYTES: usize = 56;
pub const MAX_NAME_LEN: usize = 55;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    File = 1,
    Directory = 2,
}

impl FileKind {
    /// Decode the stable numeric kind stored on disk.
    pub fn from_byte(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::File),
            2 => Ok(Self::Directory),
            _ => Err(FsError::Corrupt(format!("unknown inode kind {value}"))),
        }
    }

    /// Return the kind name shown by `stat`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Superblock {
    pub total_blocks: u32,
}

impl Superblock {
    /// Encode the fixed layout description into block zero.
    pub fn encode(&self) -> [u8; BLOCK_SIZE] {
        let mut bytes = [0; BLOCK_SIZE];

        // Identity fields reject unrelated or incompatible images.
        bytes[0..8].copy_from_slice(&MAGIC);
        put_u32(&mut bytes, 8, VERSION);
        put_u32(&mut bytes, 12, BLOCK_SIZE as u32);

        // Location fields describe every metadata region.
        put_u32(&mut bytes, 16, self.total_blocks);
        put_u32(&mut bytes, 20, INODE_COUNT);
        put_u32(&mut bytes, 24, INODE_BITMAP_BLOCK);
        put_u32(&mut bytes, 28, BLOCK_BITMAP_BLOCK);
        put_u32(&mut bytes, 32, INODE_TABLE_START);
        put_u32(&mut bytes, 36, DATA_BLOCK_START);
        put_u32(&mut bytes, 40, ROOT_INODE);
        bytes
    }

    /// Validate and decode block zero.
    pub fn decode(bytes: &[u8; BLOCK_SIZE]) -> Result<Self> {
        // Check identity before trusting any location fields.
        if bytes[0..8] != MAGIC {
            return Err(FsError::InvalidImage(
                "missing RUSTYFS signature; run `rustyfile mkfs` first".into(),
            ));
        }
        let version = get_u32(bytes, 8);
        if version != VERSION {
            return Err(FsError::InvalidImage(format!(
                "unsupported format version {version}"
            )));
        }
        // Version 1 has a fixed layout compiled into the reader.
        if get_u32(bytes, 12) != BLOCK_SIZE as u32
            || get_u32(bytes, 20) != INODE_COUNT
            || get_u32(bytes, 24) != INODE_BITMAP_BLOCK
            || get_u32(bytes, 28) != BLOCK_BITMAP_BLOCK
            || get_u32(bytes, 32) != INODE_TABLE_START
            || get_u32(bytes, 36) != DATA_BLOCK_START
            || get_u32(bytes, 40) != ROOT_INODE
        {
            return Err(FsError::InvalidImage(
                "layout constants do not match this build".into(),
            ));
        }
        let total_blocks = get_u32(bytes, 16);
        // The count must leave room for metadata and at least one data block.
        if !(DATA_BLOCK_START + 1..=MAX_BLOCKS).contains(&total_blocks) {
            return Err(FsError::InvalidImage("invalid image block count".into()));
        }
        Ok(Self { total_blocks })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Inode {
    pub kind: FileKind,
    pub size: u64,
    pub direct: [u32; DIRECT_POINTERS],
}

impl Inode {
    /// Create an inode with no data blocks.
    pub fn empty(kind: FileKind) -> Self {
        Self {
            kind,
            size: 0,
            direct: [0; DIRECT_POINTERS],
        }
    }

    /// Encode inode metadata and direct pointers into one table slot.
    pub fn encode(&self) -> [u8; INODE_SIZE] {
        let mut bytes = [0; INODE_SIZE];
        bytes[0] = self.kind as u8;
        put_u64(&mut bytes, 8, self.size);

        // Zero means unused; real data starts at block 11.
        for (index, block) in self.direct.iter().enumerate() {
            put_u32(&mut bytes, 16 + index * 4, *block);
        }
        bytes
    }

    /// Decode and sanity-check one inode-table slot.
    pub fn decode(bytes: &[u8; INODE_SIZE]) -> Result<Self> {
        let kind = FileKind::from_byte(bytes[0])?;
        let size = get_u64(bytes, 8);
        if size > MAX_FILE_SIZE as u64 {
            return Err(FsError::Corrupt("inode size exceeds direct blocks".into()));
        }
        let mut direct = [0; DIRECT_POINTERS];
        // Pointer ranges are checked later when the blocks are read.
        for (index, block) in direct.iter_mut().enumerate() {
            *block = get_u32(bytes, 16 + index * 4);
        }
        Ok(Self { kind, size, direct })
    }

    /// Count the inode's nonzero direct pointers.
    pub fn blocks_used(&self) -> usize {
        self.direct.iter().filter(|block| **block != 0).count()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DirEntry {
    pub inode: u32,
    pub kind: FileKind,
    pub name: String,
}

impl DirEntry {
    /// Encode one name-to-inode mapping into 64 bytes.
    pub fn encode(&self) -> Result<[u8; DIR_ENTRY_SIZE]> {
        let name = self.name.as_bytes();
        if name.len() > MAX_NAME_LEN {
            return Err(FsError::NameTooLong(self.name.clone()));
        }
        let mut bytes = [0; DIR_ENTRY_SIZE];

        // The header allows unused slots and exact UTF-8 name lengths.
        put_u32(&mut bytes, 0, self.inode);
        bytes[4] = self.kind as u8;
        bytes[5] = 1; // "used" flag
        bytes[6] = name.len() as u8;
        bytes[8..8 + name.len()].copy_from_slice(name);
        Ok(bytes)
    }

    /// Decode one directory slot, returning `None` for an unused slot.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>> {
        if bytes[5] == 0 {
            return Ok(None);
        }
        let length = bytes[6] as usize;
        if length > MAX_NAME_LEN {
            return Err(FsError::Corrupt("directory name is too long".into()));
        }
        // Names are valid UTF-8 by format design.
        let name = std::str::from_utf8(&bytes[8..8 + length])
            .map_err(|_| FsError::Corrupt("directory name is not UTF-8".into()))?
            .to_owned();
        Ok(Some(Self {
            inode: get_u32(bytes, 0),
            kind: FileKind::from_byte(bytes[4])?,
            name,
        }))
    }
}

/// Store a little-endian 32-bit integer at a stable offset.
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Store a little-endian 64-bit integer at a stable offset.
fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// Read a little-endian 32-bit integer from a stable offset.
fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// Read a little-endian 64-bit integer from a stable offset.
fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
