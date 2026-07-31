# Tutorial 4: deletion, allocation, and failure safety

This tutorial follows `rm`, compares it with `rmdir` and clearing contents, and
then traces the shared inode/block allocation machinery. It closes the loop
between user-visible operations and bitmap changes in the image.

## Three operations that sound like “delete”

They change different pieces of state:

```mermaid
flowchart TB
    subgraph BEFORE["before"]
        NAME0["parent entry<br/>'notes' → inode 2"]:::directory
        INODE0["inode 2<br/>size + pointers"]:::inode
        DATA0["allocated data blocks"]:::block
        NAME0 --> INODE0 --> DATA0
    end

    CLEAR["write empty contents"]:::clear
    RM["rm regular file"]:::remove
    RMDIR["rmdir empty directory"]:::remove

    CNAME["keep parent entry"]:::kept
    CINODE["keep inode bit<br/>size 0, no pointers"]:::kept
    CFREE["free old data blocks"]:::freed

    RNAME["remove parent entry"]:::freed
    RINODE["free inode bit"]:::freed
    RDATA["free data blocks"]:::freed

    DNAME["remove parent entry"]:::freed
    DINODE["free directory inode bit"]:::freed
    DDATA["free directory-data blocks<br/>only if no user entries"]:::freed

    BEFORE --> CLEAR
    BEFORE --> RM
    BEFORE --> RMDIR
    CLEAR --> CNAME --> CINODE --> CFREE
    RM --> RNAME --> RINODE --> RDATA
    RMDIR --> DNAME --> DINODE --> DDATA

    classDef directory fill:#ffedd5,stroke:#ea580c,color:#431407,stroke-width:2px
    classDef inode fill:#dcfce7,stroke:#16a34a,color:#052e16,stroke-width:2px
    classDef block fill:#ffe4e6,stroke:#e11d48,color:#4c0519,stroke-width:2px
    classDef clear fill:#dbeafe,stroke:#2563eb,color:#172554,stroke-width:2px
    classDef remove fill:#fee2e2,stroke:#dc2626,color:#450a0a,stroke-width:2px
    classDef kept fill:#cffafe,stroke:#0891b2,color:#164e63,stroke-width:2px
    classDef freed fill:#f1f5f9,stroke:#64748b,color:#0f172a,stroke-width:2px
```

| Operation | Parent name | Inode allocation bit | Data-block bits |
| --- | --- | --- | --- |
| `write FILE ""` | kept | kept | old blocks cleared |
| `rm FILE` | removed | cleared | cleared |
| `rmdir DIR` | removed, only if empty | cleared | directory-data blocks cleared |

## Delete a regular file (`rm`)

Try:

```console
cargo run -- disk.img rm /notes.txt
```

The CLI dispatch is deliberately thin:

Source: [`src/main.rs` — `rm` dispatch](../../src/main.rs#L195-L198)

```rust
let path = exactly_one(args, "rm <file>")?;
fs.remove_file(*cwd, path)?;
```

The library resolves the parent rather than the target path because it needs to
edit the parent’s entry list:

Source: [`src/filesystem/file.rs` — `remove_file`](../../src/filesystem/file.rs#L48-L69)

```rust
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
```

### Exact deletion sequence

```mermaid
sequenceDiagram
    participant CLI as rm command
    participant DIR as parent directory
    participant IT as inode table
    participant BB as block bitmap
    participant IB as inode bitmap

    rect rgb(219, 234, 254)
        CLI->>DIR: resolve parent and decode entries
        DIR->>IT: read target inode
        IT-->>DIR: kind, size, direct pointers
    end
    rect rgb(255, 237, 213)
        DIR->>DIR: remove filename entry
        DIR->>IT: rewrite parent data and inode
    end
    rect rgb(254, 226, 226)
        DIR->>BB: clear each target data-block bit
        DIR->>IB: clear target inode bit
    end
```

The order encodes an invariant: unlink the name first; free the target only
after it is unreachable. If storage were freed first, another operation could
reuse the inode or blocks while the old parent entry still named them.

Directory data is rewritten with retained blocks where possible. This lets
`rm` shrink a parent directory even when the image has no free data blocks:

Source: [`src/filesystem/disk.rs` — reuse calculation](../../src/filesystem/disk.rs#L75-L103)

```rust
let needed = blocks_for(data.len());
let old_blocks = used_blocks(inode);
let retained = old_blocks.len().min(needed);
let mut blocks = old_blocks[..retained].to_vec();

// Only directory growth needs additional blocks.
for _ in retained..needed {
    match self.allocate_block() {
        Ok(block) => {
            blocks.push(block);
            newly_allocated.push(block);
        }
        // ...
    }
}
```

Deletion does not overwrite the target’s former data blocks. It clears their
allocation bits. A later allocation clears each block before assigning it to a
new owner, preventing stale bytes from becoming visible through a newly
created file.

## Remove a directory (`rmdir`) versus `rm`

The namespace/storage tail is the same, but validation differs:

```mermaid
flowchart TD
    TARGET["target entry + inode"]:::path
    KIND{"target kind"}:::decision
    FILE["rm path"]:::file
    DIR["rmdir path"]:::directory
    FCHK["reject if directory"]:::check
    ROOT["reject root"]:::check
    EMPTY["decode entries;<br/>reject if any except '.' / '..'"]:::check
    UNLINK["rewrite parent without target"]:::write
    FREE["free target blocks + inode bit"]:::free

    TARGET --> KIND
    KIND -- regular file --> FILE --> FCHK --> UNLINK
    KIND -- directory --> DIR --> ROOT --> EMPTY --> UNLINK
    UNLINK --> FREE

    classDef path fill:#ede9fe,stroke:#7c3aed,color:#2e1065,stroke-width:2px
    classDef decision fill:#fef3c7,stroke:#d97706,color:#451a03,stroke-width:2px
    classDef file fill:#dbeafe,stroke:#2563eb,color:#172554,stroke-width:2px
    classDef directory fill:#ffedd5,stroke:#ea580c,color:#431407,stroke-width:2px
    classDef check fill:#fae8ff,stroke:#c026d3,color:#4a044e,stroke-width:2px
    classDef write fill:#ffe4e6,stroke:#e11d48,color:#4c0519,stroke-width:2px
    classDef free fill:#dcfce7,stroke:#16a34a,color:#052e16,stroke-width:3px
```

See the exact `rmdir` checks in
[Tutorial 2](02-paths-and-directories.md#remove-an-empty-directory-rmdir).
Rustyfile has no recursive delete: a learner can see each unlink and free
operation independently.

## Allocate an inode

Files and directories share one pool of 256 inode slots. Bit 0 belongs to root,
so allocation scans from 1:

Source: [`src/filesystem/disk.rs` — `allocate_inode`](../../src/filesystem/disk.rs#L133-L144)

```rust
let mut bitmap = self.read_block(INODE_BITMAP_BLOCK)?;
for inode in 1..INODE_COUNT {
    if !get_bit(&bitmap, inode) {
        set_bit(&mut bitmap, inode, true);
        self.write_block(INODE_BITMAP_BLOCK, &bitmap)?;
        return Ok(inode);
    }
}
Err(FsError::NoSpace)
```

```mermaid
flowchart LR
    READ["read inode bitmap<br/>block 1"]:::read
    SCAN["scan bits 1..255"]:::scan
    FREE{"zero bit found?"}:::decision
    SET["set bit to 1"]:::bitmap
    WRITE["write bitmap block 1"]:::write
    NUM["return inode number"]:::success
    NOSPACE["return NoSpace"]:::error

    READ --> SCAN --> FREE
    FREE -- yes --> SET --> WRITE --> NUM
    FREE -- no --> NOSPACE

    classDef read fill:#dbeafe,stroke:#2563eb,color:#172554,stroke-width:2px
    classDef scan fill:#ede9fe,stroke:#7c3aed,color:#2e1065,stroke-width:2px
    classDef decision fill:#fef3c7,stroke:#d97706,color:#451a03,stroke-width:2px
    classDef bitmap fill:#fae8ff,stroke:#c026d3,color:#4a044e,stroke-width:2px
    classDef write fill:#ffe4e6,stroke:#e11d48,color:#4c0519,stroke-width:2px
    classDef success fill:#dcfce7,stroke:#16a34a,color:#052e16,stroke-width:3px
    classDef error fill:#fee2e2,stroke:#dc2626,color:#450a0a,stroke-width:3px
```

`set_inode_allocated(inode, false)` performs the reverse during deletion:
read block 1, clear one bit, and write block 1.

## Allocate a data block

Block allocation scans the block bitmap from `DATA_BLOCK_START` (11), never
considering metadata blocks:

Source: [`src/filesystem/disk.rs` — `allocate_block`](../../src/filesystem/disk.rs#L153-L166)

```rust
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
```

```mermaid
flowchart TD
    BB["read block bitmap<br/>block 2"]:::read
    SCAN["scan bits 11..< total_blocks"]:::scan
    FOUND{"zero bit found?"}:::decision
    CLAIM["set bit = 1<br/>write bitmap"]:::bitmap
    ZERO["write 4096 zero bytes<br/>to claimed data block"]:::security
    RETURN["return block number"]:::success
    FULL["return NoSpace"]:::error

    BB --> SCAN --> FOUND
    FOUND -- yes --> CLAIM --> ZERO --> RETURN
    FOUND -- no --> FULL

    classDef read fill:#dbeafe,stroke:#2563eb,color:#172554,stroke-width:2px
    classDef scan fill:#ede9fe,stroke:#7c3aed,color:#2e1065,stroke-width:2px
    classDef decision fill:#fef3c7,stroke:#d97706,color:#451a03,stroke-width:2px
    classDef bitmap fill:#fae8ff,stroke:#c026d3,color:#4a044e,stroke-width:2px
    classDef security fill:#ffedd5,stroke:#ea580c,color:#431407,stroke-width:2px
    classDef success fill:#dcfce7,stroke:#16a34a,color:#052e16,stroke-width:3px
    classDef error fill:#fee2e2,stroke:#dc2626,color:#450a0a,stroke-width:3px
```

Claiming the bit precedes clearing the block. The filesystem is single-process
and has no concurrency control; the ordering makes ownership explicit but is
not a substitute for journaling.

## Free data blocks

`free_inode_contents` enumerates every nonzero direct pointer and clears its
block bit:

Source: [`src/filesystem/disk.rs` — `free_inode_contents`](../../src/filesystem/disk.rs#L125-L131)

```rust
pub(super) fn free_inode_contents(&mut self, inode: &Inode) -> Result<()> {
    for block in used_blocks(inode) {
        self.set_block_allocated(block, false)?;
    }
    Ok(())
}
```

Source: [`src/filesystem/disk.rs` — `used_blocks`](../../src/filesystem/disk.rs#L236-L244)

```rust
inode
    .direct
    .iter()
    .copied()
    .filter(|block| *block != 0)
    .collect()
```

The inode’s `size` is not used to decide which blocks to free. Every nonzero
pointer is considered owned. This is useful cleanup behavior, though a separate
integrity checker would be needed to reconcile badly corrupted pointers and
bitmaps safely.

## How one inode maps to the inode table

Inodes are 128 bytes and 32 fit in a 4096-byte block. The table starts at block
3:

Source: [`src/filesystem/disk.rs` — `read_inode`](../../src/filesystem/disk.rs#L175-L185)

```rust
let block = INODE_TABLE_START + inode / INODES_PER_BLOCK;
let offset = inode as usize % INODES_PER_BLOCK as usize * INODE_SIZE;
let bytes = self.read_block(block)?;
let raw: [u8; INODE_SIZE] =
    bytes[offset..offset + INODE_SIZE].try_into().unwrap();
Inode::decode(&raw)
```

Writing performs a read-modify-write of the containing block so adjacent inode
slots are preserved:

Source: [`src/filesystem/disk.rs` — `write_inode`](../../src/filesystem/disk.rs#L187-L194)

```rust
let block = INODE_TABLE_START + inode_number / INODES_PER_BLOCK;
let offset = inode_number as usize % INODES_PER_BLOCK as usize * INODE_SIZE;
let mut bytes = self.read_block(block)?;
bytes[offset..offset + INODE_SIZE].copy_from_slice(&inode.encode());
self.write_block(block, &bytes)
```

For inode 37:

```text
table block = 3 + (37 / 32) = 4
byte offset = (37 % 32) × 128 = 640
host offset  = 4 × 4096 + 640 = 17024
```

```mermaid
flowchart LR
    N["inode 37"]:::input
    DIV["37 / 32 = 1"]:::calc
    MOD["37 % 32 = 5"]:::calc
    BLOCK["table block<br/>3 + 1 = 4"]:::block
    OFFSET["slot offset<br/>5 × 128 = 640"]:::offset
    READ["read complete block 4"]:::read
    SLOT["bytes 640..768<br/>decode or replace"]:::inode
    WRITE["on mutation:<br/>write complete block 4"]:::write

    N --> DIV --> BLOCK --> READ --> SLOT --> WRITE
    N --> MOD --> OFFSET --> SLOT

    classDef input fill:#dbeafe,stroke:#2563eb,color:#172554,stroke-width:2px
    classDef calc fill:#fef3c7,stroke:#d97706,color:#451a03,stroke-width:2px
    classDef block fill:#ede9fe,stroke:#7c3aed,color:#2e1065,stroke-width:2px
    classDef offset fill:#fae8ff,stroke:#c026d3,color:#4a044e,stroke-width:2px
    classDef read fill:#e0f2fe,stroke:#0284c7,color:#082f49,stroke-width:2px
    classDef inode fill:#dcfce7,stroke:#16a34a,color:#052e16,stroke-width:2px
    classDef write fill:#ffe4e6,stroke:#e11d48,color:#4c0519,stroke-width:2px
```

## Failure behavior by operation

The code makes some local failure cases recoverable, but the filesystem is not
crash-safe:

| Operation | Protection in the implementation | Remaining limitation |
| --- | --- | --- |
| create file/directory | clears a reserved child inode (and any child blocks) if linking fails | no journal across multiple metadata writes |
| replace file contents | claims all new blocks before changing the inode; allocation failure clears partial claims | I/O failure or power loss can interrupt later writes |
| append | delegates to complete replacement, so allocation failure preserves old contents | reads and rewrites the whole file |
| rewrite directory | reuses existing blocks; newly allocated growth blocks are rolled back on allocation failure | retained blocks are written before inode commit |
| `rm` / `rmdir` | unlinks before freeing storage | interruption after unlink can leak storage |

```mermaid
flowchart LR
    subgraph GUARANTEED["ordinary allocation-failure behavior"]
        A["old regular-file inode"]:::kept
        B["try to claim all replacement blocks"]:::attempt
        C["NoSpace"]:::error
        D["clear partial new claims"]:::rollback
        E["old contents still reachable"]:::kept
        A --> B --> C --> D --> E
    end

    subgraph NOTGUARANTEED["power loss / arbitrary I/O failure"]
        W1["bitmap write"]:::write
        W2["data-block write"]:::write
        W3["inode write"]:::write
        X["interruption between writes<br/>may leak or partially update"]:::warning
        W1 --> W2 --> W3
        W1 -.-> X
        W2 -.-> X
    end

    classDef kept fill:#dcfce7,stroke:#16a34a,color:#052e16,stroke-width:2px
    classDef attempt fill:#dbeafe,stroke:#2563eb,color:#172554,stroke-width:2px
    classDef error fill:#fee2e2,stroke:#dc2626,color:#450a0a,stroke-width:2px
    classDef rollback fill:#cffafe,stroke:#0891b2,color:#164e63,stroke-width:2px
    classDef write fill:#ffe4e6,stroke:#e11d48,color:#4c0519,stroke-width:2px
    classDef warning fill:#fef3c7,stroke:#d97706,color:#451a03,stroke-width:3px
```

## End-to-end worked trace

Starting from a newly formatted image:

```console
cargo run -- disk.img mkdir /docs
cargo run -- disk.img write /docs/hello "abc"
cargo run -- disk.img append /docs/hello "def"
cargo run -- disk.img rm /docs/hello
cargo run -- disk.img rmdir /docs
```

```mermaid
sequenceDiagram
    participant ROOT as root inode 0
    participant DOCS as docs inode 1
    participant FILE as hello inode 2
    participant BM as allocation bitmaps
    participant DB as data blocks

    rect rgb(255, 237, 213)
        Note over ROOT,DOCS: mkdir /docs
        BM->>BM: claim inode 1 + a directory block
        DOCS->>DB: write '.' and '..'
        ROOT->>DB: add 'docs' → inode 1
    end
    rect rgb(219, 234, 254)
        Note over DOCS,FILE: write /docs/hello "abc"
        BM->>BM: claim inode 2
        DOCS->>DB: add 'hello' → inode 2
        BM->>BM: claim a file data block
        FILE->>DB: write "abc" + zero padding
    end
    rect rgb(237, 233, 254)
        Note over FILE,DB: append "def"
        DB-->>FILE: read "abc"
        BM->>BM: claim replacement block
        FILE->>DB: write "abcdef"
        BM->>BM: free old file block
    end
    rect rgb(254, 226, 226)
        Note over DOCS,FILE: rm /docs/hello
        DOCS->>DB: unlink 'hello'
        BM->>BM: free file block + inode 2
        Note over ROOT,DOCS: rmdir /docs
        ROOT->>DB: unlink 'docs'
        BM->>BM: free docs block + inode 1
    end
```

After the final command, the image is back to one allocated inode (root) and
the metadata plus root directory data blocks. The bytes in freed blocks are not
erased at deletion time, but any future allocator zeroes a block before handing
it to a new inode.

Return to the [complete operation index](../OPERATIONS.md), or consult the
[on-disk format reference](../FORMAT.md) for exact byte offsets.
