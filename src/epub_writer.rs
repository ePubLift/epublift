// epublift - Optimize EPUB files: convert images to WebP and upgrade to EPUB 3.3.
// Copyright (C) 2024  Baris Kayadelen
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Shared minimal EPUB 3.3 writer: package pre-rendered XHTML chapters plus image
//! assets into a valid reflowable EPUB synthesised from scratch.
//!
//! Both importers feed this: PDF (`pdf::reflow`) renders its `Block`s to XHTML,
//! Markdown (`markdown`) renders CommonMark to XHTML — then both hand the result
//! here for OCF/OPF/nav packaging. (The crate's `opf`/`nav` modules rewrite an
//! *existing* EPUB; this one builds one.)

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// One chapter: a plain-text `title` (used for the nav entry and `<title>`) and
/// its inner-`<body>` XHTML — already well-formed and escaped by the caller
/// (including its own heading markup).
pub(crate) struct RenderedChapter {
    pub title: String,
    pub body: String,
}

/// An image to embed under `OEBPS/images/`. `name` is the in-archive filename
/// the chapter bodies reference as `images/{name}`.
pub(crate) struct ImageAsset {
    pub name: String,
    pub media_type: String,
    pub data: Vec<u8>,
}

/// Write `chapters` (+ their `images`) as a reflow EPUB 3.3 to `out`.
pub(crate) fn package_epub(
    out: &Path,
    title: &str,
    language: &str,
    chapters: &[RenderedChapter],
    images: &[ImageAsset],
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

    // Images are already compressed (JPEG/PNG/WebP/…) → store, don't deflate.
    for (i, img) in images.iter().enumerate() {
        zip.start_file(format!("OEBPS/images/{}", img.name), stored)?;
        zip.write_all(&img.data)?;
        manifest.push_str(&format!(
            "  <item id=\"img{:03}\" href=\"images/{}\" media-type=\"{}\"/>\n",
            i + 1,
            img.name,
            img.media_type,
        ));
    }

    for (i, ch) in chapters.iter().enumerate() {
        let id = format!("ch{:03}", i + 1);
        let href = format!("{id}.xhtml");
        let xhtml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="{lang}"><head><title>{t}</title></head>
<body>
{body}</body></html>"#,
            lang = esc(language),
            t = esc(&ch.title),
            body = ch.body,
        );
        zip.start_file(format!("OEBPS/{href}"), deflated)?;
        zip.write_all(xhtml.as_bytes())?;

        manifest.push_str(&format!(
            "  <item id=\"{id}\" href=\"{href}\" media-type=\"application/xhtml+xml\"/>\n"
        ));
        spine.push_str(&format!("  <itemref idref=\"{id}\"/>\n"));
        nav_items.push_str(&format!(
            "    <li><a href=\"{href}\">{}</a></li>\n",
            esc(&ch.title)
        ));
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

/// Escape text for inclusion in XHTML, dropping control chars illegal in XML 1.0.
pub(crate) fn esc(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_xml_and_strips_control_chars() {
        // control char (0x07) dropped; metacharacters escaped
        let out = esc("a <b> & \"c\"\u{0007}");
        assert_eq!(out, "a &lt;b&gt; &amp; &quot;c&quot;");
    }

    #[test]
    fn writes_a_valid_epub() {
        let chapters = vec![RenderedChapter {
            title: "Chapter <1>".to_string(),
            body: "<h1>Chapter &lt;1&gt;</h1>\n<p>Hello &amp; welcome.</p>\n".to_string(),
        }];
        let out =
            std::env::temp_dir().join(format!("epublift_pkg_test_{}.epub", std::process::id()));
        package_epub(&out, "My Title", "en", &chapters, &[]).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_file(&out);

        assert_eq!(&bytes[..2], b"PK", "not a zip");
        // OCF requires the first entry to be an uncompressed `mimetype` whose
        // bytes therefore appear verbatim near the start of the archive.
        assert!(
            bytes.windows(20).any(|w| w == b"application/epub+zip"),
            "mimetype entry missing"
        );
    }
}
