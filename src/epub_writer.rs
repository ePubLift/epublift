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
/// (including its own heading markup). A chapter without a title (the front
/// matter before the first heading) is left out of the nav, and its `<title>`
/// is the book's: nothing is invented for it.
pub(crate) struct RenderedChapter {
    pub title: Option<String>,
    pub body: String,
}

/// An image to embed under `OEBPS/images/`. `name` is the in-archive filename
/// the chapter bodies reference as `images/{name}`.
pub(crate) struct ImageAsset {
    pub name: String,
    pub media_type: String,
    pub data: Vec<u8>,
}

/// Write `chapters` (+ their `images`) as a reflow EPUB 3.3 to `out`. When
/// `cover` is set, it's embedded as the EPUB cover image: a `cover-image`
/// manifest entry, an EPUB2-fallback `<meta name="cover">`, and a `cover.xhtml`
/// page placed first in the spine. `author`, when known, is the `dc:creator`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn package_epub(
    out: &Path,
    title: &str,
    author: Option<&str>,
    language: &str,
    chapters: &[RenderedChapter],
    images: &[ImageAsset],
    cover: Option<&ImageAsset>,
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

    zip.start_file("OEBPS/style.css", deflated)?;
    zip.write_all(STYLESHEET.as_bytes())?;
    manifest.push_str("  <item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n");
    let mut cover_meta = String::new();

    // Cover (optional): the image, a cover page, and the spine entry come first so
    // it opens the book. Already-compressed image bytes are stored, not deflated.
    if let Some(c) = cover {
        zip.start_file(format!("OEBPS/images/{}", c.name), stored)?;
        zip.write_all(&c.data)?;

        let cover_xhtml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="{lang}" lang="{lang}"><head><title>Cover</title>
<style>html,body{{margin:0;padding:0;height:100%;text-align:center}}img{{max-width:100%;max-height:100%}}</style></head>
<body><div><img src="images/{name}" alt="Cover"/></div></body></html>"#,
            lang = esc(language),
            name = c.name,
        );
        zip.start_file("OEBPS/cover.xhtml", deflated)?;
        zip.write_all(cover_xhtml.as_bytes())?;

        manifest.push_str(&format!(
            "  <item id=\"cover-image\" href=\"images/{}\" media-type=\"{}\" properties=\"cover-image\"/>\n",
            c.name, c.media_type,
        ));
        manifest.push_str(
            "  <item id=\"cover\" href=\"cover.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
        );
        spine.push_str("  <itemref idref=\"cover\"/>\n");
        // EPUB2-style cover hint, still honoured by many reading systems / KDP.
        cover_meta.push_str("  <meta name=\"cover\" content=\"cover-image\"/>\n");
    }

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
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="{lang}" lang="{lang}"><head><title>{t}</title>
<link rel="stylesheet" type="text/css" href="style.css"/></head>
<body>
{body}</body></html>"#,
            lang = esc(language),
            t = esc(ch.title.as_deref().unwrap_or(title)),
            body = ch.body,
        );
        zip.start_file(format!("OEBPS/{href}"), deflated)?;
        zip.write_all(xhtml.as_bytes())?;

        manifest.push_str(&format!(
            "  <item id=\"{id}\" href=\"{href}\" media-type=\"application/xhtml+xml\"/>\n"
        ));
        spine.push_str(&format!("  <itemref idref=\"{id}\"/>\n"));
        if let Some(t) = &ch.title {
            nav_items.push_str(&format!("    <li><a href=\"{href}\">{}</a></li>\n", esc(t)));
        }
    }
    // The nav's list may not be empty: a book with no titled chapter gets one
    // entry, the book's title, at its start.
    if nav_items.is_empty() && !chapters.is_empty() {
        nav_items.push_str(&format!(
            "    <li><a href=\"ch001.xhtml\">{}</a></li>\n",
            esc(title)
        ));
    }

    zip.start_file("OEBPS/nav.xhtml", deflated)?;
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="{lang}" lang="{lang}">
<head><title>Contents</title>
<link rel="stylesheet" type="text/css" href="style.css"/></head>
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
  <dc:identifier id="bookid">urn:uuid:{uuid}</dc:identifier>
  <dc:title>{title}</dc:title>
{creator}  <dc:language>{lang}</dc:language>
  <meta property="dcterms:modified">{modified}</meta>
{cover_meta} </metadata>
 <manifest>
  <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
{manifest} </manifest>
 <spine>
{spine} </spine>
</package>"#,
            // A new book with no ISBN: a random UUID, made once (each import
            // is a book of its own, so two never share an identifier).
            uuid = uuid::Uuid::new_v4(),
            modified = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
            title = esc(title),
            creator = author
                .map(|a| format!("  <dc:creator>{}</dc:creator>\n", esc(a)))
                .unwrap_or_default(),
            lang = esc(language),
            cover_meta = cover_meta,
        )
        .as_bytes(),
    )?;

    zip.finish()?;
    Ok(())
}

/// A plain book style: indented, justified, hyphenated paragraphs; headings,
/// the paragraph after them and figures not indented; images never wider or
/// taller than the screen. A reader's own settings still win where they are set.
const STYLESHEET: &str = "\
body { margin: 0; }
p { margin: 0; text-indent: 1.2em; text-align: justify; -webkit-hyphens: auto; hyphens: auto; }
h1, h2, h3, h4, h5, h6 { text-align: left; -webkit-hyphens: manual; hyphens: manual; page-break-after: avoid; break-after: avoid; }
h1 { margin: 1em 0 1.2em; }
h2, h3, h4, h5, h6 { margin: 1.4em 0 0.6em; }
h1 + p, h2 + p, h3 + p, h4 + p, h5 + p, h6 + p, div.figure + p { text-indent: 0; }
div.figure { margin: 1em 0; text-align: center; text-indent: 0; page-break-inside: avoid; break-inside: avoid; }
img { max-width: 100%; max-height: 95vh; height: auto; }
";

/// Escape text for inclusion in XHTML, dropping chars illegal in XML 1.0.
pub(crate) fn esc(s: &str) -> String {
    s.chars()
        // XML 1.0 `Char`: no C0 controls but tab/newline/CR, and no U+FFFE or
        // U+FFFF (fonts' ToUnicode maps give them to unmapped glyphs).
        .filter(|&c| {
            matches!(c, '\t' | '\n' | '\r' | ' '..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}')
                || c >= '\u{10000}'
        })
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
        // noncharacters U+FFFE / U+FFFF dropped; the rest of the BMP and
        // astral chars kept
        assert_eq!(
            esc("x\u{FFFE}y\u{FFFF}z\u{FFFD}\u{1F600}"),
            "xyz\u{FFFD}\u{1F600}"
        );
    }

    #[test]
    fn writes_a_valid_epub() {
        let chapters = vec![RenderedChapter {
            title: Some("Chapter <1>".to_string()),
            body: "<h1>Chapter &lt;1&gt;</h1>\n<p>Hello &amp; welcome.</p>\n".to_string(),
        }];
        let out =
            std::env::temp_dir().join(format!("epublift_pkg_test_{}.epub", std::process::id()));
        package_epub(&out, "My Title", None, "en", &chapters, &[], None).unwrap();
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

    /// Every XHTML file has a DOCTYPE and `lang`: EPUB 3 does not require the
    /// DOCTYPE, but Sigil "cleans" a file without one on open, and says so.
    #[test]
    fn every_xhtml_file_has_a_doctype_and_lang() {
        let chapters = vec![RenderedChapter {
            title: Some("One".to_string()),
            body: "<p>x</p>\n".to_string(),
        }];
        let cover = ImageAsset {
            name: "cover.png".to_string(),
            media_type: "image/png".to_string(),
            data: vec![0x89, b'P', b'N', b'G'],
        };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("book.epub");
        package_epub(
            &out,
            "T",
            Some("A. Writer"),
            "tr",
            &chapters,
            &[],
            Some(&cover),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut seen = 0;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).unwrap();
            if !f.name().unwrap().ends_with(".xhtml") {
                continue;
            }
            let mut text = String::new();
            std::io::Read::read_to_string(&mut f, &mut text).unwrap();
            assert!(text.contains("<!DOCTYPE html>"), "{}", f.name().unwrap());
            assert!(text.contains(r#" lang="tr""#), "{}", f.name().unwrap());
            seen += 1;
        }
        assert_eq!(seen, 3, "cover, chapter and nav");
    }

    fn nav_of(chapters: Vec<RenderedChapter>) -> String {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("book.epub");
        package_epub(&out, "The Book", None, "en", &chapters, &[], None).unwrap();
        let report = epubveri::validate_bytes(std::fs::read(&out).unwrap());
        assert_eq!(report.fatals() + report.errors(), 0);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut nav = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("OEBPS/nav.xhtml").unwrap(), &mut nav)
            .unwrap();
        nav
    }

    /// A chapter without a title gets no made-up name: the nav leaves it out,
    /// unless no chapter has one, when the book's title stands for it.
    #[test]
    fn an_untitled_chapter_is_left_out_of_the_nav() {
        let ch = |title: Option<&str>| RenderedChapter {
            title: title.map(str::to_string),
            body: "<p>x</p>\n".to_string(),
        };
        let nav = nav_of(vec![ch(None), ch(Some("One")), ch(Some("Two"))]);
        assert!(!nav.contains("ch001.xhtml"), "{nav}");
        assert!(nav.contains(r#"<a href="ch002.xhtml">One</a>"#), "{nav}");
        assert!(!nav.contains("Section"), "{nav}");

        let nav = nav_of(vec![ch(None), ch(None)]);
        assert!(
            nav.contains(r#"<a href="ch001.xhtml">The Book</a>"#),
            "{nav}"
        );
        assert_eq!(nav.matches("<li>").count(), 1, "{nav}");
    }
}
