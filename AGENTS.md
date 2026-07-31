# AGENTS.md

## Purpose

This repository is a toy filesystem written in Rust for learning how filesystem internals work. The code models a complete but intentionally small stack: image formatting, block I/O, allocation bitmaps, inode metadata, directories, path traversal, and regular-file reads/writes.

This file is for coding agents. Prefer it for repository navigation and implementation context; use `README.md` for user-facing usage and walkthrough material.

## Working guidance for agents

- Keep the implementation educational and readable over cleverness.
- Preserve the layer boundaries:
  - `layout.rs` defines the on-disk format and byte encoding.
  - `src/filesystem/disk.rs` owns raw block/inode I/O and allocation.
  - `src/filesystem/directory.rs` owns pathname resolution and namespace updates.
  - `src/filesystem/file.rs` owns regular-file operations.
  - `src/main.rs` should stay a thin CLI/shell adapter over the library.
- If changing the disk format, update both `src/layout.rs` and `docs/FORMAT.md` together.
- Prefer adding behavior through the library (`src/filesystem/*`) and keep CLI-specific parsing/output in `src/main.rs`.
- Generated build artifacts live under `target/`; do not edit them.

## Repo map

### Root

- `Cargo.toml` — crate metadata plus library/binary targets.
- `Cargo.lock` — lockfile.
- `README.md` — user-facing introduction, commands, reading order, and limits.
- `AGENTS.md` — agent-oriented repo guide.
- `docs/FORMAT.md` — byte-level description of the on-disk image format.

### Source tree

- `src/lib.rs` — public crate surface; re-exports `FileSystem`, info structs, and errors.
- `src/main.rs` — CLI entrypoint and interactive shell.
- `src/layout.rs` — format constants and encoding/decoding for superblocks, inodes, and directory entries.

### Filesystem implementation

- `src/filesystem/mod.rs` — `FileSystem` type, image open/format lifecycle, shared structs, module wiring.
- `src/filesystem/disk.rs` — block reads/writes, inode table access, bitmap manipulation, allocation/freeing, and inode data replacement helpers.
- `src/filesystem/directory.rs` — path resolution, directory listing/stat, create/remove directory flow, parent lookup, and directory serialization.
- `src/filesystem/file.rs` — create/read/write/append/remove operations for regular files.
- `src/filesystem/error.rs` — shared `FsError` enum and `Result` alias.
- `src/filesystem/tests.rs` — unit/integration-style library behavior tests for persistence, traversal, deletion, full-image behavior, and size limits.

### Tests

- `tests/cli_workflow.rs` — end-to-end binary test covering `mkfs`, one-shot commands, shell mode, host-file import/export, persistence, and cleanup.

### Generated output

- `target/` — Cargo build output, rustdoc output, and compiled binaries/libraries.

## High-value places to inspect before editing

- For anything format-related: `src/layout.rs`, then `docs/FORMAT.md`.
- For allocation or corruption behavior: `src/filesystem/disk.rs`.
- For path or directory semantics: `src/filesystem/directory.rs`.
- For file-content behavior: `src/filesystem/file.rs`.
- For CLI behavior or command wording: `src/main.rs` and `tests/cli_workflow.rs`.

## Validation

Run these after changes:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```
