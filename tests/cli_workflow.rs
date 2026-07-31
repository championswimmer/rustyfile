use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_dir() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("rustyfile-cli-{unique}"))
}

fn run(binary: &str, directory: &Path, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap()
}

fn assert_success(output: Output) -> String {
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn complete_100_mib_cli_workflow_persists() {
    let binary = env!("CARGO_BIN_EXE_rustyfile");
    let directory = unique_dir();
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("host-input.bin"), b"\0binary\nbytes\xff").unwrap();

    let formatted = assert_success(run(
        binary,
        &directory,
        &["mkfs", "disk.img", "--size", "100M"],
    ));
    assert!(formatted.contains("100 MiB"));
    assert_eq!(
        fs::metadata(directory.join("disk.img")).unwrap().len(),
        100 * 1024 * 1024
    );

    assert_success(run(binary, &directory, &["disk.img", "mkdir", "/docs"]));
    assert_success(run(
        binary,
        &directory,
        &["disk.img", "write", "/docs/hello.txt", "hello world"],
    ));
    assert_success(run(
        binary,
        &directory,
        &["disk.img", "put", "host-input.bin", "/docs/copied.bin"],
    ));

    let listing = assert_success(run(binary, &directory, &["disk.img", "ls", "/docs"]));
    assert!(listing.contains("hello.txt"));
    assert!(listing.contains("copied.bin"));
    assert_eq!(
        assert_success(run(
            binary,
            &directory,
            &["disk.img", "cat", "/docs/hello.txt"],
        )),
        "hello world\n"
    );

    let mut shell = Command::new(binary)
        .args(["shell", "disk.img"])
        .current_dir(&directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    shell
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"cd /docs\npwd\nappend hello.txt \" from shell\"\ncat hello.txt\ncd ..\nrm docs/hello.txt\nexit\n",
        )
        .unwrap();
    let output = shell.wait_with_output().unwrap();
    let stdout = assert_success(output);
    assert!(stdout.contains("/docs"));
    assert!(stdout.contains("hello world from shell"));

    // Reopening in a later process proves changes live in the block image.
    let listing = assert_success(run(binary, &directory, &["disk.img", "dir", "/docs"]));
    assert!(!listing.contains("hello.txt"));
    assert!(listing.contains("copied.bin"));
    assert_success(run(
        binary,
        &directory,
        &["disk.img", "get", "/docs/copied.bin", "host-output.bin"],
    ));
    assert_eq!(
        fs::read(directory.join("host-output.bin")).unwrap(),
        b"\0binary\nbytes\xff"
    );
    assert_success(run(
        binary,
        &directory,
        &["disk.img", "rm", "/docs/copied.bin"],
    ));
    assert_success(run(binary, &directory, &["disk.img", "rmdir", "/docs"]));

    fs::remove_dir_all(directory).unwrap();
}

use std::io::Write;
