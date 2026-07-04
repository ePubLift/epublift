//! Integration tests for the `epublift check` subcommand (the epubveri
//! validation wiring). These exercise OUR surface — argument parsing, the
//! grep-style exit code, and the JSON shape — without pinning to epubveri's
//! exact rule set (which is pre-1.0 and evolves), so they stay robust across
//! validator upgrades.
#![cfg(feature = "validate")]

use std::process::Command;

/// The `epublift` binary built for this test run (with `--features validate`).
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_epublift")
}

/// Write `bytes` to a temp file with the given name and return its path.
fn tmp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("epublift-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn malformed_epub_fails_with_exit_1() {
    // Not a ZIP at all — epubveri can't open it, so the book is invalid and the
    // command must exit non-zero (grep-style: ran fine, found problems).
    let path = tmp_file("not-a-real.epub", b"this is plainly not an epub");
    let out = Command::new(bin())
        .arg("check")
        .arg(&path)
        .output()
        .expect("run epublift check");

    assert_eq!(out.status.code(), Some(1), "invalid EPUB should exit 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("FAIL") || stdout.contains("ERROR"),
        "human report should mark the file as failing; got:\n{stdout}"
    );
}

#[test]
fn json_output_is_wellformed_and_flags_invalid() {
    let path = tmp_file("bad-for-json.epub", b"still not an epub");
    let out = Command::new(bin())
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .expect("run epublift check --json");

    // Non-zero exit even in JSON mode.
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let trimmed = stdout.trim();
    assert!(
        trimmed.starts_with('['),
        "JSON should be an array: {trimmed}"
    );
    assert!(trimmed.ends_with(']'));
    assert!(
        trimmed.contains("\"valid\": false"),
        "malformed EPUB should be reported invalid in JSON; got:\n{trimmed}"
    );
    // The file path we passed must round-trip into the report.
    assert!(trimmed.contains("bad-for-json.epub"));
}

#[test]
fn missing_file_is_reported_not_panicked() {
    let out = Command::new(bin())
        .arg("check")
        .arg("/no/such/path/definitely-missing.epub")
        .output()
        .expect("run epublift check on a missing file");

    // A file that can't even be read is still a batch failure (exit 1), and the
    // process must not panic (which would surface as signal-based termination).
    assert_eq!(out.status.code(), Some(1));
}
