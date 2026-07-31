//! Command-line adapter around the filesystem library.

use rustyfile::layout::{FileKind, BLOCK_SIZE, MAX_FILE_SIZE, ROOT_INODE};
use rustyfile::{FileSystem, FsError, Result};
use std::env;
use std::io::{self, BufRead, Write};

/// Print CLI errors once and return a conventional failure status.
fn main() {
    if let Err(error) = run() {
        eprintln!("rustyfile: {error}");
        std::process::exit(1);
    }
}

/// Dispatch formatting, an interactive shell, or a one-shot command.
fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        print_usage();
        return Ok(());
    }

    // The first argument selects lifecycle mode or names an image.
    match args.remove(0).as_str() {
        "mkfs" => command_mkfs(&args),
        "shell" => {
            let image = exactly_one(&args, "shell <image>")?;
            command_shell(image)
        }
        image => {
            if args.is_empty() {
                return Err(FsError::InvalidPath(
                    "missing command; try `rustyfile --help`".into(),
                ));
            }
            let mut fs = FileSystem::open(image)?;
            let command = args.remove(0);
            let mut cwd = ROOT_INODE;
            let mut cwd_path = "/".to_owned();
            execute(&mut fs, &mut cwd, &mut cwd_path, &command, &args)?;
            fs.sync()
        }
    }
}

/// Format a new image or an already-sized host file.
fn command_mkfs(args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(FsError::InvalidPath(
            "usage: rustyfile mkfs <image> [--size 100M]".into(),
        ));
    }
    let image = &args[0];
    // No size means the image must already exist and be block-aligned.
    let size = match &args[1..] {
        [] => None,
        [flag, value] if flag == "--size" => Some(parse_size(value)?),
        _ => {
            return Err(FsError::InvalidPath(
                "usage: rustyfile mkfs <image> [--size 100M]".into(),
            ))
        }
    };
    let info = FileSystem::format(image, size)?.info()?;
    println!(
        "formatted {image}: {} blocks × {} bytes ({} MiB), {} inodes",
        info.total_blocks,
        BLOCK_SIZE,
        info.total_blocks as u64 * BLOCK_SIZE as u64 / 1024 / 1024,
        info.total_inodes
    );
    Ok(())
}

/// Keep one image and current directory open across shell commands.
fn command_shell(image: &str) -> Result<()> {
    let mut fs = FileSystem::open(image)?;
    let mut cwd = ROOT_INODE;
    let mut cwd_path = "/".to_owned();
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let interactive = std::io::IsTerminal::is_terminal(&stdin);

    if interactive {
        println!("Rustyfile shell. Type `help` for commands.");
    }
    // A terminal gets prompts; redirected scripts receive clean output.
    loop {
        if interactive {
            print!("rustyfile:{cwd_path}$ ");
            io::stdout().flush()?;
        }
        let Some(line) = lines.next() else {
            break;
        };
        let line = line?;
        // Parse quotes before dispatching the command.
        let words = match split_command_line(&line) {
            Ok(words) => words,
            Err(message) => {
                eprintln!("error: {message}");
                continue;
            }
        };
        if words.is_empty() {
            continue;
        }
        let command = &words[0];
        if matches!(command.as_str(), "exit" | "quit") {
            break;
        }
        // Command errors are recoverable and do not close the shell.
        if let Err(error) = execute(&mut fs, &mut cwd, &mut cwd_path, command, &words[1..]) {
            eprintln!("error: {error}");
        }
    }
    fs.sync()
}

/// Execute one shell command against the current directory.
fn execute(
    fs: &mut FileSystem,
    cwd: &mut u32,
    cwd_path: &mut String,
    command: &str,
    args: &[String],
) -> Result<()> {
    // This layer handles text and host files; storage stays in the library.
    match command {
        "help" => print_shell_help(),
        "pwd" => {
            require_count(args, 0, "pwd")?;
            println!("{cwd_path}");
        }
        "cd" => {
            let path = exactly_one(args, "cd <directory>")?;
            let inode_number = fs.resolve_path(*cwd, path)?;
            let stat = fs.stat(*cwd, path)?;
            if stat.kind != FileKind::Directory {
                return Err(FsError::NotDirectory(path.into()));
            }
            *cwd = inode_number;
            *cwd_path = normalize_display_path(cwd_path, path);
        }
        "ls" | "dir" => {
            let path = zero_or_one(args, "ls [directory]")?.unwrap_or(".");
            for entry in fs.list_dir(*cwd, path)? {
                let marker = if entry.kind == FileKind::Directory {
                    "/"
                } else {
                    ""
                };
                println!("{:>8}  {}{}", entry.size, entry.name, marker);
            }
        }
        "mkdir" => {
            let path = exactly_one(args, "mkdir <directory>")?;
            fs.create_dir(*cwd, path)?;
        }
        "touch" => {
            let path = exactly_one(args, "touch <file>")?;
            match fs.stat(*cwd, path) {
                Ok(stat) if stat.kind == FileKind::File => {}
                Ok(_) => return Err(FsError::IsDirectory(path.into())),
                Err(FsError::NotFound(_)) => {
                    fs.create_file(*cwd, path)?;
                }
                Err(error) => return Err(error),
            }
        }
        "write" => {
            if args.len() < 2 {
                return Err(FsError::InvalidPath("usage: write <file> <text>".into()));
            }
            fs.write_file(*cwd, &args[0], args[1..].join(" ").as_bytes())?;
        }
        "append" => {
            if args.len() < 2 {
                return Err(FsError::InvalidPath("usage: append <file> <text>".into()));
            }
            fs.append_file(*cwd, &args[0], args[1..].join(" ").as_bytes())?;
        }
        "cat" => {
            let path = exactly_one(args, "cat <file>")?;
            let bytes = fs.read_file(*cwd, path)?;
            io::stdout().write_all(&bytes)?;
            if !bytes.ends_with(b"\n") {
                println!();
            }
        }
        "rm" => {
            let path = exactly_one(args, "rm <file>")?;
            fs.remove_file(*cwd, path)?;
        }
        "rmdir" => {
            let path = exactly_one(args, "rmdir <empty-directory>")?;
            let target = fs.resolve_path(*cwd, path)?;
            if target == *cwd {
                return Err(FsError::InvalidPath(
                    "cannot remove the current directory".into(),
                ));
            }
            fs.remove_dir(*cwd, path)?;
        }
        "stat" => {
            let path = exactly_one(args, "stat <path>")?;
            let stat = fs.stat(*cwd, path)?;
            println!("inode:  {}", stat.inode);
            println!("type:   {}", stat.kind.as_str());
            println!("size:   {} bytes", stat.size);
            println!("blocks: {}", stat.blocks);
        }
        "info" => {
            require_count(args, 0, "info")?;
            let info = fs.info()?;
            println!(
                "blocks: {}/{} used ({} KiB blocks)",
                info.used_blocks,
                info.total_blocks,
                BLOCK_SIZE / 1024
            );
            println!("inodes: {}/{} used", info.used_inodes, info.total_inodes);
            println!("maximum file size: {} KiB", MAX_FILE_SIZE / 1024);
        }
        "put" => {
            if args.len() != 2 {
                return Err(FsError::InvalidPath(
                    "usage: put <host-file> <filesystem-path>".into(),
                ));
            }
            let bytes = std::fs::read(&args[0])?;
            fs.write_file(*cwd, &args[1], &bytes)?;
        }
        "get" => {
            if args.len() != 2 {
                return Err(FsError::InvalidPath(
                    "usage: get <filesystem-path> <host-file>".into(),
                ));
            }
            let bytes = fs.read_file(*cwd, &args[0])?;
            std::fs::write(&args[1], bytes)?;
        }
        "exit" | "quit" => {}
        _ => {
            return Err(FsError::InvalidPath(format!(
                "unknown command `{command}`; try `help`"
            )))
        }
    }
    Ok(())
}

/// Parse bytes or a binary K/M/G size suffix.
fn parse_size(text: &str) -> Result<u64> {
    let (number, multiplier) = match text.as_bytes().last().copied() {
        Some(b'K' | b'k') => (&text[..text.len() - 1], 1024_u64),
        Some(b'M' | b'm') => (&text[..text.len() - 1], 1024_u64 * 1024),
        Some(b'G' | b'g') => (&text[..text.len() - 1], 1024_u64 * 1024 * 1024),
        _ => (text, 1),
    };
    let value = number
        .parse::<u64>()
        .map_err(|_| FsError::InvalidPath(format!("invalid size: {text}")))?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| FsError::InvalidPath(format!("size is too large: {text}")))
}

/// Normalize a successful `cd` path for the prompt.
fn normalize_display_path(cwd: &str, path: &str) -> String {
    let combined = if path.starts_with('/') {
        path.to_owned()
    } else if cwd == "/" {
        format!("/{path}")
    } else {
        format!("{cwd}/{path}")
    };
    // A stack makes `..` remove exactly one prior component.
    let mut parts = Vec::new();
    for part in combined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            component => parts.push(component),
        }
    }
    if parts.is_empty() {
        "/".into()
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Enough shell parsing for paths/text with spaces: single quotes, double
/// quotes, and backslash escapes. Environment expansion is intentionally absent.
fn split_command_line(line: &str) -> std::result::Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut word_started = false;
    // Scan once while tracking the active quote and escape state.
    for character in line.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            word_started = true;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            word_started = true;
            continue;
        }
        if let Some(mark) = quote {
            if character == mark {
                quote = None;
            } else {
                word.push(character);
            }
            word_started = true;
        } else if character == '\'' || character == '"' {
            quote = Some(character);
            word_started = true;
        } else if character.is_whitespace() {
            if word_started {
                words.push(std::mem::take(&mut word));
                word_started = false;
            }
        } else {
            word.push(character);
            word_started = true;
        }
    }
    if escaped {
        return Err("line ends with an unfinished escape".into());
    }
    if quote.is_some() {
        return Err("line has an unclosed quote".into());
    }
    if word_started {
        words.push(word);
    }
    Ok(words)
}

/// Require exactly one argument and return it.
fn exactly_one<'a>(args: &'a [String], usage: &str) -> Result<&'a str> {
    require_count(args, 1, usage)?;
    Ok(&args[0])
}

/// Accept an optional single argument.
fn zero_or_one<'a>(args: &'a [String], usage: &str) -> Result<Option<&'a str>> {
    if args.len() > 1 {
        return Err(FsError::InvalidPath(format!("usage: {usage}")));
    }
    Ok(args.first().map(String::as_str))
}

/// Produce a consistent usage error for the wrong argument count.
fn require_count(args: &[String], count: usize, usage: &str) -> Result<()> {
    if args.len() != count {
        return Err(FsError::InvalidPath(format!("usage: {usage}")));
    }
    Ok(())
}

/// Print top-level invocation help.
fn print_usage() {
    println!(
        "\
rustyfile — a tiny filesystem in one block file

USAGE
  rustyfile mkfs <image> [--size 100M]  Format a new or existing image
  rustyfile shell <image>               Start the interactive shell
  rustyfile <image> <command> [args]    Run one command

EXAMPLES
  rustyfile mkfs disk.img --size 100M
  rustyfile shell disk.img
  rustyfile disk.img mkdir /docs
  rustyfile disk.img write /docs/hello.txt \"hello from blocks\"
  rustyfile disk.img cat /docs/hello.txt"
    );
}

/// Print commands available inside an opened image.
fn print_shell_help() {
    println!(
        "\
Navigation:  pwd, cd <dir>, ls [dir], dir [dir]
Create:      mkdir <dir>, touch <file>
Data:        write <file> <text>, append <file> <text>, cat <file>
Host copy:   put <host-file> <fs-path>, get <fs-path> <host-file>
Remove:      rm <file>, rmdir <empty-dir>
Inspect:     stat <path>, info
Shell:       help, exit, quit"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quoting preserves spaces without retaining quote characters.
    #[test]
    fn shell_words_support_quotes_and_escapes() {
        assert_eq!(
            split_command_line(r#"write "my file" 'hello world'"#).unwrap(),
            ["write", "my file", "hello world"]
        );
        assert_eq!(
            split_command_line(r#"write a hello\ world"#).unwrap(),
            ["write", "a", "hello world"]
        );
    }

    /// Prompt paths collapse dot components and cannot move above root.
    #[test]
    fn paths_are_normalized_for_prompt() {
        assert_eq!(normalize_display_path("/one/two", "../three"), "/one/three");
        assert_eq!(normalize_display_path("/", "../../"), "/");
        assert_eq!(
            normalize_display_path("/one", "/absolute/./x"),
            "/absolute/x"
        );
    }

    /// Human-readable sizes use powers of 1024.
    #[test]
    fn human_sizes_are_binary() {
        assert_eq!(parse_size("100M").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_size("4096").unwrap(), 4096);
    }
}
