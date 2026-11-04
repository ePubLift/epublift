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

/// Same as `legacy_with_images`, but the PNG logo is so tiny that a WebP would
/// be *larger* than the source — exercises the keep-the-original path.
fn legacy_with_keepable_png() -> Vec<Entry> {
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
            data: png(2, 2),
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
        .map(|i| zip.by_index(i).unwrap().name().unwrap().to_string())
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
    first.name().unwrap() == "mimetype" && first.compression() == CompressionMethod::Stored
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn full_legacy_epub_is_modernized() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_with_images());

    let webp = Options {
        image_strategy: epublift::ImageStrategy::WebP,
        ..Options::default()
    };
    let report = convert(&input, &webp, |_| {}).unwrap();

    // --- Report ---
    assert_eq!(report.input_name, "legacy.epub");
    assert_eq!(report.output_name, "legacy_v3.3.epub");
    assert_eq!(report.target_version, EpubVersion::V3_3);
    assert!(report.output_path.exists());
    assert_eq!(report.image_metrics.len(), 2);
    for m in &report.image_metrics {
        assert!(m.new_size > 0, "{} produced an empty WebP", m.name);
        // Core guarantee: an image is never written larger than the source.
        assert!(
            m.new_size <= m.original_size,
            "{} grew: {} -> {}",
            m.name,
            m.original_size,
            m.new_size
        );
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
fn larger_webp_keeps_the_original_image() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("keep.epub");
    build_epub(&input, &legacy_with_keepable_png());

    let webp = Options {
        image_strategy: epublift::ImageStrategy::WebP,
        ..Options::default()
    };
    let report = convert(&input, &webp, |_| {}).unwrap();

    // No image is ever written larger than its source.
    for m in &report.image_metrics {
        assert!(
            m.new_size <= m.original_size,
            "{} grew: {} -> {}",
            m.name,
            m.original_size,
            m.new_size
        );
    }

    let cover = report
        .image_metrics
        .iter()
        .find(|m| m.name == "cover.jpg")
        .expect("cover metric");
    assert!(
        !cover.kept && cover.new_size < cover.original_size,
        "the JPEG cover should still convert to a smaller WebP"
    );

    let logo = report
        .image_metrics
        .iter()
        .find(|m| m.name == "logo.png")
        .expect("logo metric");
    assert!(
        logo.kept,
        "the tiny PNG should be kept (WebP would be larger)"
    );
    assert_eq!(logo.new_size, logo.original_size);

    // Archive: cover became WebP, the kept PNG is untouched.
    let out = &report.output_path;
    let names = entry_names(out);
    let has = |n: &str| names.iter().any(|e| e == n);
    assert!(has("OEBPS/images/cover.webp"), "cover converted to WebP");
    assert!(has("OEBPS/images/logo.png"), "kept PNG retained as-is");
    assert!(
        !has("OEBPS/images/logo.webp"),
        "no WebP written for the kept image"
    );

    // OPF: cover updated to WebP, the kept PNG entry unchanged.
    let opf = read_entry(out, "OEBPS/content.opf");
    assert!(opf.contains("cover.webp"), "manifest cover points at WebP");
    assert!(
        opf.contains("images/logo.png") && opf.contains("image/png"),
        "kept PNG stays in the manifest as image/png"
    );

    // The reference to the kept PNG must remain intact (not rewritten to .webp).
    let css = read_entry(out, "OEBPS/styles.css");
    assert!(
        css.contains("images/logo.png") && !css.contains("images/logo.webp"),
        "kept PNG reference left unchanged in CSS"
    );
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

#[test]
fn kepub_injects_kobospans_and_names_output() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_with_images());

    let opts = Options {
        kepub: true,
        ..Options::default()
    };
    let report = convert(&input, &opts, |_| {}).unwrap();

    // Kobo output keeps the version stamp and adds the .kepub.epub extension
    // (Kobo keys on .kepub.epub; the _v3.3 marks it apart from the original).
    assert_eq!(report.output_name, "legacy_v3.3.kepub.epub");
    let out = &report.output_path;
    // Still a valid EPUB: mimetype stored first.
    assert!(
        mimetype_first_and_stored(out),
        "mimetype must be stored first"
    );

    // Content document carries Kobo markup on top of the normal upgrades.
    let chapter = read_entry(out, "OEBPS/chapter1.html");
    assert!(chapter.contains("class=\"koboSpan\""), "koboSpans injected");
    assert!(chapter.contains("id=\"kobo."), "koboSpan ids present");
    assert!(
        chapter.contains("id=\"book-inner\"") && chapter.contains("id=\"book-columns\""),
        "body wrapped in Kobo column scaffolding"
    );
    // EPUB 3: a marker class and no obsolete `type` (kepubverto 0.1.1's fix).
    assert!(
        chapter.contains("<style class=\"kobostylehacks\">"),
        "Kobo style hack added, valid for EPUB 3"
    );
    // Kobo can't render WebP, so .kepub forces keep-original images: the cover
    // stays a JPEG and is NOT converted to WebP.
    assert!(
        chapter.contains("images/cover.jpg"),
        "original image kept (no WebP) for Kobo"
    );
    assert!(
        !chapter.contains(".webp"),
        "kepub must not produce WebP refs"
    );
    assert!(
        report.image_metrics.is_empty(),
        "no images converted when keeping originals"
    );
    let names = entry_names(out);
    assert!(
        names.iter().any(|n| n == "OEBPS/images/cover.jpg"),
        "original JPEG retained in the archive"
    );
    assert!(
        !names.iter().any(|n| n.ends_with(".webp")),
        "no WebP files in a kepub"
    );
    assert!(
        chapter.contains("<!DOCTYPE html>"),
        "DOCTYPE still modernized"
    );
}

#[test]
fn images_are_kept_by_default() {
    // The default keeps JPEG and PNG as they are; WebP is opt-in.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_with_images());

    let report = convert(&input, &Options::default(), |_| {}).unwrap();
    assert!(report.image_metrics.is_empty());
    let names = entry_names(&report.output_path);
    assert!(names.iter().any(|n| n == "OEBPS/images/cover.jpg"));
    assert!(!names.iter().any(|n| n.ends_with(".webp")));
}

#[test]
fn kepub_webp_opts_into_webp_images() {
    // Opt-in: `--kepub --kepub-webp` emits WebP (for Kobo devices that have the
    // WebP image plugin installed) instead of the default keep-original.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_with_images());

    let opts = Options {
        kepub: true,
        kepub_webp: true,
        ..Options::default()
    };
    let report = convert(&input, &opts, |_| {}).unwrap();

    // Still a Kobo .kepub with koboSpan markup...
    assert_eq!(report.output_name, "legacy_v3.3.kepub.epub");
    let out = &report.output_path;
    let chapter = read_entry(out, "OEBPS/chapter1.html");
    assert!(chapter.contains("class=\"koboSpan\""), "still a kepub");
    // ...but now images ARE converted to WebP.
    let names = entry_names(out);
    assert!(
        names.iter().any(|n| n.ends_with(".webp")),
        "kepub_webp must produce WebP images"
    );
    assert!(
        !names.iter().any(|n| n == "OEBPS/images/cover.jpg"),
        "original JPEG replaced by WebP"
    );
    assert!(
        !report.image_metrics.is_empty(),
        "images were converted with kepub_webp"
    );
}

#[test]
fn keep_images_skips_webp_but_upgrades_structure() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_with_images());

    let opts = Options {
        image_strategy: epublift::ImageStrategy::KeepOriginal,
        ..Options::default()
    };
    let report = convert(&input, &opts, |_| {}).unwrap();

    // No conversion happened...
    assert!(report.image_metrics.is_empty(), "no images converted");
    let out = &report.output_path;
    let names = entry_names(out);
    assert!(
        names.iter().any(|n| n == "OEBPS/images/cover.jpg")
            && names.iter().any(|n| n == "OEBPS/images/logo.png"),
        "originals kept"
    );
    assert!(
        !names.iter().any(|n| n.ends_with(".webp")),
        "no WebP produced"
    );

    // ...but the structure was still modernized.
    let opf = read_entry(out, "OEBPS/content.opf");
    assert!(
        opf.contains("version=\"3.0\""),
        "package still upgraded to 3.0"
    );
    assert!(opf.contains("dcterms:modified"), "modified timestamp added");
    assert!(!opf.contains("image/webp"), "no WebP media types");
    let chapter = read_entry(out, "OEBPS/chapter1.html");
    assert!(chapter.contains("images/cover.jpg"), "image ref unchanged");
    assert!(chapter.contains("<!DOCTYPE html>"), "DOCTYPE modernized");
}

#[test]
fn default_run_has_no_kobospans() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_text_only());

    let report = convert(&input, &Options::default(), |_| {}).unwrap();
    let chapter = read_entry(&report.output_path, "OEBPS/chapter1.html");
    assert!(
        !chapter.contains("koboSpan"),
        "koboSpans must only appear with --kepub"
    );
}

// ---------------------------------------------------------------------------
// Experimental Zstandard-OCF packaging (feature-gated research track)
// ---------------------------------------------------------------------------

#[cfg(feature = "zstd-experimental")]
mod zstd_experimental {
    use super::*;
    use epublift::{Packaging, ZstdMode, decode_zstd_epub};

    fn read_bytes(epub: &Path, name: &str) -> Vec<u8> {
        let mut zip = ZipArchive::new(std::fs::File::open(epub).unwrap()).unwrap();
        let mut f = zip.by_name(name).unwrap();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        v
    }

    /// The credibility anchor: a Zstd-packaged EPUB decodes back to byte-for-byte
    /// the same contents as the conformant Deflate output. (The OPF carries a
    /// `dcterms:modified` timestamp that differs between the two independent
    /// `convert` runs, so it is checked structurally rather than byte-compared.)
    #[test]
    fn zstd_packaging_round_trips_losslessly() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("legacy.epub");
        build_epub(&input, &legacy_with_images());

        // Conformant reference output.
        let deflate = convert(&input, &Options::default(), |_| {}).unwrap();
        assert!(mimetype_first_and_stored(&deflate.output_path));

        // Experimental Zstd output, then decode it back to a conformant EPUB.
        let zstd_opts = Options {
            packaging: Packaging::Zstd {
                mode: ZstdMode::PerEntry,
                level: 19,
            },
            output: Some(dir.path().join("book_zstd-experimental.epub")),
            ..Options::default()
        };
        let zstd = convert(&input, &zstd_opts, |_| {}).unwrap();

        let decoded = dir.path().join("book_decoded.epub");
        decode_zstd_epub(&zstd.output_path, &decoded).unwrap();

        // Decoded archive is a normal, conformant EPUB again.
        assert!(
            mimetype_first_and_stored(&decoded),
            "decoded mimetype stored first"
        );

        let mut a = entry_names(&deflate.output_path);
        let mut b = entry_names(&decoded);
        a.sort();
        b.sort();
        assert_eq!(a, b, "decoded entry set matches the conformant output");

        // Every entry except the timestamped OPF is byte-identical — proving the
        // Zstd encode/decode is lossless on text and (compressed) image payloads.
        for name in &a {
            if name.ends_with(".opf") {
                continue;
            }
            assert_eq!(
                read_bytes(&deflate.output_path, name),
                read_bytes(&decoded, name),
                "entry {name} must survive the Zstd round-trip byte-for-byte"
            );
        }
        let opf = entry_names(&decoded)
            .into_iter()
            .find(|n| n.ends_with(".opf"))
            .unwrap();
        assert!(read_entry(&decoded, &opf).contains("version=\"3.0\""));
    }

    /// The output is named with the `_zstd-experimental` suffix (conformance
    /// axis), never a version-looking `_v3.x` name.
    #[test]
    fn zstd_output_is_named_experimental() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("My Book.epub");
        build_epub(&input, &legacy_text_only());

        let opts = Options {
            packaging: Packaging::Zstd {
                mode: ZstdMode::PerEntry,
                level: 3,
            },
            ..Options::default()
        };
        let report = convert(&input, &opts, |_| {}).unwrap();
        assert_eq!(report.output_name, "My Book_zstd-experimental.epub");
        assert!(!report.output_name.contains("_v3"));
    }

    /// Non-mimetype entries really use ZIP method 93 (Zstandard).
    #[test]
    fn zstd_entries_use_method_93() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("legacy.epub");
        build_epub(&input, &legacy_text_only());

        let opts = Options {
            packaging: Packaging::Zstd {
                mode: ZstdMode::PerEntry,
                level: 3,
            },
            ..Options::default()
        };
        let report = convert(&input, &opts, |_| {}).unwrap();

        // Read the central directory ourselves: count compression-method ids.
        let bytes = std::fs::read(&report.output_path).unwrap();
        let methods = central_directory_methods(&bytes);
        assert!(methods.contains(&0), "mimetype should be Stored (method 0)");
        assert!(methods.contains(&93), "content should be Zstd (method 93)");
        assert!(
            !methods.contains(&8),
            "no Deflate (method 8) in a Zstd archive"
        );
    }

    /// Scan central-directory headers and return each entry's compression method.
    fn central_directory_methods(zip: &[u8]) -> Vec<u16> {
        const SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
        let mut out = Vec::new();
        let mut i = 0;
        while i + 12 <= zip.len() {
            if zip[i..i + 4] == SIG {
                out.push(u16::from_le_bytes([zip[i + 10], zip[i + 11]]));
            }
            i += 1;
        }
        out
    }
}

#[test]
fn hostile_nesting_is_declined_not_a_crash() {
    // 20,000 nested elements in the OPF: without epubveri's xmlguard in front
    // of roxmltree this overflows the stack and aborts the whole process (the
    // web server included). It has to come back as an ordinary error instead.
    let depth = 20_000;
    let nested = format!(
        "{}{}</metadata>",
        "<x:d xmlns:x=\"urn:x\">".repeat(depth),
        "</x:d>".repeat(depth)
    );
    let opf: &'static str = Box::leak(OPF_TEXT_ONLY.replacen("</metadata>", &nested, 1).into());
    let mut entries = legacy_text_only();
    entries[1] = text("OEBPS/content.opf", opf);

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("deep.epub");
    build_epub(&input, &entries);

    let Err(err) = convert(&input, &Options::default(), |_| {}) else {
        panic!("convert accepted a 20,000-level OPF");
    };
    assert!(format!("{err:#}").contains("nesting"), "got: {err:#}");
    let Err(err) = epublift::read_metadata(&input) else {
        panic!("read_metadata accepted a 20,000-level OPF");
    };
    assert!(format!("{err:#}").contains("nesting"), "got: {err:#}");
}

/// The file system decides how `images/` and `Images/` are held, so this passes
/// only if the names come back whichever it is.
#[test]
fn a_folder_spelled_two_ways_keeps_both_spellings() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("two.epub");
    let mut entries = vec![Entry {
        path: "OEBPS/Images/other.jpg",
        data: jpeg(8, 8),
    }];
    entries.extend(legacy_with_images());
    build_epub(&input, &entries);

    let output = dir.path().join("out.epub");
    let options = Options {
        output: Some(output.clone()),
        ..Options::default()
    };
    convert(&input, &options, |_| {}).unwrap();

    let names = entry_names(&output);
    for name in [
        "OEBPS/Images/other.jpg",
        "OEBPS/images/cover.jpg",
        "OEBPS/images/logo.png",
        "OEBPS/nav.xhtml",
    ] {
        assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
    }
}

/// Where the file system holds both files they both come back; where it cannot,
/// the book is refused, and neither file silently replaces the other.
#[test]
fn files_differing_only_in_case_are_kept_or_refused() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("two.epub");
    let mut entries = legacy_with_images();
    entries.push(Entry {
        path: "OEBPS/images/COVER.jpg",
        data: jpeg(8, 8),
    });
    build_epub(&input, &entries);

    let output = dir.path().join("out.epub");
    let options = Options {
        output: Some(output.clone()),
        ..Options::default()
    };
    match convert(&input, &options, |_| {}) {
        Ok(_) => {
            let names = entry_names(&output);
            assert!(names.iter().any(|n| n == "OEBPS/images/cover.jpg"));
            assert!(names.iter().any(|n| n == "OEBPS/images/COVER.jpg"));
        }
        Err(e) => assert!(e.to_string().contains("differ only in case"), "{e}"),
    }
}

#[test]
fn report_goes_beside_the_output() {
    // A run with `-o` must not write into the input's directory, which may be a
    // read-only shared shelf: the default report follows the output.
    let dir = tempfile::tempdir().unwrap();
    let shelf = dir.path().join("shelf");
    let out_dir = dir.path().join("out");
    std::fs::create_dir_all(&shelf).unwrap();
    std::fs::create_dir_all(&out_dir).unwrap();
    let input = shelf.join("legacy.epub");
    build_epub(&input, &legacy_text_only());

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_epublift"))
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(out_dir.join("legacy.epub"))
        .output()
        .unwrap()
        .status;
    assert_ne!(status.code(), Some(2));
    assert!(out_dir.join("legacy_report.txt").is_file());
    let mut left: Vec<_> = std::fs::read_dir(&shelf)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    left.sort();
    assert_eq!(left, ["legacy.epub"], "nothing new beside the input");
}

#[test]
fn a_book_labelled_older_than_epub_2_is_repaired_first() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("old.epub");
    let out = dir.path().join("out.epub");
    let mut entries = legacy_text_only();
    entries[1] = text(
        "OEBPS/content.opf",
        Box::leak(
            OPF_TEXT_ONLY
                .replacen("version=\"2.0\"", "version=\"1.0\"", 1)
                .into_boxed_str(),
        ),
    );
    build_epub(&input, &entries);

    let run = std::process::Command::new(env!("CARGO_BIN_EXE_epublift"))
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert_ne!(run.status.code(), Some(2));
    let said = String::from_utf8_lossy(&run.stdout);
    assert!(said.contains("repairing it first"), "{said}");
    let mut zip = ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut opf = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("OEBPS/content.opf").unwrap(), &mut opf)
        .unwrap();
    assert!(opf.contains("version=\"3.0\""), "{opf}");
}

/// Declare entry `name` 100 bytes long in its local and central headers; the
/// data and the CRC stay intact.
fn understate_size(epub: &Path, name: &str) {
    let mut b = std::fs::read(epub).unwrap();
    let n = name.as_bytes();
    for i in 0..b.len() - 46 {
        if b[i..].starts_with(b"PK\x03\x04") && b[i + 30..].starts_with(n) {
            b[i + 22..i + 26].copy_from_slice(&100u32.to_le_bytes());
        }
        if b[i..].starts_with(b"PK\x01\x02") && b[i + 46..].starts_with(n) {
            b[i + 24..i + 28].copy_from_slice(&100u32.to_le_bytes());
        }
    }
    std::fs::write(epub, b).unwrap();
}

#[test]
fn entry_longer_than_its_header_says_is_read_whole() {
    // zip 9 refuses such an entry; zip 8 and epubcheck read it, and so do we.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.epub");
    build_epub(&input, &legacy_text_only());
    understate_size(&input, "OEBPS/content.opf");

    let report = convert(&input, &Options::default(), |_| {}).unwrap();
    assert!(read_entry(&report.output_path, "OEBPS/content.opf").contains("version=\"3.0\""));
    assert!(epublift::read_metadata(&input).is_ok());
    let eparc = dir.path().join("legacy.eparc");
    epublift::eparc::archive_epub(&input, &eparc).unwrap();
    let restored = dir.path().join("restored.epub");
    epublift::eparc::restore_eparc(&eparc, &restored).unwrap();
    assert!(read_entry(&restored, "OEBPS/content.opf").len() > 100);
}
