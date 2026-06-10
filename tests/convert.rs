//! End-to-end integration tests for the `convert` library API.
//!
//! Each test builds a legacy EPUB 2 fixture in a temporary directory, runs it
//! through `epublift::convert`, and asserts on both the returned `Report` and
//! the contents of the produced EPUB.

use std::io::{Cursor, Read, Write};
use std::path::Path;

use epublift::{EpubVersion, Options, convert};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

// ---------------------------------------------------------------------------
// Fixture construction
// ---------------------------------------------------------------------------

/// One entry to place in a fixture EPUB (`path` is the in-archive name).
struct Entry {
    path: &'static str,
    data: Vec<u8>,
}

/// A solid-color JPEG, encoded in memory.
fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_pixel(w, h, Rgb([0x2c, 0x3e, 0x50]));
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, ImageFormat::Jpeg)
        .unwrap();
    buf.into_inner()
}

/// A solid-color RGBA PNG (with transparency channel), encoded in memory.
fn png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbaImage::from_pixel(w, h, Rgba([0xe7, 0x4c, 0x3c, 0xff]));
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

fn text(path: &'static str, body: &str) -> Entry {
    Entry {
        path,
        data: body.as_bytes().to_vec(),
    }
}

const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

const TOC_NCX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd">
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head><meta name="dtb:uid" content="urn:uuid:0001"/></head>
  <docTitle><text>Test EPUB 2 Book</text></docTitle>
  <navMap>
    <navPoint id="navPoint-1" playOrder="1">
      <navLabel><text>Chapter 1</text></navLabel>
      <content src="chapter1.html"/>
    </navPoint>
  </navMap>
</ncx>"#;

const CHAPTER1_WITH_IMAGES: &str = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter 1</title><link rel="stylesheet" type="text/css" href="styles.css"/></head>
<body>
  <h1>Chapter 1</h1>
  <p><img src="images/cover.jpg" alt="Cover"/></p>
  <p><img src="images/logo.png" alt="Logo"/></p>
</body>
</html>"#;

const CHAPTER1_TEXT_ONLY: &str = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter 1</title></head>
<body><h1>Chapter 1</h1><p>No images here.</p></body>
</html>"#;

const STYLES_CSS: &str = r#"body { margin: 1em; }
.logo { background-image: url('images/logo.png'); }"#;

const OPF_WITH_IMAGES: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="BookId" opf:scheme="UUID">urn:uuid:0001</dc:identifier>
    <dc:title>Test EPUB 2 Book</dc:title>
    <dc:creator opf:role="aut">Jane Doe</dc:creator>
    <dc:language>en</dc:language>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="style" href="styles.css" media-type="text/css"/>
    <item id="cover-image" href="images/cover.jpg" media-type="image/jpeg"/>
    <item id="logo-image" href="images/logo.png" media-type="image/png"/>
    <item id="chapter1" href="chapter1.html" media-type="text/html"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="chapter1"/>
  </spine>
  <guide>
    <reference type="cover" title="Cover Page" href="chapter1.html"/>
  </guide>
</package>"#;

const OPF_TEXT_ONLY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="BookId" opf:scheme="UUID">urn:uuid:0002</dc:identifier>
    <dc:title>Text Only Book</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="chapter1" href="chapter1.html" media-type="text/html"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="chapter1"/>
  </spine>
</package>"#;

fn legacy_with_images() -> Vec<Entry> {
    vec![
        text("META-INF/container.xml", CONTAINER_XML),
        text("OEBPS/content.opf", OPF_WITH_IMAGES),
        text("OEBPS/toc.ncx", TOC_NCX),
        text("OEBPS/chapter1.html", CHAPTER1_WITH_IMAGES),
        text("OEBPS/styles.css", STYLES_CSS),
        Entry {
            path: "OEBPS/images/cover.jpg",
            data: jpeg(32, 32),
        },
        Entry {
            path: "OEBPS/images/logo.png",
            data: png(32, 32),
        },
    ]
}

fn legacy_text_only() -> Vec<Entry> {
    vec![
        text("META-INF/container.xml", CONTAINER_XML),
        text("OEBPS/content.opf", OPF_TEXT_ONLY),
        text("OEBPS/toc.ncx", TOC_NCX),
        text("OEBPS/chapter1.html", CHAPTER1_TEXT_ONLY),
    ]
}

/// Assemble `entries` into an EPUB zip at `dest`, with `mimetype` stored first.
fn build_epub(dest: &Path, entries: &[Entry]) {
    let file = std::fs::File::create(dest).unwrap();
    let mut zip = ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    for e in entries {
        zip.start_file(e.path, deflated).unwrap();
        zip.write_all(&e.data).unwrap();
    }
    zip.finish().unwrap();
}

// ---------------------------------------------------------------------------
// Output inspection helpers
// ---------------------------------------------------------------------------

fn entry_names(epub: &Path) -> Vec<String> {
    let mut zip = ZipArchive::new(std::fs::File::open(epub).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_string())
        .collect()
}

fn read_entry(epub: &Path, name: &str) -> String {
    let mut zip = ZipArchive::new(std::fs::File::open(epub).unwrap()).unwrap();
    let mut f = zip.by_name(name).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    s
}

/// True when the first archive entry is an uncompressed `mimetype` (per spec).
fn mimetype_first_and_stored(epub: &Path) -> bool {
    let mut zip = ZipArchive::new(std::fs::File::open(epub).unwrap()).unwrap();
    let first = zip.by_index(0).unwrap();
    first.name() == "mimetype" && first.compression() == CompressionMethod::Stored
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn full_legacy_epub_is_modernized() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_with_images());

    let report = convert(&input, &Options::default(), |_| {}).unwrap();

    // --- Report ---
    assert_eq!(report.input_name, "legacy.epub");
    assert_eq!(report.output_name, "legacy_v3.3.epub");
    assert_eq!(report.target_version, EpubVersion::V3_3);
    assert!(report.output_path.exists());
    assert_eq!(report.image_metrics.len(), 2);
    for m in &report.image_metrics {
        assert!(m.new_size > 0, "{} produced an empty WebP", m.name);
    }
    let names: Vec<&str> = report
        .image_metrics
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert!(names.contains(&"cover.jpg") && names.contains(&"logo.png"));

    // --- Output archive structure ---
    let out = &report.output_path;
    assert!(
        mimetype_first_and_stored(out),
        "mimetype must be stored first"
    );

    let names = entry_names(out);
    let has = |n: &str| names.iter().any(|e| e == n);
    assert!(has("OEBPS/images/cover.webp"), "cover should be WebP");
    assert!(has("OEBPS/images/logo.webp"), "logo should be WebP");
    assert!(
        !has("OEBPS/images/cover.jpg"),
        "original JPEG should be gone"
    );
    assert!(!has("OEBPS/images/logo.png"), "original PNG should be gone");
    assert!(
        has("OEBPS/nav.xhtml"),
        "EPUB 3 nav document should be generated"
    );
    assert!(
        has("OEBPS/toc.ncx"),
        "legacy NCX should be retained (hybrid)"
    );

    // --- OPF upgrade ---
    let opf = read_entry(out, "OEBPS/content.opf");
    assert!(opf.contains("version=\"3.0\""), "package upgraded to 3.0");
    assert!(opf.contains("dcterms:modified"), "modified timestamp added");
    assert!(
        opf.contains("image/webp"),
        "image media types updated to WebP"
    );
    assert!(
        opf.contains("cover.webp"),
        "manifest href points at the WebP"
    );
    assert!(opf.contains("properties=\"nav\""), "nav item registered");
    assert!(!opf.contains("<guide"), "legacy <guide> removed");

    // --- Content modernization ---
    let chapter = read_entry(out, "OEBPS/chapter1.html");
    assert!(
        chapter.contains("<!DOCTYPE html>"),
        "DOCTYPE modernized to HTML5"
    );
    assert!(
        !chapter.to_lowercase().contains("xhtml 1.1"),
        "legacy DOCTYPE gone"
    );
    assert!(
        chapter.contains("images/cover.webp"),
        "image refs rewritten in XHTML"
    );
    assert!(
        !chapter.contains("images/cover.jpg"),
        "stale JPEG ref removed"
    );

    // CSS references should be rewritten too.
    let css = read_entry(out, "OEBPS/styles.css");
    assert!(css.contains("images/logo.webp") && !css.contains("images/logo.png"));
}

#[test]
fn output_name_is_version_stamped_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("my book.epub");
    build_epub(&input, &legacy_text_only());

    let report = convert(&input, &Options::default(), |_| {}).unwrap();
    assert_eq!(report.output_name, "my book_v3.3.epub");
    assert!(report.output_path.exists());
}

#[test]
fn ascii_option_transliterates_output_name() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("Çocuk Kitabı.epub");
    build_epub(&input, &legacy_text_only());

    let opts = Options {
        ascii: true,
        ..Options::default()
    };
    let report = convert(&input, &opts, |_| {}).unwrap();

    assert!(report.output_name.is_ascii(), "name should be ASCII");
    assert!(
        !report.output_name.contains(' '),
        "spaces should be replaced"
    );
    assert!(report.output_name.ends_with("_v3.3.epub"));
    assert!(report.output_path.exists());
}

#[test]
fn epub_without_images_still_upgrades_structure() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("textonly.epub");
    build_epub(&input, &legacy_text_only());

    let report = convert(&input, &Options::default(), |_| {}).unwrap();

    assert!(
        report.image_metrics.is_empty(),
        "no raster images to convert"
    );

    let out = &report.output_path;
    let opf = read_entry(out, "OEBPS/content.opf");
    assert!(opf.contains("version=\"3.0\""));
    assert!(opf.contains("dcterms:modified"));
    assert!(entry_names(out).iter().any(|n| n == "OEBPS/nav.xhtml"));
}

#[test]
fn explicit_output_path_is_respected() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_text_only());

    let explicit = dir.path().join("custom-name.epub");
    let opts = Options {
        output: Some(explicit.clone()),
        ..Options::default()
    };
    let report = convert(&input, &opts, |_| {}).unwrap();

    assert_eq!(report.output_path, explicit);
    assert_eq!(report.output_name, "custom-name.epub");
    assert!(explicit.exists());
}
