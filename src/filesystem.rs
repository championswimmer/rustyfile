use crate::layout::{
    DirEntry, FileKind, Inode, Superblock, BLOCK_BITMAP_BLOCK, BLOCK_SIZE, DATA_BLOCK_START,
    DIRECT_POINTERS, DIR_ENTRY_SIZE, INODES_PER_BLOCK, INODE_BITMAP_BLOCK, INODE_COUNT, INODE_SIZE,
    INODE_TABLE_START, MAX_FILE_SIZE, MAX_IMAGE_SIZE, MAX_NAME_LEN, ROOT_INODE,
};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub type Result<T> = std::result::Result<T, FsError>;

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
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Debug)]
pub struct DirEntryInfo {
    pub inode: u32,
    pub kind: FileKind,
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct InodeInfo {
    pub inode: u32,
    pub kind: FileKind,
    pub size: u64,
    pub blocks: usize,
}

#[derive(Clone, Debug)]
pub struct FsInfo {
    pub total_blocks: u32,
    pub used_blocks: u32,
    pub total_inodes: u32,
    pub used_inodes: u32,
}

/// An opened Rustyfile image.
///
/// All changes go directly to the image. There is no cache and no journal,
/// deliberately keeping the path from an operation to disk writes obvious.
pub struct FileSystem {
    file: File,
    superblock: Superblock,
}

impl FileSystem {
    /// Create a new image (or replace the contents of an existing block file).
    pub fn format(path: impl AsRef<Path>, size: Option<u64>) -> Result<Self> {
        let path = path.as_ref();
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        if size.is_some() {
            options.create(true);
        }
        let file = options.open(path)?;
        if let Some(size) = size {
            file.set_len(size)?;
        }
        let image_size = file.metadata()?.len();
        if image_size % BLOCK_SIZE as u64 != 0 {
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

        let superblock = Superblock {
            total_blocks: (image_size / BLOCK_SIZE as u64) as u32,
        };
        let mut fs = Self { file, superblock };

        // Clear all metadata blocks. Data blocks need not be cleared: no inode
        // can refer to them until allocated, and allocated blocks are zeroed.
        for block in 0..DATA_BLOCK_START {
            fs.write_block(block, &[0; BLOCK_SIZE])?;
        }
        fs.write_block(0, &fs.superblock.encode())?;

        // Mark metadata blocks used in the block bitmap.
        let mut block_bitmap = [0; BLOCK_SIZE];
        for block in 0..DATA_BLOCK_START {
            set_bit(&mut block_bitmap, block, true);
        }
        fs.write_block(BLOCK_BITMAP_BLOCK, &block_bitmap)?;

        // Reserve inode zero for the root directory.
        let mut inode_bitmap = [0; BLOCK_SIZE];
        set_bit(&mut inode_bitmap, ROOT_INODE, true);
        fs.write_block(INODE_BITMAP_BLOCK, &inode_bitmap)?;

        let mut root = Inode::empty(FileKind::Directory);
        fs.write_inode(ROOT_INODE, &root)?;
        let root_entries = vec![
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
        fs.write_directory(ROOT_INODE, &mut root, &root_entries)?;
        fs.file.sync_all()?;
        Ok(fs)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let image_size = file.metadata()?.len();
        let mut fs = Self {
            file,
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

    pub fn sync(&mut self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }

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

    pub fn resolve_path(&mut self, cwd: u32, path: &str) -> Result<u32> {
        if path.is_empty() {
            return Err(FsError::InvalidPath(path.into()));
        }
        let mut current = if path.starts_with('/') {
            ROOT_INODE
        } else {
            cwd
        };
        for component in path.split('/').filter(|part| !part.is_empty()) {
            if component == "." {
                continue;
            }
            let inode = self.read_inode(current)?;
            if inode.kind != FileKind::Directory {
                return Err(FsError::NotDirectory(component.into()));
            }
            let entry = self
                .read_directory(&inode)?
                .into_iter()
                .find(|entry| entry.name == component)
                .ok_or_else(|| FsError::NotFound(path.into()))?;
            current = entry.inode;
        }
        Ok(current)
    }

    pub fn list_dir(&mut self, cwd: u32, path: &str) -> Result<Vec<DirEntryInfo>> {
        let inode_number = self.resolve_path(cwd, path)?;
        let inode = self.read_inode(inode_number)?;
        if inode.kind != FileKind::Directory {
            return Err(FsError::NotDirectory(path.into()));
        }
        let mut result = Vec::new();
        for entry in self.read_directory(&inode)? {
            if entry.name == "." || entry.name == ".." {
                continue;
            }
            let child = self.read_inode(entry.inode)?;
            result.push(DirEntryInfo {
                inode: entry.inode,
                kind: entry.kind,
                name: entry.name,
                size: child.size,
            });
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    pub fn stat(&mut self, cwd: u32, path: &str) -> Result<InodeInfo> {
        let inode_number = self.resolve_path(cwd, path)?;
        let inode = self.read_inode(inode_number)?;
        Ok(InodeInfo {
            inode: inode_number,
            kind: inode.kind,
            size: inode.size,
            blocks: inode.blocks_used(),
        })
    }

    pub fn create_file(&mut self, cwd: u32, path: &str) -> Result<u32> {
        self.create(cwd, path, FileKind::File)
    }

    pub fn create_dir(&mut self, cwd: u32, path: &str) -> Result<u32> {
        self.create(cwd, path, FileKind::Directory)
    }

    pub fn read_file(&mut self, cwd: u32, path: &str) -> Result<Vec<u8>> {
        let inode_number = self.resolve_path(cwd, path)?;
        let inode = self.read_inode(inode_number)?;
        if inode.kind == FileKind::Directory {
            return Err(FsError::IsDirectory(path.into()));
        }
        self.read_inode_data(&inode)
    }

    /// Replace a file's contents. New blocks are allocated before old blocks
    /// are released, so an out-of-space error leaves the old file intact.
    pub fn write_file(&mut self, cwd: u32, path: &str, data: &[u8]) -> Result<()> {
        let inode_number = match self.resolve_path(cwd, path) {
            Ok(number) => number,
            Err(FsError::NotFound(_)) => self.create_file(cwd, path)?,
            Err(error) => return Err(error),
        };
        let mut inode = self.read_inode(inode_number)?;
        if inode.kind == FileKind::Directory {
            return Err(FsError::IsDirectory(path.into()));
        }
        self.replace_inode_data(inode_number, &mut inode, data)
    }

    pub fn append_file(&mut self, cwd: u32, path: &str, data: &[u8]) -> Result<()> {
        let mut contents = match self.read_file(cwd, path) {
            Ok(contents) => contents,
            Err(FsError::NotFound(_)) => Vec::new(),
            Err(error) => return Err(error),
        };
        contents.extend_from_slice(data);
        self.write_file(cwd, path, &contents)
    }

    pub fn remove_file(&mut self, cwd: u32, path: &str) -> Result<()> {
        let (parent_number, name) = self.resolve_parent(cwd, path)?;
        let mut parent = self.read_inode(parent_number)?;
        let mut entries = self.read_directory(&parent)?;
        let index = entries
            .iter()
            .position(|entry| entry.name == name)
            .ok_or_else(|| FsError::NotFound(path.into()))?;
        let target_number = entries[index].inode;
        let target = self.read_inode(target_number)?;
        if target.kind == FileKind::Directory {
            return Err(FsError::IsDirectory(path.into()));
        }
        entries.remove(index);
        self.write_directory(parent_number, &mut parent, &entries)?;
        self.free_inode_contents(&target)?;
        self.set_inode_allocated(target_number, false)?;
        Ok(())
    }

    pub fn remove_dir(&mut self, cwd: u32, path: &str) -> Result<()> {
        let (parent_number, name) = self.resolve_parent(cwd, path)?;
        let mut parent = self.read_inode(parent_number)?;
        let mut entries = self.read_directory(&parent)?;
        let index = entries
            .iter()
            .position(|entry| entry.name == name)
            .ok_or_else(|| FsError::NotFound(path.into()))?;
        let target_number = entries[index].inode;
        if target_number == ROOT_INODE {
            return Err(FsError::InvalidPath("cannot remove root".into()));
        }
        let target = self.read_inode(target_number)?;
        if target.kind != FileKind::Directory {
            return Err(FsError::NotDirectory(path.into()));
        }
        if self
            .read_directory(&target)?
            .iter()
            .any(|entry| entry.name != "." && entry.name != "..")
        {
            return Err(FsError::DirectoryNotEmpty(path.into()));
        }
        entries.remove(index);
        self.write_directory(parent_number, &mut parent, &entries)?;
        self.free_inode_contents(&target)?;
        self.set_inode_allocated(target_number, false)?;
        Ok(())
    }

    fn create(&mut self, cwd: u32, path: &str, kind: FileKind) -> Result<u32> {
        let (parent_number, name) = self.resolve_parent(cwd, path)?;
        validate_name(&name)?;
        let mut parent = self.read_inode(parent_number)?;
        if parent.kind != FileKind::Directory {
            return Err(FsError::NotDirectory(path.into()));
        }
        let mut entries = self.read_directory(&parent)?;
        if entries.iter().any(|entry| entry.name == name) {
            return Err(FsError::AlreadyExists(path.into()));
        }

        let inode_number = self.allocate_inode()?;
        let mut inode = Inode::empty(kind);
        self.write_inode(inode_number, &inode)?;

        if kind == FileKind::Directory {
            let initial = vec![
                DirEntry {
                    inode: inode_number,
                    kind,
                    name: ".".into(),
                },
                DirEntry {
                    inode: parent_number,
                    kind: FileKind::Directory,
                    name: "..".into(),
                },
            ];
            if let Err(error) = self.write_directory(inode_number, &mut inode, &initial) {
                self.set_inode_allocated(inode_number, false)?;
                return Err(error);
            }
        }

        entries.push(DirEntry {
            inode: inode_number,
            kind,
            name,
        });
        if let Err(error) = self.write_directory(parent_number, &mut parent, &entries) {
            self.free_inode_contents(&inode)?;
            self.set_inode_allocated(inode_number, false)?;
            return Err(error);
        }
        Ok(inode_number)
    }

    fn resolve_parent(&mut self, cwd: u32, path: &str) -> Result<(u32, String)> {
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() || trimmed == "/" {
            return Err(FsError::InvalidPath(path.into()));
        }
        let (parent_path, name) = match trimmed.rsplit_once('/') {
            Some(("", name)) => ("/", name),
            Some((parent, name)) => (parent, name),
            None => (".", trimmed),
        };
        if name.is_empty() || name == "." || name == ".." {
            return Err(FsError::InvalidPath(path.into()));
        }
        let parent = self.resolve_path(cwd, parent_path)?;
        Ok((parent, name.to_owned()))
    }

    fn read_directory(&mut self, inode: &Inode) -> Result<Vec<DirEntry>> {
        if inode.kind != FileKind::Directory {
            return Err(FsError::Corrupt(
                "attempted to decode non-directory data".into(),
            ));
        }
        let data = self.read_inode_data(inode)?;
        if data.len() % DIR_ENTRY_SIZE != 0 {
            return Err(FsError::Corrupt(
                "directory size is not a whole number of entries".into(),
            ));
        }
        let mut entries = Vec::new();
        for bytes in data.chunks_exact(DIR_ENTRY_SIZE) {
            if let Some(entry) = DirEntry::decode(bytes)? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    fn write_directory(
        &mut self,
        inode_number: u32,
        inode: &mut Inode,
        entries: &[DirEntry],
    ) -> Result<()> {
        let mut data = Vec::with_capacity(entries.len() * DIR_ENTRY_SIZE);
        for entry in entries {
            data.extend_from_slice(&entry.encode()?);
        }
        self.replace_inode_data(inode_number, inode, &data)
    }

    fn read_inode_data(&mut self, inode: &Inode) -> Result<Vec<u8>> {
        let mut result = Vec::with_capacity(inode.size as usize);
        let blocks_needed = blocks_for(inode.size as usize);
        for block in inode.direct.iter().take(blocks_needed) {
            if *block < DATA_BLOCK_START || *block >= self.superblock.total_blocks {
                return Err(FsError::Corrupt(format!(
                    "inode refers to invalid data block {block}"
                )));
            }
            result.extend_from_slice(&self.read_block(*block)?);
        }
        result.truncate(inode.size as usize);
        Ok(result)
    }

    fn replace_inode_data(
        &mut self,
        inode_number: u32,
        inode: &mut Inode,
        data: &[u8],
    ) -> Result<()> {
        if data.len() > MAX_FILE_SIZE {
            return Err(FsError::FileTooLarge {
                size: data.len(),
                maximum: MAX_FILE_SIZE,
            });
        }
        let needed = blocks_for(data.len());
        let mut new_blocks = Vec::with_capacity(needed);
        for _ in 0..needed {
            match self.allocate_block() {
                Ok(block) => new_blocks.push(block),
                Err(error) => {
                    for block in new_blocks {
                        self.set_block_allocated(block, false)?;
                    }
                    return Err(error);
                }
            }
        }

        for (index, block) in new_blocks.iter().enumerate() {
            let mut bytes = [0; BLOCK_SIZE];
            let start = index * BLOCK_SIZE;
            let end = data.len().min(start + BLOCK_SIZE);
            bytes[..end - start].copy_from_slice(&data[start..end]);
            self.write_block(*block, &bytes)?;
        }

        let old_blocks: Vec<u32> = inode
            .direct
            .iter()
            .copied()
            .filter(|block| *block != 0)
            .collect();
        inode.direct = [0; DIRECT_POINTERS];
        inode.direct[..new_blocks.len()].copy_from_slice(&new_blocks);
        inode.size = data.len() as u64;
        self.write_inode(inode_number, inode)?;
        for block in old_blocks {
            self.set_block_allocated(block, false)?;
        }
        Ok(())
    }

    fn free_inode_contents(&mut self, inode: &Inode) -> Result<()> {
        for block in inode.direct.iter().filter(|block| **block != 0) {
            self.set_block_allocated(*block, false)?;
        }
        Ok(())
    }

    fn allocate_inode(&mut self) -> Result<u32> {
        let mut bitmap = self.read_block(INODE_BITMAP_BLOCK)?;
        for inode in 1..INODE_COUNT {
            if !get_bit(&bitmap, inode) {
                set_bit(&mut bitmap, inode, true);
                self.write_block(INODE_BITMAP_BLOCK, &bitmap)?;
                return Ok(inode);
            }
        }
        Err(FsError::NoSpace)
    }

    fn set_inode_allocated(&mut self, inode: u32, allocated: bool) -> Result<()> {
        let mut bitmap = self.read_block(INODE_BITMAP_BLOCK)?;
        set_bit(&mut bitmap, inode, allocated);
        self.write_block(INODE_BITMAP_BLOCK, &bitmap)
    }

    fn allocate_block(&mut self) -> Result<u32> {
        let mut bitmap = self.read_block(BLOCK_BITMAP_BLOCK)?;
        for block in DATA_BLOCK_START..self.superblock.total_blocks {
            if !get_bit(&bitmap, block) {
                set_bit(&mut bitmap, block, true);
                self.write_block(BLOCK_BITMAP_BLOCK, &bitmap)?;
                self.write_block(block, &[0; BLOCK_SIZE])?;
                return Ok(block);
            }
        }
        Err(FsError::NoSpace)
    }

    fn set_block_allocated(&mut self, block: u32, allocated: bool) -> Result<()> {
        let mut bitmap = self.read_block(BLOCK_BITMAP_BLOCK)?;
        set_bit(&mut bitmap, block, allocated);
        self.write_block(BLOCK_BITMAP_BLOCK, &bitmap)
    }

    fn read_inode(&mut self, inode: u32) -> Result<Inode> {
        if inode >= INODE_COUNT {
            return Err(FsError::Corrupt(format!("invalid inode {inode}")));
        }
        let block = INODE_TABLE_START + inode / INODES_PER_BLOCK;
        let offset = inode as usize % INODES_PER_BLOCK as usize * INODE_SIZE;
        let bytes = self.read_block(block)?;
        let raw: [u8; INODE_SIZE] = bytes[offset..offset + INODE_SIZE].try_into().unwrap();
        Inode::decode(&raw)
    }

    fn write_inode(&mut self, inode_number: u32, inode: &Inode) -> Result<()> {
        let block = INODE_TABLE_START + inode_number / INODES_PER_BLOCK;
        let offset = inode_number as usize % INODES_PER_BLOCK as usize * INODE_SIZE;
        let mut bytes = self.read_block(block)?;
        bytes[offset..offset + INODE_SIZE].copy_from_slice(&inode.encode());
        self.write_block(block, &bytes)
    }

    fn read_block(&mut self, block: u32) -> Result<[u8; BLOCK_SIZE]> {
        if block >= self.superblock.total_blocks && self.superblock.total_blocks != 0 {
            return Err(FsError::Corrupt(format!("block {block} is out of range")));
        }
        let mut bytes = [0; BLOCK_SIZE];
        self.file
            .seek(SeekFrom::Start(block as u64 * BLOCK_SIZE as u64))?;
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn write_block(&mut self, block: u32, bytes: &[u8; BLOCK_SIZE]) -> Result<()> {
        if block >= self.superblock.total_blocks {
            return Err(FsError::Corrupt(format!("block {block} is out of range")));
        }
        self.file
            .seek(SeekFrom::Start(block as u64 * BLOCK_SIZE as u64))?;
        self.file.write_all(bytes)?;
        Ok(())
    }
}

fn blocks_for(byte_count: usize) -> usize {
    byte_count.div_ceil(BLOCK_SIZE)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(FsError::InvalidPath(name.into()));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(FsError::NameTooLong(name.into()));
    }
    Ok(())
}

fn get_bit(bitmap: &[u8], bit: u32) -> bool {
    bitmap[bit as usize / 8] & (1 << (bit % 8)) != 0
}

fn set_bit(bitmap: &mut [u8], bit: u32, value: bool) {
    let byte = &mut bitmap[bit as usize / 8];
    let mask = 1 << (bit % 8);
    if value {
        *byte |= mask;
    } else {
        *byte &= !mask;
    }
}

fn count_set_bits(bitmap: &[u8], up_to: u32) -> u32 {
    (0..up_to).filter(|bit| get_bit(bitmap, *bit)).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn image_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustyfile-{name}-{unique}.img"))
    }

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
}
