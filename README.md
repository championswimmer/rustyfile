# Rustyfile

Rustyfile is a small filesystem written in Rust for learning. Its entire disk is
one ordinary host file, split into 4096-byte blocks. It implements its own
superblock, free-space bitmaps, inode table, directories, path lookup, and file
data allocation.

It is deliberately **not** a production filesystem and it does not require root
or a kernel driver. “Mounting” an image means opening it with the Rustyfile shell.
That makes the interesting filesystem code portable and easy to step through.

Rustyfile uses only the Rust standard library.

## Try it

You need a current stable Rust toolchain.

```console
cargo build
cargo run -- mkfs disk.img --size 100M
cargo run -- shell disk.img
```

Inside the shell:

```console
rustyfile:/$ mkdir projects
rustyfile:/$ cd projects
rustyfile:/projects$ write hello.txt "hello from my filesystem"
rustyfile:/projects$ ls
      24  hello.txt
rustyfile:/projects$ cat hello.txt
hello from my filesystem
rustyfile:/projects$ stat hello.txt
inode:  2
type:   file
size:   24 bytes
blocks: 1
rustyfile:/projects$ cd ..
rustyfile:/$ rm projects/hello.txt
rustyfile:/$ rmdir projects
rustyfile:/$ exit
```

The same operations work as one-shot commands, which is useful in scripts:

```console
cargo run -- disk.img mkdir /notes
cargo run -- disk.img write /notes/todo "understand inode allocation"
cargo run -- disk.img cat /notes/todo
cargo run -- disk.img dir /notes
```

You can also format a block file you created yourself:

```console
truncate -s 100M disk.img
cargo run -- mkfs disk.img
```

Formatting destroys the old filesystem metadata in that image.

## Commands

| Command | Meaning |
| --- | --- |
| `pwd` | Show the current shell directory |
| `cd DIR` | Change the current shell directory |
| `ls [DIR]`, `dir [DIR]` | List a directory |
| `mkdir DIR` | Create a directory |
| `touch FILE` | Create an empty file |
| `write FILE TEXT` | Create or replace a file |
| `append FILE TEXT` | Add bytes to the end of a file |
| `cat FILE` | Read a file |
| `rm FILE` | Remove a file |
| `rmdir DIR` | Remove an empty directory |
| `put HOST_FILE FS_PATH` | Copy a host file into the image |
| `get FS_PATH HOST_FILE` | Copy a file out of the image |
| `stat PATH` | Show an inode's type, size, and block count |
| `info` | Show image allocation statistics |
| `help`, `exit` | Shell help and exit |

Paths can be absolute or relative and support `.` and `..`. Quote shell
arguments containing spaces. File contents are arbitrary bytes when copied with
`put` and `get`; `cat` and `write` are the text-oriented conveniences.

## How a write reaches the image

For `write /notes/a.txt hello`, Rustyfile does roughly this:

1. Start at root inode 0 and look up `notes` in its directory entries.
2. Look up `a.txt` in the `notes` inode. If absent, claim a free inode bit and
   add a directory entry.
3. Calculate how many 4096-byte blocks are needed.
4. Find zero bits in the block bitmap and turn them on.
5. Write the bytes into those blocks.
6. Store the block numbers and byte length in the file's inode.
7. Release the file's old blocks, if this replaced existing contents.

For source-led, step-by-step explanations of every operation—including inline
implementation excerpts and colored diagrams—start with
[`docs/OPERATIONS.md`](docs/OPERATIONS.md). Its tutorials trace:

- image formatting, opening, flushing, and allocation statistics;
- path resolution, directory reads, `mkdir`, `rmdir`, `cd`, and `stat`;
- `touch`, exact-byte reads, replacement writes, append, import, and export;
- clearing file contents, unlinking, bitmap allocation, and failure ordering.

Read these files in order:

1. [`docs/OPERATIONS.md`](docs/OPERATIONS.md) — tutorial index and complete command-to-code map.
2. [`src/layout.rs`](src/layout.rs) — constants and byte encoding for structures.
3. [`src/filesystem/mod.rs`](src/filesystem/mod.rs) — image lifecycle and module map.
4. [`src/filesystem/disk.rs`](src/filesystem/disk.rs) — block I/O and allocation.
5. [`src/filesystem/directory.rs`](src/filesystem/directory.rs) — paths and directories.
6. [`src/filesystem/file.rs`](src/filesystem/file.rs) — regular-file operations.
7. [`src/main.rs`](src/main.rs) — the thin command-line shell.
8. [`docs/FORMAT.md`](docs/FORMAT.md) — a byte-level map and worked example.
9. [`docs/SHELL_ONLY_AND_PRODUCTION_PATH.md`](docs/SHELL_ONLY_AND_PRODUCTION_PATH.md) — why Rustyfile only works through its own shell today, plus what would be required for POSIX compatibility and a production-grade mountable filesystem.

Run the tests while experimenting:

```console
cargo test
cargo clippy --all-targets -- -D warnings
```

## Intentional limits

Keeping the implementation readable requires visible tradeoffs:

- Images are at most 128 MiB; the suggested 100 MiB image is fully supported.
- There are 256 inodes, so at most 255 non-root files/directories exist.
- An inode has 12 direct block pointers, making a file at most 48 KiB.
- Names are UTF-8 and at most 55 bytes.
- Directories are ordinary inode data containing fixed-size entries.
- There are no permissions, timestamps, hard links, sparse files, symlinks,
  concurrent access, cache, journal, or crash recovery.
- Only the Rustyfile program understands this format; it cannot be mounted by
  the operating system as a POSIX filesystem.

Those omissions are good next exercises. A natural progression is an indirect
block (larger files), then an integrity checker, then a write-ahead journal, and
finally a FUSE adapter.
