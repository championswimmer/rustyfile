# Why Rustyfile only works through its own shell

Rustyfile is intentionally a **toy filesystem for learning**, not an operating-system filesystem.
It stores a filesystem image inside an ordinary host file and exposes that image through the Rustyfile CLI and shell in `src/main.rs`.

This document explains:

1. why the current implementation only works through Rustyfile itself,
2. what is missing before Linux or macOS could use it like a normal filesystem,
3. what “POSIX-compatible” would require,
4. and a realistic roadmap from this repo to something more production-like.

## What Rustyfile is today

Today the repository contains:

- an on-disk format in `src/layout.rs` and `docs/FORMAT.md`,
- a library that can open and mutate that format in `src/filesystem/*`,
- and a thin shell/CLI in `src/main.rs`.

The important point is that **only Rustyfile knows how to interpret the image format**.
Linux, macOS, Finder, `ls`, `cp`, text editors, and other normal programs do not know this format and do not know how to send standard file operations to it.

## Why it only works with its own shell

### 1. There is no kernel or VFS integration

Operating systems do not talk to filesystems by calling arbitrary application code.
They use a filesystem interface provided by the kernel:

- Linux uses the VFS (Virtual Filesystem Switch) plus either a kernel filesystem driver or a FUSE daemon.
- macOS has its own VFS layer and in practice commonly uses FUSE-style integrations (for example macFUSE) for third-party filesystems.

Rustyfile currently has **none** of those adapters.
It is just a normal Rust program that reads and writes bytes in a disk image.

That means:

- `cargo run -- shell disk.img` works,
- but `mount disk.img /mnt/rustyfile` does not,
- and `ls /mnt/rustyfile`, `vim`, Finder, or any normal app cannot access Rustyfile paths.

### 2. The shell is the only syscall translation layer

The shell in `src/main.rs` translates user commands like:

- `mkdir foo`
- `write foo/bar hello`
- `cat foo/bar`

into direct library calls like:

- create directory
- resolve a path
- allocate blocks
- read bytes from an inode

A real OS filesystem must instead translate **system calls** such as:

- `open`
- `read`
- `write`
- `readdir`
- `rename`
- `unlink`
- `stat`
- `chmod`
- `truncate`

Rustyfile has no layer that accepts those OS requests.
Its shell is effectively a tiny custom API, not a filesystem mount interface.

### 3. The on-disk format is intentionally minimal

The current format is excellent for learning but far too small and incomplete for general use:

- fixed 4096-byte blocks,
- fixed inode count,
- tiny maximum image size,
- 12 direct block pointers only,
- no permissions,
- no ownership,
- no timestamps,
- no symlinks,
- no hard links,
- no special files,
- no journaling,
- no crash recovery,
- no checksums,
- no concurrent access control.

A Unix-like operating system expects much richer behavior than this.

### 4. There is no long-running mounted filesystem service

When a filesystem is mounted, the OS expects it to remain available as a service.
Rustyfile today is a short-lived CLI process.
It opens an image, performs one command or shell session, then exits.

A mounted filesystem instead needs:

- a long-running process or kernel module,
- stable request handling for many clients at once,
- caching rules,
- concurrency control,
- and correct behavior when many processes open the same file simultaneously.

### 5. There is no POSIX metadata or semantics

Even if an adapter existed, many normal programs would still fail because Rustyfile does not yet model the metadata and guarantees they expect.
For example:

- `stat` in Unix means much more than type/size/block count,
- file permissions must influence access checks,
- rename behavior has atomicity expectations,
- open file descriptors must keep working across many namespace changes,
- link counts matter,
- directory iteration and `.` / `..` behavior must follow OS rules,
- and many tools rely on modification times.

## What would let Linux or macOS use it like a normal filesystem?

There are two broad paths.

## Path A: add a FUSE adapter first

This is the most realistic next step.

FUSE lets a userspace program appear to the OS as a mounted filesystem.
Instead of writing a kernel driver immediately, you write a daemon that implements callbacks like:

- lookup
- getattr
- readdir
- open
- read
- write
- create
- mkdir
- unlink
- rmdir
- rename
- setattr

That daemon would call into the existing Rustyfile library.

### Why FUSE is the practical bridge

It lets you keep most of the educational code in userspace while making the filesystem feel normal to applications.
A FUSE adapter would allow commands like:

```bash
mkdir /mnt/rustyfile/projects
echo hello > /mnt/rustyfile/projects/note.txt
cat /mnt/rustyfile/projects/note.txt
```

without using the Rustyfile shell.

### What still has to change even with FUSE

FUSE solves the **mount interface** problem, but not the **filesystem semantics** problem.
The underlying library would still need major upgrades:

- richer inode metadata,
- stronger path semantics,
- larger-file support,
- better error behavior,
- concurrent access handling,
- crash consistency,
- and compatibility with common Unix expectations.

## Path B: write a real kernel filesystem

This is the “native” route in the strict sense, but it is much harder.

### Linux

On Linux, a truly native filesystem usually means a kernel module or upstream kernel filesystem implementation wired into the Linux VFS.
That requires:

- kernel-space code,
- kernel memory safety discipline,
- block-device integration,
- page cache integration,
- writeback behavior,
- locking and concurrency correctness,
- and deep knowledge of Linux kernel internals.

### macOS

For macOS, true native kernel filesystem development is even less attractive for hobby and learning projects.
In practice, third-party filesystems are usually exposed through FUSE-like mechanisms rather than private or tightly controlled kernel interfaces.

### Bottom line

For this repo, the realistic “usable by Linux or macOS” milestone is:

1. improve the Rustyfile library,
2. add a FUSE mount layer,
3. then harden semantics and format.

A kernel-native implementation should come much later, if ever.

## What “production-grade filesystem” means

“Production-grade” is not just “it mounts”.
It means the filesystem remains correct, recoverable, and performant under real workloads and failures.

At minimum, that usually includes the following areas.

## 1. Correctness and crash safety

The current code has no journal or copy-on-write transaction model.
Power loss at the wrong moment can leak blocks or leave partially updated metadata.

A production filesystem needs one of these strategies:

- **journaling / write-ahead log**: record intent before applying metadata changes,
- **copy-on-write transactions**: write new tree/metadata versions and flip pointers atomically,
- **ordered writes + recovery logic**: more limited but still deliberate crash semantics.

It also needs:

- recovery rules on mount,
- consistency checking (`fsck`-style repair or verification),
- and preferably metadata and/or data checksums.

## 2. Scalable data structures

Rustyfile uses fixed inode counts and twelve direct pointers.
That is simple and readable, but production filesystems need to scale much further.

Typical upgrades include:

- dynamically scalable inode allocation,
- extents instead of many direct pointers,
- indirect blocks or B-tree extent maps,
- directory indexing (hash trees, B-trees, etc.),
- free-space management beyond one tiny bitmap.

## 3. Real metadata

A real Unix filesystem inode generally needs much more than kind and byte length.
Typical fields include:

- mode bits (`rwx` permissions + file type bits),
- owner UID,
- group GID,
- link count,
- access/modification/change/birth timestamps,
- flags,
- device numbers for special files,
- extended attributes or ACL references.

Without this, many tools and OS features cannot work correctly.

## 4. Concurrency and locking

The current toy design is effectively single-process, single-writer oriented.
A production system must define behavior when:

- many processes read one file,
- one process writes while another reads,
- many directory operations happen concurrently,
- memory-mapped I/O interacts with buffered I/O,
- multiple threads race on rename/unlink/create.

That requires:

- inode locks,
- directory locks,
- allocation locks,
- transaction ordering,
- and careful deadlock avoidance.

## 5. Performance and caching

A mounted filesystem must perform well enough for ordinary application use.
That means thinking about:

- read-ahead,
- write buffering,
- page cache interaction,
- attribute and entry caching,
- reducing metadata seeks,
- batching updates,
- and avoiding full-directory scans for common lookups.

Today Rustyfile optimizes for clarity, not throughput or latency.
That is the correct choice for a teaching repo, but it is not production behavior.

## 6. Security and robustness

A production filesystem must defend itself against malformed on-disk state and hostile inputs.
That means:

- validating every on-disk structure carefully,
- rejecting impossible lengths/pointers/counts,
- avoiding integer overflow bugs,
- preventing path traversal bugs,
- controlling permissions correctly,
- and surviving corrupt images without memory unsafety or silent damage.

## 7. Maintenance features

Real filesystems also need operational support:

- format versioning and upgrade paths,
- diagnostics and debug tooling,
- online/offline repair tools,
- metrics,
- compatibility tests,
- soak testing under stress,
- and clear mount/recovery behavior.

## What would be required for POSIX compatibility?

“POSIX-compatible” can mean different levels of strictness, but at a practical level it means ordinary Unix software can use the filesystem and observe expected behavior.

## Core object types

Rustyfile currently supports only:

- regular files,
- directories.

POSIX-style environments also expect support for at least some of:

- symbolic links,
- hard links,
- FIFOs,
- Unix domain socket nodes,
- block devices,
- character devices.

A minimal user-facing POSIX-like filesystem can sometimes postpone special-device support, but symlinks and hard links become important quickly.

## Required metadata model

To behave like a Unix filesystem, inodes need fields for:

- file type,
- permission bits,
- UID/GID,
- link count,
- inode number stability,
- `atime`, `mtime`, `ctime` (and often `birthtime`),
- file size,
- block accounting,
- optional xattrs/ACLs.

## Required operations

You would need correct implementations of operations corresponding to:

- `open`, `close`
- `read`, `write`, `pread`, `pwrite`
- `lseek`
- `truncate`, `ftruncate`
- `stat`, `lstat`, `fstat`
- `mkdir`, `rmdir`
- `opendir`, `readdir`
- `rename`
- `link`, `unlink`
- `symlink`, `readlink`
- `chmod`, `fchmod`
- `chown`, `fchown`
- `utimensat` / timestamp updates
- `fsync`, `fdatasync`
- `access`

For a mountable filesystem, these are typically expressed through VFS/FUSE callbacks rather than direct libc entrypoints, but the semantics must still match what those syscalls expect.

## Semantic rules that matter a lot

Implementing the function names is not enough. The difficult part is matching the rules.

### Stable inode identity

An inode number should keep identifying the same object while it exists.
Programs cache inode numbers and use them to detect hard links or changes.

### Hard links

A file can have multiple directory entries pointing to the same inode.
Deleting one name should not destroy the file until the link count reaches zero and no open file handles still require it.

### Open-unlink behavior

On Unix, a file can be unlinked while still open.
Its directory entry disappears, but the file's data remains accessible through existing file descriptors until the last reference closes.
That requires reference/lifetime rules that Rustyfile does not yet have.

### Atomic rename

`rename(old, new)` has strong expectations: it should appear atomic to observers.
Many programs rely on “write temp file, fsync, rename over old file” as a safe update pattern.
Without correct rename semantics, many editors and package managers behave badly.

### Permissions and ownership

The filesystem must store and enforce access rules based on user and group identity.
Without permission checks, it is not POSIX compatible in practice.

### Timestamps

Many tools depend on `mtime` and `ctime`, build systems depend on timestamps, and sync tools compare them.

### Sparse files

Large Unix files may contain holes that read as zeroes without occupying physical blocks.
Supporting sparse files is not required for a first prototype, but many real applications assume the possibility.

### `fsync` and durability semantics

Applications such as databases need to know what `fsync` guarantees.
A production filesystem must define and honor durability boundaries.

## Linux-specific work

To work on Linux like a normal filesystem, you would typically do the following.

### Shorter path: FUSE on Linux

Build a daemon using a Rust FUSE binding and map VFS requests to the Rustyfile library.
Then:

- mount the image at a mountpoint,
- serve kernel requests from the daemon,
- implement Linux file attributes and error codes,
- test with normal tools (`cp`, `mv`, `rm`, editors, shells, build systems).

### Longer path: kernel-native Linux filesystem

Eventually you would need:

- a Linux kernel implementation of the on-disk format,
- page-cache integration,
- writeback and invalidation logic,
- kernel locking,
- mount options,
- superblock/inode/dentry operations,
- and recovery behavior on mount.

That is a separate project from this Rust userspace library.

## macOS-specific work

For macOS, the practical path is again a FUSE-style mount layer.
That would require:

- mapping Rustyfile objects to macOS file attributes,
- testing with Finder, Terminal, and common apps,
- handling macOS metadata expectations reasonably,
- and translating platform-specific edge cases.

True “native” integration on macOS is much less approachable for an educational repository and should not be the first target.

## A realistic roadmap from this repo

If the goal is to evolve Rustyfile while preserving its learning value, this is a sensible order.

## Stage 1: strengthen the current library

Before mounting anything, improve correctness inside `src/filesystem/*`.

Suggested additions:

1. **Richer inode metadata**
   - permissions/mode bits
   - UID/GID placeholders
   - timestamps
   - link count

2. **Bigger-file support**
   - indirect blocks or extents
   - larger max image sizes
   - better free-space accounting

3. **Safer updates**
   - transactional metadata updates
   - journal or copy-on-write strategy
   - recovery checks on open/mount

4. **Better validation**
   - reject corrupt images defensively
   - add invariant-checking tests

5. **Directory semantics**
   - rename
   - harder edge-case testing
   - maybe eventually hard links and symlinks

## Stage 2: add POSIX-shaped library operations

Introduce APIs that resemble filesystem/VFS operations more closely than the current shell commands.
For example:

- create with mode bits,
- lookup returning rich attributes,
- set attributes,
- rename atomically,
- link/unlink,
- symlink/readlink,
- truncate,
- fsync-like flush hooks.

This layer becomes the bridge between the toy implementation and a future mount adapter.

## Stage 3: build a FUSE adapter

Add a new binary that mounts a Rustyfile image.
Its job is to convert kernel requests into library calls.

At that point, normal host tools could interact with the mounted image.
This is the first milestone where the filesystem starts feeling “real” to users.

## Stage 4: harden for real workloads

After it mounts, test behavior with:

- concurrent file access,
- large directory trees,
- repeated crash/recovery scenarios,
- common Unix tools,
- editors that use temp-file + rename workflows,
- build tools,
- extraction of tarballs,
- recursive delete/copy operations.

Also add:

- fuzzing for on-disk parsing,
- stress tests,
- corruption tests,
- performance measurement.

## Stage 5: consider native-kernel implementations only if needed

Once the format and semantics are stable, a Linux kernel implementation could be considered.
But that would no longer be a small extension of this repo; it would be a major systems project.

## What this repo is best for right now

Rustyfile is already very good at teaching:

- block layout,
- allocation bitmaps,
- inode-based naming,
- directories as serialized data,
- path resolution,
- and the gap between “a filesystem format” and “an OS-integrated filesystem”.

That last point is important: a filesystem is not only bytes on disk. It is also:

- an API contract with the operating system,
- metadata semantics,
- recovery rules,
- concurrency rules,
- and durability guarantees.

Rustyfile intentionally stops before those layers so the core ideas remain easy to read.

## Short version

Rustyfile only works through its own shell because:

- only Rustyfile understands the on-disk format,
- there is no FUSE or kernel mount adapter,
- there is no POSIX metadata model,
- and there are no production-grade crash, concurrency, or security guarantees.

To make it usable by Linux or macOS like a normal filesystem, the most practical path is:

1. enrich the library semantics,
2. add real inode metadata and safer update rules,
3. implement POSIX-like operations,
4. build a FUSE mount adapter,
5. then harden correctness, recovery, and performance.

A truly native kernel filesystem is possible in principle, but it is a much larger project than evolving this learning-oriented repo.
