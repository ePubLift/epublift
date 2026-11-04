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
fn format_metadata_is_the_json_object() {
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
}

#[test]
#[cfg(feature = "metadata")]
fn the_old_enrich_spellings_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let epub = tiny_epub(dir.path());
    for old in [["--lang", "en"].as_slice(), ["--cover"].as_slice()] {
        let out = Command::new(bin())
            .args(["meta", "enrich", "--isbn", "9780000000002"])
            .args(old)
            .arg("-i")
            .arg(&epub)
            .output()
            .expect("run epublift meta enrich");
        assert_eq!(out.status.code(), Some(2), "{old:?}");
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
    // `--json` is the envelope family-wide, and this output isn't one.
    let (code, _, _) = show(&["--json"], &epub);
    assert_eq!(code, Some(2));
}

/// An EPUB 2 with the shape calibre leaves: an empty unique identifier, two real
/// ones, an empty NCX `dtb:uid`.
fn calibre_shape(dir: &std::path::Path, encryption: bool) -> std::path::PathBuf {
    let path = dir.join("calibre.epub");
    let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut add = |name: &str, body: &str| {
        zip.start_file(name, stored).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    add("mimetype", "application/epub+zip");
    add(
        "META-INF/container.xml",
        r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
    );
    if encryption {
        add(
            "META-INF/encryption.xml",
            r#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><EncryptedData xmlns="http://www.w3.org/2001/04/xmlenc#"><EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/></EncryptedData></encryption>"#,
        );
    }
    add(
        "OEBPS/content.opf",
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" xmlns:opf="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:language>en</dc:language><dc:identifier id="bookid"/><dc:identifier opf:scheme="calibre">abc-123</dc:identifier><dc:identifier opf:scheme="ISBN">978-0-14-032872-1</dc:identifier></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"/></package>"#,
    );
    add(
        "OEBPS/toc.ncx",
        r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head><meta name="dtb:uid" content=""/></head><docTitle><text>T</text></docTitle><navMap/></ncx>"#,
    );
    zip.finish().unwrap();
    path
}

fn set(args: &[&str], epub: &std::path::Path, out: &std::path::Path) -> (Option<i32>, String) {
    let o = Command::new(bin())
        .args(["meta", "set"])
        .args(args)
        .arg("-i")
        .arg(epub)
        .arg("-o")
        .arg(out)
        .output()
        .expect("run epublift meta set");
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn entry(epub: &std::path::Path, name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(epub).unwrap()).unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut zip.by_name(name).unwrap(), &mut s).unwrap();
    s
}

#[test]
fn unique_identifier_takes_an_existing_value_and_the_ncx_follows() {
    let dir = tempfile::tempdir().unwrap();
    let book = calibre_shape(dir.path(), false);
    let out = dir.path().join("out.epub");
    let (code, err) = set(&["--unique-identifier", "abc-123"], &book, &out);
    assert_eq!(code, Some(0), "{err}");
    let opf = entry(&out, "OEBPS/content.opf");
    assert!(opf.contains(r#"unique-identifier="epublift-uid""#), "{opf}");
    assert!(
        opf.contains(r#"id="epublift-uid" opf:scheme="calibre">abc-123<"#)
            || opf.contains(r#"opf:scheme="calibre" id="epublift-uid">abc-123<"#),
        "{opf}"
    );
    assert!(!opf.contains(r#"id="bookid""#), "the empty one goes: {opf}");
    assert!(entry(&out, "OEBPS/toc.ncx").contains(r#"content="abc-123""#));
}

#[test]
fn unknown_value_is_exit_2_and_lists_the_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    let book = calibre_shape(dir.path(), false);
    let out = dir.path().join("out.epub");
    let (code, err) = set(&["--unique-identifier", "nope"], &book, &out);
    assert_eq!(code, Some(2));
    assert!(
        err.contains("abc-123") && err.contains("978-0-14-032872-1"),
        "{err}"
    );
    assert!(!out.exists());
}

#[test]
fn both_unique_identifier_options_are_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let book = calibre_shape(dir.path(), false);
    let out = dir.path().join("out.epub");
    let (code, _) = set(
        &["--unique-identifier", "abc-123", "--new-unique-identifier"],
        &book,
        &out,
    );
    assert_eq!(code, Some(2));
}

#[test]
fn new_unique_identifier_comes_from_the_isbn_and_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    let book = calibre_shape(dir.path(), false);
    let out = dir.path().join("out.epub");
    let (code, err) = set(&["--new-unique-identifier"], &book, &out);
    assert_eq!(code, Some(0), "{err}");
    let uuid = "urn:uuid:71f90c32-46da-593e-8441-cba14452e693";
    assert!(entry(&out, "OEBPS/content.opf").contains(uuid));
    assert!(entry(&out, "OEBPS/toc.ncx").contains(uuid));
    // Again on the result: the same identifier, not a second copy.
    let again = dir.path().join("again.epub");
    let (code, err) = set(&["--new-unique-identifier"], &out, &again);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(entry(&again, "OEBPS/content.opf").matches(uuid).count(), 1);
}

#[test]
fn obfuscated_fonts_refuse_a_change_and_name_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let book = calibre_shape(dir.path(), true);
    let out = dir.path().join("out.epub");
    let (code, err) = set(&["--new-unique-identifier"], &book, &out);
    assert_eq!(code, Some(2));
    assert!(err.contains("META-INF/encryption.xml"), "{err}");
    assert!(!out.exists());
}
