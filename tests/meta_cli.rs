//! Integration tests for `epublift meta show`'s output formats.

use std::io::Write;
use std::process::Command;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_epublift")
}

/// A minimal EPUB 3 with a title and an author, written to a temp dir.
fn tiny_epub(dir: &std::path::Path) -> std::path::PathBuf {
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
        br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:uuid:1</dc:identifier><dc:title>Tiny Book</dc:title><dc:creator>Ada Lovelace</dc:creator><dc:language>en</dc:language></metadata><manifest/><spine/></package>"#,
    )
    .unwrap();
    zip.finish().unwrap();
    path
}

fn show(args: &[&str], epub: &std::path::Path) -> (Option<i32>, String, String) {
    let out = Command::new(bin())
        .args(["meta", "show"])
        .args(args)
        .arg("-i")
        .arg(epub)
        .output()
        .expect("run epublift meta show");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn format_metadata_is_the_json_object_and_json_is_its_deprecated_alias() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());

    let (code, human, _) = show(&[], &epub);
    assert_eq!(code, Some(0));
    assert!(human.contains("Tiny Book"));
    assert_eq!(show(&["--format", "human"], &epub).1, human);

    let (code, md, _) = show(&["--format", "metadata"], &epub);
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&md).expect("one JSON object");
    assert!(md.contains("Tiny Book"), "got: {v}");
    let (code, out, err) = show(&["--json"], &epub);
    assert_eq!(code, Some(0));
    assert_eq!(out, md, "--json must print what --format metadata prints");
    // Deprecated (conventions CLI.md §3.2.2: `--json` is the envelope family-wide),
    // and the warning goes to stderr so the JSON on stdout stays clean.
    assert!(
        err.contains("deprecated") && err.contains("--format metadata"),
        "got: {err}"
    );
}

#[test]
#[cfg(feature = "metadata")]
fn enrich_old_and_new_spellings_conflict() {
    // `--lang` / `--cover` are the deprecated spellings of `--assume-language` /
    // `--include-cover`; giving both is a usage error (exit 2) before any lookup.
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    for pair in [
        ["--lang", "en", "--assume-language", "en"].as_slice(),
        ["--cover", "--include-cover"].as_slice(),
    ] {
        let out = Command::new(bin())
            .args(["meta", "enrich", "--isbn", "9780000000002"])
            .args(pair)
            .arg("-i")
            .arg(&epub)
            .output()
            .expect("run epublift meta enrich");
        assert_eq!(out.status.code(), Some(2), "{pair:?}");
    }
}

#[test]
fn json_is_not_a_meta_show_format() {
    // `json` is reserved for the veripublica envelope; this output isn't one.
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    let (code, _, err) = show(&["--format", "json"], &epub);
    assert_eq!(code, Some(2));
    assert!(
        err.contains("human") && err.contains("metadata"),
        "got: {err}"
    );
    let (code, _, _) = show(&["--json", "--format", "metadata"], &epub);
    assert_eq!(code, Some(2));
}
