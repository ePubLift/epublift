//! veripublica conventions (CLI.md v0.7) on the command line: help, repeated
//! flags, `--format`, no bare paths, the file-safety refusals and exit codes.

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

// §1, §2: a bare path is a usage error, on every command.
#[test]
fn a_path_without_i_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    for args in [
        &[s(&epub)][..],
        &["check", s(&epub)],
        &["repair", s(&epub)],
        &["meta", "show", s(&epub)],
        &["meta", "set", "--title", "x", s(&epub)],
        &["archive", s(&epub)],
        &["restore", s(&epub)],
    ] {
        let (code, _, stderr) = run(args);
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("--help"), "{args:?}: {stderr}");
    }
}

// §2: a transformer takes one input; a verifier may repeat `-i`.
#[test]
fn a_second_input_is_a_usage_error_except_for_check() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let p = s(&epub);
    for args in [
        &["-i", p, "-i", p][..],
        &["repair", "-i", p, "-i", p],
        &["archive", "-i", p, "-i", p],
        &["meta", "show", "-i", p, "-i", p],
    ] {
        assert_eq!(run(args).0, Some(2), "{args:?}");
    }
    assert_ne!(run(&["check", "-i", p, "-i", p]).0, Some(2));
}

// §3.2: the short-flag set is closed.
#[test]
fn the_old_short_spellings_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let out = dir.path().join("out.epub");
    for old in [["-q", "70"], ["-r", "r.txt"]] {
        let (code, _, _) = run(&["-i", s(&epub), "-o", s(&out), old[0], old[1]]);
        assert_eq!(code, Some(2), "{old:?}");
    }
    assert!(!out.exists());
}

// §4: an existing file, output or report, is refused without `-f`.
#[test]
fn an_existing_output_needs_force() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let out = dir.path().join("out.epub");
    let report = dir.path().join("out.txt");
    let args = ["-i", s(&epub), "-o", s(&out), "--report", s(&report)];
    assert_ne!(run(&args).0, Some(2));

    std::fs::write(&out, b"mine").unwrap();
    let (code, _, stderr) = run(&args);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("exists; use -f"), "{stderr}");
    assert!(stderr.contains("out.epub"), "{stderr}");
    assert_eq!(std::fs::read(&out).unwrap(), b"mine", "left alone");

    // The report alone counts too.
    std::fs::remove_file(&out).unwrap();
    let (code, _, stderr) = run(&args);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("out.txt"), "{stderr}");
    assert!(!out.exists(), "refused before any work");

    let (code, _, stderr) = run(&["-i", s(&epub), "-o", s(&out), "--report", s(&report), "-f"]);
    assert_ne!(code, Some(2), "{stderr}");
    assert!(out.exists());
}

#[test]
fn each_command_refuses_an_existing_output() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let meta = dir.path().join("meta.epub");
    std::fs::write(&meta, b"mine").unwrap();
    let (code, _, stderr) = run(&[
        "meta",
        "set",
        "--title",
        "x",
        "-i",
        s(&epub),
        "-o",
        s(&meta),
    ]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("use -f"), "{stderr}");
    assert_eq!(run(&["archive", "-i", s(&epub)]).0, Some(0));
    assert_eq!(run(&["archive", "-i", s(&epub)]).0, Some(2));
    assert_eq!(run(&["archive", "-i", s(&epub), "-f"]).0, Some(0));
}

// §6: a run that could not do its job is 2, never 1.
#[test]
fn a_failed_run_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.epub");
    let not_epub = dir.path().join("not.epub");
    std::fs::write(&not_epub, b"not a zip").unwrap();
    for args in [
        &[][..],
        &["-i", s(&missing)],
        &["-i", s(&not_epub)],
        &["meta", "show", "-i", s(&missing)],
        &["meta", "set", "--title", "x", "-i", s(&not_epub)],
        &["archive", "-i", s(&not_epub)],
        &["restore", "-i", s(&missing)],
        &["check", "-i", s(&missing)],
        &["repair", "-i", s(&missing)],
    ] {
        let (code, _, stderr) = run(args);
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
    }
}

// §7.
#[test]
fn help_names_the_conventions_version() {
    for args in [
        &["--help"][..],
        &["check", "-h"],
        &["meta", "set", "--help"],
    ] {
        let (_, stdout, _) = run(args);
        assert!(
            stdout.contains("Conforms to veripublica conventions v0.7."),
            "{args:?}"
        );
    }
}

// §6: a transformer whose book still has errors exits 1, with the file written.
#[test]
fn an_upgrade_that_leaves_errors_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let out = dir.path().join("out.epub");
    let (check, _, _) = run(&["check", "-i", s(&epub)]);
    assert_eq!(check, Some(1), "the fixture is invalid");
    let (code, _, stderr) = run(&["-i", s(&epub), "-o", s(&out)]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(out.exists());
    assert!(stderr.contains("still has"), "{stderr}");
}
