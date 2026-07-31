//! Public operations on regular files.

use super::{FileSystem, FsError, Result};
use crate::layout::FileKind;

impl FileSystem {
    /// Create an empty regular file.
    pub fn create_file(&mut self, cwd: u32, path: &str) -> Result<u32> {
        self.create(cwd, path, FileKind::File)
    }

    /// Read a regular file's exact bytes.
    pub fn read_file(&mut self, cwd: u32, path: &str) -> Result<Vec<u8>> {
        let inode_number = self.resolve_path(cwd, path)?;
        let inode = self.read_inode(inode_number)?;
        if inode.kind == FileKind::Directory {
            return Err(FsError::IsDirectory(path.into()));
        }
        self.read_inode_data(&inode)
    }

    /// Create or replace a regular file.
    pub fn write_file(&mut self, cwd: u32, path: &str, data: &[u8]) -> Result<()> {
        // `write` creates the final file but still reports invalid parent paths.
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

    /// Append bytes by reading and replacing the small direct-block file.
    pub fn append_file(&mut self, cwd: u32, path: &str, data: &[u8]) -> Result<()> {
        let mut contents = match self.read_file(cwd, path) {
            Ok(contents) => contents,
            Err(FsError::NotFound(_)) => Vec::new(),
            Err(error) => return Err(error),
        };
        contents.extend_from_slice(data);
        self.write_file(cwd, path, &contents)
    }

    /// Unlink a regular file and release its inode and data blocks.
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

        // Remove the name before returning its storage to the free pools.
        entries.remove(index);
        self.write_directory(parent_number, &mut parent, &entries)?;
        self.free_inode_contents(&target)?;
        self.set_inode_allocated(target_number, false)?;
        Ok(())
    }
}
