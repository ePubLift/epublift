//! Emit a reflowable EPUB 3.3 from structured chapters.
//!
//! A self-contained minimal writer (the crate's `opf`/`nav` modules rewrite an
//! *existing* EPUB; here we synthesise one from scratch). Deeper reuse of those
//! writers can come later.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::structure::Chapter;

/// Write `chapters` as a reflow EPUB to `out`.
pub(crate) fn write_epub(
    out: &Path,
    title: &str,
    language: &str,
    chapters: &[Chapter],
) -> Result<()> {
    let file = std::fs::File::create(out)
        .with_context(|| format!("failed to create {}", out.display()))?;
    let mut zip = ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // mimetype: first entry, stored uncompressed (OCF requirement).
    zip.start_file("mimetype", stored)?;
    zip.write_all(b"application/epub+zip")?;

    zip.start_file("META-INF/container.xml", deflated)?;
    zip.write_all(
        br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#,
    )?;

    let mut manifest = String::new();
    let mut spine = String::new();
    let mut nav_items = String::new();
    for (i, ch) in chapters.iter().enumerate() {
        let id = format!("ch{:03}", i + 1);
        let href = format!("{id}.xhtml");
        let heading = ch.title.clone().unwrap_or_else(|| format!("Section {}", i + 1));

        let mut body = format!("<h1>{}</h1>\n", esc(&heading));
        for p in &ch.paragraphs {
            body.push_str(&format!("<p>{}</p>\n", esc(p)));
        }
        let xhtml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="{lang}"><head><title>{t}</title></head>
<body>
{body}</body></html>"#,
            lang = esc(language),
            t = esc(&heading),
        );
        zip.start_file(format!("OEBPS/{href}"), deflated)?;
        zip.write_all(xhtml.as_bytes())?;

        manifest.push_str(&format!(
            "  <item id=\"{id}\" href=\"{href}\" media-type=\"application/xhtml+xml\"/>\n"
        ));
        spine.push_str(&format!("  <itemref idref=\"{id}\"/>\n"));
        nav_items.push_str(&format!("    <li><a href=\"{href}\">{}</a></li>\n", esc(&heading)));
    }

    zip.start_file("OEBPS/nav.xhtml", deflated)?;
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="{lang}">
<head><title>Contents</title></head>
<body><nav epub:type="toc"><h1>Contents</h1><ol>
{nav_items}</ol></nav></body></html>"#,
            lang = esc(language),
        )
        .as_bytes(),
    )?;

    zip.start_file("OEBPS/content.opf", deflated)?;
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
 <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:identifier id="bookid">urn:uuid:epublift-import-{n}</dc:identifier>
  <dc:title>{title}</dc:title>
  <dc:language>{lang}</dc:language>
  <meta property="dcterms:modified">2026-01-01T00:00:00Z</meta>
 </metadata>
 <manifest>
  <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
{manifest} </manifest>
 <spine>
{spine} </spine>
</package>"#,
            n = chapters.len(),
            title = esc(title),
            lang = esc(language),
        )
        .as_bytes(),
    )?;

    zip.finish()?;
    Ok(())
}

fn esc(s: &str) -> String {
    s.chars()
        // Drop control chars that are illegal in XML 1.0 (keep tab/newline/CR).
        .filter(|&c| c >= ' ' || c == '\t' || c == '\n' || c == '\r')
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            other => other.to_string(),
        })
        .collect()
}
