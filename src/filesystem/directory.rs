//! Directory entries, path traversal, and namespace changes.

use super::{DirEntryInfo, FileSystem, FsError, InodeInfo, Result};
use crate::layout::{DirEntry, FileKind, Inode, DIR_ENTRY_SIZE, MAX_NAME_LEN, ROOT_INODE};

impl FileSystem {
    /// Resolve an absolute or cwd-relative path to an inode number.
    pub fn resolve_path(&mut self, cwd: u32, path: &str) -> Result<u32> {
        if path.is_empty() {
            return Err(FsError::InvalidPath(path.into()));
        }
        let mut current = if path.starts_with('/') {
            ROOT_INODE
        } else {
            cwd
        };

        // Each name is looked up in the directory reached so far.
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

    /// List a directory, hiding its structural `.` and `..` entries.
    pub fn list_dir(&mut self, cwd: u32, path: &str) -> Result<Vec<DirEntryInfo>> {
        let inode_number = self.resolve_path(cwd, path)?;
        let inode = self.read_inode(inode_number)?;
        if inode.kind != FileKind::Directory {
            return Err(FsError::NotDirectory(path.into()));
        }

        // Read each child inode to add its current size to the listing.
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

    /// Return the inode metadata for one path.
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

    /// Create an empty directory with `.` and `..` entries.
    pub fn create_dir(&mut self, cwd: u32, path: &str) -> Result<u32> {
        self.create(cwd, path, FileKind::Directory)
    }

    /// Remove an empty, non-root directory.
    pub fn remove_dir(&mut self, cwd: u32, path: &str) -> Result<()> {
        let (parent_number, name) = self.resolve_parent(cwd, path)?;
        let mut parent = self.read_inode(parent_number)?;
        let mut entries = self.read_directory(&parent)?;
        let index = entry_index(&entries, &name).ok_or_else(|| FsError::NotFound(path.into()))?;
        let target_number = entries[index].inode;

        if target_number == ROOT_INODE {
            return Err(FsError::InvalidPath("cannot remove root".into()));
        }
        let target = self.read_inode(target_number)?;
        if target.kind != FileKind::Directory {
            return Err(FsError::NotDirectory(path.into()));
        }
        if has_user_entries(&self.read_directory(&target)?) {
            return Err(FsError::DirectoryNotEmpty(path.into()));
        }

        // Unlink first, then release the unreachable inode and blocks.
        entries.remove(index);
        self.write_directory(parent_number, &mut parent, &entries)?;
        self.free_inode_contents(&target)?;
        self.set_inode_allocated(target_number, false)?;
        Ok(())
    }

    /// Create an inode and link it into its parent directory.
    pub(super) fn create(&mut self, cwd: u32, path: &str, kind: FileKind) -> Result<u32> {
        let (parent_number, name) = self.resolve_parent(cwd, path)?;
        validate_name(&name)?;
        let mut parent = self.read_inode(parent_number)?;
        if parent.kind != FileKind::Directory {
            return Err(FsError::NotDirectory(path.into()));
        }
        let mut entries = self.read_directory(&parent)?;
        if entry_index(&entries, &name).is_some() {
            return Err(FsError::AlreadyExists(path.into()));
        }

        // Reserve and initialize the child before the parent can reference it.
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

        // Linking is the commit point; clean up the child if it fails.
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

    /// Resolve all but the final path component.
    pub(super) fn resolve_parent(&mut self, cwd: u32, path: &str) -> Result<(u32, String)> {
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
        Ok((self.resolve_path(cwd, parent_path)?, name.to_owned()))
    }

    /// Decode a directory inode's fixed-size entries.
    pub(super) fn read_directory(&mut self, inode: &Inode) -> Result<Vec<DirEntry>> {
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

    /// Encode directory entries and persist them as inode data.
    pub(super) fn write_directory(
        &mut self,
        inode_number: u32,
        inode: &mut Inode,
        entries: &[DirEntry],
    ) -> Result<()> {
        let mut data = Vec::with_capacity(entries.len() * DIR_ENTRY_SIZE);
        for entry in entries {
            data.extend_from_slice(&entry.encode()?);
        }

        // Reuse blocks so deletion still works on a completely full image.
        self.rewrite_inode_data_reusing_blocks(inode_number, inode, &data)
    }
}

/// Find a named entry's position inside a directory.
fn entry_index(entries: &[DirEntry], name: &str) -> Option<usize> {
    entries.iter().position(|entry| entry.name == name)
}

/// Ignore `.` and `..` when deciding whether a directory is empty.
fn has_user_entries(entries: &[DirEntry]) -> bool {
    entries
        .iter()
        .any(|entry| entry.name != "." && entry.name != "..")
}

/// Enforce the directory-entry naming rules.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(FsError::InvalidPath(name.into()));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(FsError::NameTooLong(name.into()));
    }
    Ok(())
}
