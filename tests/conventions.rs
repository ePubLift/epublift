//! veripublica conventions (CLI.md v0.7) on the command line: help, repeated
//! flags, `--format`, and the output-equals-input refusal, plus the warnings
//! for the spellings that go in the next major release.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_epublift"))
        .args(args)
        .output()
        .expect("run epublift");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A minimal EPUB 3, written to `dir`.
fn tiny_epub(dir: &Path) -> PathBuf {
    let path = dir.join("tiny.epub");
    let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();
    zip.start_file("META-INF/container.xml", stored).unwrap();
    zip.write_all(
        br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
    )
    .unwrap();
    zip.start_file("content.opf", stored).unwrap();
    zip.write_all(
        br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:uuid:1</dc:identifier><dc:title>Tiny Book</dc:title><dc:language>en</dc:language></metadata><manifest/><spine/></package>"#,
    )
    .unwrap();
    zip.finish().unwrap();
    path
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

// §5: `-h` / `--help` anywhere wins over a malformed line.
#[test]
fn help_wins_over_an_unknown_option() {
    for (bad, good) in [
        (&["--zzz-nope", "--help"][..], &["--help"][..]),
        (&["--zzz-nope", "-h"], &["-h"]),
        (
            &["meta", "enrich", "--zzz-nope", "--help"],
            &["meta", "enrich", "--help"],
        ),
        (&["meta", "--zzz-nope", "-h"], &["meta", "-h"]),
        (&["archive", "--help", "--zzz-nope"], &["archive", "--help"]),
        (&["-V", "-h"], &["-h"]),
    ] {
        let (code, stdout, _) = run(bad);
        assert_eq!(code, Some(0), "{bad:?}");
        assert_eq!(stdout, run(good).1, "{bad:?} printed other help");
    }
}

#[test]
fn help_after_double_dash_is_not_a_help_request() {
    let (code, _, _) = run(&["--zzz-nope", "--", "--help"]);
    assert_eq!(code, Some(2));
}

// §7: `-h` and `--help` print the same text, with the exit codes.
#[test]
fn short_and_long_help_are_the_same() {
    for cmd in [&[][..], &["repair"], &["meta", "set"], &["archive"]] {
        let short = run(&[cmd, &["-h"]].concat()).1;
        assert_eq!(short, run(&[cmd, &["--help"]].concat()).1, "{cmd:?}");
        assert!(short.contains("Exit codes:"), "{cmd:?}");
    }
}

// §3.4: a boolean given twice means the same as once; a value given twice
// is still an error.
#[test]
fn a_repeated_flag_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let (code, _, stderr) = run(&["repair", "-i", s(&epub), "--dry-run", "--dry-run"]);
    assert_ne!(code, Some(2), "{stderr}");
    let (code, _, _) = run(&[
        "repair",
        "-i",
        s(&epub),
        "--goal",
        "valid",
        "--goal",
        "valid",
    ]);
    assert_eq!(code, Some(2));
}

// §3.6: every command takes `--format`; `human` is all most of them have.
#[test]
fn format_human_everywhere_json_only_where_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let (code, _, stderr) = run(&["meta", "show", "-i", s(&epub), "--format", "human"]);
    assert_eq!(code, Some(0), "{stderr}");
    let (code, _, stderr) = run(&["-i", s(&epub), "--format", "json"]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("human"),
        "the message lists the values: {stderr}"
    );
}

// §4: the output never replaces the input, `-f` or not.
#[test]
fn output_equal_to_input_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let before = std::fs::read(&epub).unwrap();
    for args in [
        &["-i", s(&epub), "-o", s(&epub), "-f"][..],
        &[
            "meta",
            "set",
            "-i",
            s(&epub),
            "--title",
            "X",
            "-o",
            s(&epub),
        ],
        &["repair", "-i", s(&epub), "-o", s(&epub)],
    ] {
        let (code, _, stderr) = run(args);
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert!(stderr.starts_with("error: "), "{stderr}");
    }
    assert_eq!(
        std::fs::read(&epub).unwrap(),
        before,
        "the input was modified"
    );
}

// Until the next major: the old spellings work, with a warning.
#[test]
fn deprecated_spellings_warn() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let (code, _, stderr) = run(&["meta", "show", s(&epub)]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("warning: a path without -i"), "{stderr}");

    let out = dir.path().join("out.epub");
    let args = ["-i", s(&epub), "-o", s(&out), "-q", "70", "--webp"];
    let (_, _, stderr) = run(&args);
    assert!(stderr.contains("warning: -q is deprecated"), "{stderr}");
    let (_, _, stderr) = run(&args);
    assert!(stderr.contains("without -f/--force"), "{stderr}");
    let (_, _, stderr) = run(&["-i", s(&epub), "-o", s(&out), "--quality", "70", "-f"]);
    assert!(!stderr.contains("warning:"), "{stderr}");
}
