//! Integration tests for `epublift repair` (the epubsana wiring). These pin OUR
//! surface — who decides each fix, when a file is written, the exit code, the
//! envelope's skeleton — on small books whose one defect epubsana is known to
//! fix, not epubsana's whole rule set, which evolves on its own.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_epublift")
}

/// A fresh directory for one test, so tests never see each other's outputs.
fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("epublift-repair-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A small valid EPUB 3. `body` is the chapter's body; `spine` is the itemrefs.
fn epub(dir: &Path, name: &str, body: &str, spine: &str) -> PathBuf {
    let path = dir.join(name);
    let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut put = |name: &str, data: &str| {
        zip.start_file(name, stored).unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    };
    put("mimetype", "application/epub+zip");
    put(
        "META-INF/container.xml",
        r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
    );
    put(
        "content.opf",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="id">urn:uuid:12345678-1234-1234-1234-123456789abc</dc:identifier>
<dc:title>Tiny Book</dc:title>
<dc:language>en</dc:language>
<meta property="dcterms:modified">2026-01-01T00:00:00Z</meta>
</metadata>
<manifest>
<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
<item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
</manifest>
<spine>{spine}</spine>
</package>"#
        ),
    );
    put(
        "nav.xhtml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="en" xml:lang="en">
<head><title>Contents</title></head>
<body><nav epub:type="toc"><ol><li><a href="c1.xhtml">One</a></li></ol></nav></body>
</html>"#,
    );
    put(
        "c1.xhtml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" lang="en" xml:lang="en">
<head><title>One</title></head>
<body>{body}</body>
</html>"#
        ),
    );
    zip.finish().unwrap();
    path
}

const SPINE: &str = r#"<itemref idref="c1"/>"#;

fn valid_book(dir: &Path) -> PathBuf {
    epub(dir, "book.epub", "<p>Hello.</p>", SPINE)
}

/// Undeclared `&nbsp;`: a fatal, and the fix is `AutoSafe`.
fn entity_book(dir: &Path) -> PathBuf {
    epub(dir, "book.epub", "<p>Hello&nbsp;world.</p>", SPINE)
}

/// The chapter listed twice in the spine (OPF-034): the fix needs a decision.
fn duplicate_spine_book(dir: &Path) -> PathBuf {
    epub(
        dir,
        "book.epub",
        "<p>Hello.</p>",
        r#"<itemref idref="c1"/><itemref idref="c1"/>"#,
    )
}

/// Run `epublift repair <book> <args>` with stdin closed — no terminal, so
/// nothing can be asked.
fn repair(book: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("repair")
        .arg(book)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run epublift repair")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn envelope(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}):\n{}", stdout(out)))
}

fn repaired(dir: &Path) -> PathBuf {
    dir.join("book_repaired.epub")
}

#[test]
fn a_valid_book_has_nothing_to_repair_and_writes_nothing() {
    let dir = test_dir("valid");
    let out = repair(&valid_book(&dir), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("Nothing to repair"),
        "{}",
        stdout(&out)
    );
    assert!(
        !repaired(&dir).exists(),
        "a run that changed nothing wrote a file"
    );
}

#[test]
fn a_safe_fix_is_applied_without_a_terminal() {
    let dir = test_dir("autosafe");
    let book = entity_book(&dir);
    let original = std::fs::read(&book).unwrap();
    let out = repair(&book, &["--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));

    let env = envelope(&out);
    let input = &env["inputs"][0];
    assert_eq!(env["tool"], "epublift");
    assert_eq!(input["status"], "ok");
    let fix = &input["items"][0];
    assert_eq!(fix["data"]["fix_id"], "fix.html_entities");
    assert_eq!(fix["outcome"], "applied");
    assert_eq!(input["summary"]["fatals_after"], 0);

    assert!(repaired(&dir).exists(), "the repaired book was not written");
    assert_eq!(
        std::fs::read(&book).unwrap(),
        original,
        "the input was modified"
    );
}

#[test]
fn a_fix_that_needs_a_decision_is_skipped_without_a_terminal() {
    let dir = test_dir("skipped");
    let out = repair(&duplicate_spine_book(&dir), &["--format", "json"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));

    let input = &envelope(&out)["inputs"][0];
    let fix = &input["items"][0];
    assert_eq!(fix["data"]["fix_id"], "fix.spine_duplicate_itemref");
    assert_eq!(fix["data"]["tier"], "confirm_needed");
    assert_eq!(fix["outcome"], "skipped");
    assert!(input["output"].is_null());
    assert!(
        !repaired(&dir).exists(),
        "nothing was applied, yet a file was written"
    );
}

#[test]
fn yes_applies_a_fix_that_needs_a_decision() {
    let dir = test_dir("yes");
    let out = repair(&duplicate_spine_book(&dir), &["--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("fixed (needs a decision)"),
        "{}",
        stdout(&out)
    );
    assert!(repaired(&dir).exists());

    // The repaired book has nothing left to repair.
    let again = repair(
        &repaired(&dir),
        &["-o", dir.join("again.epub").to_str().unwrap()],
    );
    assert_eq!(again.status.code(), Some(0), "{}", stdout(&again));
    assert!(
        stdout(&again).contains("Nothing to repair"),
        "{}",
        stdout(&again)
    );
}

#[test]
fn a_dry_run_proposes_and_writes_nothing() {
    let dir = test_dir("dry");
    let out = repair(
        &duplicate_spine_book(&dir),
        &["--dry-run", "--format", "json"],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));

    let env = envelope(&out);
    assert_eq!(env["dry_run"], true);
    let input = &env["inputs"][0];
    assert_eq!(input["items"][0]["outcome"], "proposed");
    // It names the file it would write.
    assert!(
        input["output"]
            .as_str()
            .unwrap()
            .ends_with("book_repaired.epub")
    );
    assert!(!repaired(&dir).exists(), "a dry run wrote a file");
}

#[test]
fn the_output_may_not_be_the_input() {
    let dir = test_dir("same");
    let book = entity_book(&dir);
    let original = std::fs::read(&book).unwrap();
    let out = repair(&book, &["-o", book.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("output path is the input"));
    assert_eq!(
        std::fs::read(&book).unwrap(),
        original,
        "the input was modified"
    );
}

#[test]
fn an_unreadable_book_exits_2() {
    let dir = test_dir("unreadable");
    let book = dir.join("book.epub");
    std::fs::write(&book, b"this is plainly not an epub").unwrap();
    let out = repair(&book, &[]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
}

#[test]
fn dry_run_and_yes_contradict_each_other() {
    let dir = test_dir("conflict");
    let out = repair(&valid_book(&dir), &["--dry-run", "--yes"]);
    assert_eq!(out.status.code(), Some(2), "clap usage errors exit 2");
}
