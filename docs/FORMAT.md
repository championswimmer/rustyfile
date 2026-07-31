# Rustyfile on-disk format, version 1

This document describes what is physically inside a Rustyfile image. All
integers use little-endian byte order. Blocks are 4096 bytes and block numbers
start at zero.

For instructional traces showing exactly how commands manipulate these
structures, including inline Rust excerpts and colored Mermaid diagrams, start
with [How Rustyfile operations execute](OPERATIONS.md).

## Whole-image map

```text
block 0        superblock
block 1        inode allocation bitmap
block 2        block allocation bitmap
blocks 3..10   inode table (256 inodes × 128 bytes)
blocks 11..N   directory and regular-file data
```

A bitmap stores one allocation state per bit. If bit 14 of the block bitmap is
one, block 14 is in use. The bitmap is itself metadata, so bits 0 through 10 are
always one.

One 4096-byte block bitmap has 32768 bits. Multiplying by the block size gives
the format's 128 MiB maximum image size.

## Superblock: block 0

The superblock tells the reader which format it has opened and where the fixed
regions live.

| Byte offset | Width | Value |
| ---: | ---: | --- |
| 0 | 8 | ASCII `RUSTYFS` followed by zero |
| 8 | 4 | format version (`1`) |
| 12 | 4 | block size (`4096`) |
| 16 | 4 | total blocks in this particular image |
| 20 | 4 | inode count (`256`) |
| 24 | 4 | inode bitmap block (`1`) |
| 28 | 4 | block bitmap block (`2`) |
| 32 | 4 | first inode-table block (`3`) |
| 36 | 4 | first data block (`11`) |
| 40 | 4 | root inode number (`0`) |
| 44..4095 | | reserved, currently zero |

## Inode bitmap: block 1

Each bit represents one of 256 inode slots. Formatting reserves bit 0 for the
root directory. Creating a file or directory finds the first zero bit and sets
it. Removing that object clears the bit.

## Block bitmap: block 2

Each bit represents a block in the image. Formatting reserves blocks 0 through
10 for metadata, then allocates a data block for the root directory.

## Inode table: blocks 3 through 10

An inode is a file's identity and metadata. Its name is *not* here; names live in
the parent directory. Renaming a file in a future version could therefore
change its directory entry without moving the file data.

Each inode is 128 bytes:

| Byte offset | Width | Meaning |
| ---: | ---: | --- |
| 0 | 1 | kind: `1` regular file, `2` directory |
| 1..7 | | reserved |
| 8 | 8 | exact byte length |
| 16 | 48 | twelve 4-byte direct data-block numbers |
| 64..127 | | reserved |

Thirty-two inodes fit in one block. Inode `i` therefore lives in:

```text
table block = 3 + (i / 32)
byte offset = (i % 32) * 128
```

The twelve direct pointers are the main simplification in this project. A
48 KiB file uses all twelve. Production filesystems add indirect trees or
extents so an inode can address much more data.

## Directory data

A directory is an inode whose data blocks contain an array of 64-byte entries:

| Byte offset | Width | Meaning |
| ---: | ---: | --- |
| 0 | 4 | child inode number |
| 4 | 1 | child kind |
| 5 | 1 | used flag (`1`) |
| 6 | 1 | filename byte length |
| 7 | 1 | reserved |
| 8 | 56 | UTF-8 filename followed by zero space |

Every directory begins with `.` pointing at itself and `..` pointing at its
parent. Path traversal needs no special in-memory parent map: looking up `..`
works like any other directory lookup. `ls` merely hides these two entries.

## Worked example

After:

```text
mkdir /docs
write /docs/hello "abc"
```

the relationships look like:

```text
root inode 0 (directory)
└── directory entry "docs" ──> inode 1 (directory)
                              └── entry "hello" ──> inode 2 (file)
                                                       │
                                                       └── direct[0] ──> data block containing "abc"
```

Names lead to inode numbers; inodes lead to data blocks. That two-step lookup is
the central idea behind an inode filesystem.

## Updating and deleting

When replacing a file, Rustyfile allocates and writes new blocks before
releasing the old blocks. An ordinary out-of-space error therefore preserves the
old contents. This is not full crash safety: power loss between metadata writes
can still leak blocks or leave partial state because there is no journal.

Deleting a regular file removes its directory entry, clears its data-block bits,
and clears its inode bit. A directory can be deleted only when it contains
nothing except `.` and `..`.
