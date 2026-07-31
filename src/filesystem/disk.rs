//! Raw block I/O, inode I/O, and free-space allocation.

use super::{FileSystem, FsError, Result};
use crate::layout::{
    Inode, BLOCK_BITMAP_BLOCK, BLOCK_SIZE, DATA_BLOCK_START, DIRECT_POINTERS, INODES_PER_BLOCK,
    INODE_BITMAP_BLOCK, INODE_COUNT, INODE_SIZE, INODE_TABLE_START, MAX_FILE_SIZE,
};
use std::io::{Read, Seek, SeekFrom, Write};

impl FileSystem {
    /// Join an inode's data blocks into its exact byte contents.
    pub(super) fn read_inode_data(&mut self, inode: &Inode) -> Result<Vec<u8>> {
        let mut result = Vec::with_capacity(inode.size as usize);
        let blocks_needed = blocks_for(inode.size as usize);

        for block in inode.direct.iter().take(blocks_needed) {
            // A pointer outside the data region means metadata is damaged.
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

    /// Replace data while preserving old contents on allocation failure.
    pub(super) fn replace_inode_data(
        &mut self,
        inode_number: u32,
        inode: &mut Inode,
        data: &[u8],
    ) -> Result<()> {
        ensure_file_size(data.len())?;
        let needed = blocks_for(data.len());

        // Claim every new block before changing the inode.
        let mut new_blocks = Vec::with_capacity(needed);
        for _ in 0..needed {
            match self.allocate_block() {
                Ok(block) => new_blocks.push(block),
                Err(error) => {
                    // Roll back claims; the old inode is still untouched.
                    for block in new_blocks {
                        self.set_block_allocated(block, false)?;
                    }
                    return Err(error);
                }
            }
        }

        // Store full blocks, padding only the final block with zeroes.
        for (index, block) in new_blocks.iter().enumerate() {
            let mut bytes = [0; BLOCK_SIZE];
            let start = index * BLOCK_SIZE;
            let end = data.len().min(start + BLOCK_SIZE);
            bytes[..end - start].copy_from_slice(&data[start..end]);
            self.write_block(*block, &bytes)?;
        }

        // Point the inode at new data before releasing old blocks.
        let old_blocks = used_blocks(inode);
        inode.direct = [0; DIRECT_POINTERS];
        inode.direct[..new_blocks.len()].copy_from_slice(&new_blocks);
        inode.size = data.len() as u64;
        self.write_inode(inode_number, inode)?;
        for block in old_blocks {
            self.set_block_allocated(block, false)?;
        }
        Ok(())
    }

    /// Rewrite metadata using existing blocks whenever possible.
    pub(super) fn rewrite_inode_data_reusing_blocks(
        &mut self,
        inode_number: u32,
        inode: &mut Inode,
        data: &[u8],
    ) -> Result<()> {
        ensure_file_size(data.len())?;
        let needed = blocks_for(data.len());
        let old_blocks = used_blocks(inode);
        let retained = old_blocks.len().min(needed);
        let mut blocks = old_blocks[..retained].to_vec();
        let mut newly_allocated = Vec::new();

        // Only directory growth needs additional blocks.
        for _ in retained..needed {
            match self.allocate_block() {
                Ok(block) => {
                    blocks.push(block);
                    newly_allocated.push(block);
                }
                Err(error) => {
                    for block in newly_allocated {
                        self.set_block_allocated(block, false)?;
                    }
                    return Err(error);
                }
            }
        }

        // Rewrite retained blocks and fill any newly allocated blocks.
        for (index, block) in blocks.iter().enumerate() {
            let mut bytes = [0; BLOCK_SIZE];
            let start = index * BLOCK_SIZE;
            let end = data.len().min(start + BLOCK_SIZE);
            bytes[..end - start].copy_from_slice(&data[start..end]);
            self.write_block(*block, &bytes)?;
        }

        // Commit the new length, then release blocks no longer needed.
        inode.direct = [0; DIRECT_POINTERS];
        inode.direct[..blocks.len()].copy_from_slice(&blocks);
        inode.size = data.len() as u64;
        self.write_inode(inode_number, inode)?;
        for block in old_blocks.into_iter().skip(needed) {
            self.set_block_allocated(block, false)?;
        }
        Ok(())
    }

    /// Return every data block owned by an inode to the free pool.
    pub(super) fn free_inode_contents(&mut self, inode: &Inode) -> Result<()> {
        for block in used_blocks(inode) {
            self.set_block_allocated(block, false)?;
        }
        Ok(())
    }

    /// Claim the first free non-root inode.
    pub(super) fn allocate_inode(&mut self) -> Result<u32> {
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

    /// Set or clear one inode-allocation bit.
    pub(super) fn set_inode_allocated(&mut self, inode: u32, allocated: bool) -> Result<()> {
        let mut bitmap = self.read_block(INODE_BITMAP_BLOCK)?;
        set_bit(&mut bitmap, inode, allocated);
        self.write_block(INODE_BITMAP_BLOCK, &bitmap)
    }

    /// Claim and clear the first free data block.
    fn allocate_block(&mut self) -> Result<u32> {
        let mut bitmap = self.read_block(BLOCK_BITMAP_BLOCK)?;
        for block in DATA_BLOCK_START..self.superblock.total_blocks {
            if !get_bit(&bitmap, block) {
                set_bit(&mut bitmap, block, true);
                self.write_block(BLOCK_BITMAP_BLOCK, &bitmap)?;
                // Clearing prevents a new file from seeing deleted bytes.
                self.write_block(block, &[0; BLOCK_SIZE])?;
                return Ok(block);
            }
        }
        Err(FsError::NoSpace)
    }

    /// Set or clear one block-allocation bit.
    fn set_block_allocated(&mut self, block: u32, allocated: bool) -> Result<()> {
        let mut bitmap = self.read_block(BLOCK_BITMAP_BLOCK)?;
        set_bit(&mut bitmap, block, allocated);
        self.write_block(BLOCK_BITMAP_BLOCK, &bitmap)
    }

    /// Decode one inode from its slot in the inode table.
    pub(super) fn read_inode(&mut self, inode: u32) -> Result<Inode> {
        if inode >= INODE_COUNT {
            return Err(FsError::Corrupt(format!("invalid inode {inode}")));
        }
        let block = INODE_TABLE_START + inode / INODES_PER_BLOCK;
        let offset = inode as usize % INODES_PER_BLOCK as usize * INODE_SIZE;
        let bytes = self.read_block(block)?;
        let raw: [u8; INODE_SIZE] = bytes[offset..offset + INODE_SIZE].try_into().unwrap();
        Inode::decode(&raw)
    }

    /// Encode one inode into its slot without disturbing adjacent inodes.
    pub(super) fn write_inode(&mut self, inode_number: u32, inode: &Inode) -> Result<()> {
        let block = INODE_TABLE_START + inode_number / INODES_PER_BLOCK;
        let offset = inode_number as usize % INODES_PER_BLOCK as usize * INODE_SIZE;
        let mut bytes = self.read_block(block)?;
        bytes[offset..offset + INODE_SIZE].copy_from_slice(&inode.encode());
        self.write_block(block, &bytes)
    }

    /// Read one complete block at its calculated host-file offset.
    pub(super) fn read_block(&mut self, block: u32) -> Result<[u8; BLOCK_SIZE]> {
        if block >= self.superblock.total_blocks && self.superblock.total_blocks != 0 {
            return Err(FsError::Corrupt(format!("block {block} is out of range")));
        }
        let mut bytes = [0; BLOCK_SIZE];
        self.file
            .seek(SeekFrom::Start(block as u64 * BLOCK_SIZE as u64))?;
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    /// Write one complete block at its calculated host-file offset.
    pub(super) fn write_block(&mut self, block: u32, bytes: &[u8; BLOCK_SIZE]) -> Result<()> {
        if block >= self.superblock.total_blocks {
            return Err(FsError::Corrupt(format!("block {block} is out of range")));
        }
        self.file
            .seek(SeekFrom::Start(block as u64 * BLOCK_SIZE as u64))?;
        self.file.write_all(bytes)?;
        Ok(())
    }
}

/// Round a byte count up to whole blocks.
fn blocks_for(byte_count: usize) -> usize {
    byte_count.div_ceil(BLOCK_SIZE)
}

/// Reject data that cannot fit in the inode's direct pointers.
fn ensure_file_size(size: usize) -> Result<()> {
    if size > MAX_FILE_SIZE {
        return Err(FsError::FileTooLarge {
            size,
            maximum: MAX_FILE_SIZE,
        });
    }
    Ok(())
}

/// Collect the nonzero direct pointers in disk order.
fn used_blocks(inode: &Inode) -> Vec<u32> {
    inode
        .direct
        .iter()
        .copied()
        .filter(|block| *block != 0)
        .collect()
}

/// Read one bit from a bitmap.
fn get_bit(bitmap: &[u8], bit: u32) -> bool {
    bitmap[bit as usize / 8] & (1 << (bit % 8)) != 0
}

/// Set or clear one bit in a bitmap.
pub(super) fn set_bit(bitmap: &mut [u8], bit: u32, value: bool) {
    let byte = &mut bitmap[bit as usize / 8];
    let mask = 1 << (bit % 8);
    if value {
        *byte |= mask;
    } else {
        *byte &= !mask;
    }
}

/// Count set bits only in the valid portion of a bitmap.
pub(super) fn count_set_bits(bitmap: &[u8], up_to: u32) -> u32 {
    (0..up_to).filter(|bit| get_bit(bitmap, *bit)).count() as u32
}
