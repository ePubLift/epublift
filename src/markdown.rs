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

//! [EXPERIMENTAL] Markdown → EPUB import (`epublift import file.md`).
//!
//! Turns a CommonMark file — typically the markdown an AI OCR tool (e.g. Mistral
//! OCR) emits for a PDF, alongside an `images/` folder — into a reflowable EPUB.
//! This is the pure-Rust, fully offline core; the AI "Smart Import" layer simply
//! produces the markdown this consumes.
//!
//! Pipeline: parse with `pulldown-cmark` → render the event stream to well-formed
//! XHTML (CommonMark's HTML output isn't valid XML), splitting into chapters at
//! top-level `#` headings and embedding referenced local images → hand the
//! rendered chapters to [`crate::epub_writer`] for OCF/OPF/nav packaging.
//!
//! Scope (v1): headings, paragraphs, emphasis/strong/strikethrough, inline &
//! fenced code, lists, blockquotes, tables, links, horizontal rules, and local
//! image embedding. Raw HTML blocks are dropped (to keep the output valid XHTML);
//! footnotes and task lists are not enabled yet.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::epub_writer::{ImageAsset, RenderedChapter, esc, package_epub};

/// WebP quality used when re-encoding a cover image (matches the convert default).
const COVER_QUALITY: u8 = 80;

/// Options controlling a Markdown import.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Content language (BCP-47, e.g. "tr"). Sets `dc:language`; defaults to "en".
    pub language: Option<String>,
}

/// What an import produced (for the CLI/web to report).
#[derive(Debug, Clone, Copy)]
pub struct ImportSummary {
    pub chapters: usize,
    pub images: usize,
}

/// Import the single Markdown file at `input`, writing a reflow EPUB to `output`.
/// The book title is taken from the first `#` heading, falling back to the
/// filename. (A thin wrapper over [`import_collection`].)
pub fn import(input: &Path, output: &Path, opts: &ImportOptions) -> Result<ImportSummary> {
    import_collection(
        std::slice::from_ref(&input.to_path_buf()),
        None,
        output,
        None,
        opts,
    )
}

/// Import a *folder* of Markdown: every `.md`/`.markdown` file in `dir` (sorted
/// by filename, so `00_…`, `01_… ` chapter prefixes order correctly) is appended
/// in turn, and a `cover.*` image in the folder becomes the EPUB cover. The book
/// title defaults to the folder name.
pub fn import_dir(dir: &Path, output: &Path, opts: &ImportOptions) -> Result<ImportSummary> {
    let (files, cover) = collect_markdown_dir(dir)?;
    if files.is_empty() {
        bail!("no .md / .markdown files in {}", dir.display());
    }
    let title = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty());
    import_collection(&files, cover.as_deref(), output, title.as_deref(), opts)
}

/// Import an in-memory `.zip` of Markdown-plus-images: it's safely unpacked
/// (hardened against zip-slip and zip bombs), every `.md`/`.markdown` is appended
/// in filename order, and a `cover.*` image becomes the EPUB cover. `title` sets
/// the book title (typically the uploaded zip's name).
pub fn import_zip(
    bytes: &[u8],
    output: &Path,
    title: Option<&str>,
    opts: &ImportOptions,
) -> Result<ImportSummary> {
    let tmp = tempfile::Builder::new()
        .prefix("epublift_md_zip_")
        .tempdir()
        .context("failed to create a temp dir for the zip")?;
    let (files, cover) = extract_markdown_zip(bytes, tmp.path())?;
    if files.is_empty() {
        bail!("the .zip has no .md / .markdown file");
    }
    import_collection(&files, cover.as_deref(), output, title, opts)
}

/// Import one or more Markdown `files` (appended in the given order) into a single
/// reflow EPUB at `output`, optionally embedding `cover` as the EPUB cover image.
///
/// `title` sets the book title; when `None` it falls back to the first `#` heading
/// across all files, then to the first file's name. Local images referenced by
/// each file resolve relative to *that file's* folder, so a multi-folder zip works.
pub fn import_collection(
    files: &[PathBuf],
    cover: Option<&Path>,
    output: &Path,
    title: Option<&str>,
    opts: &ImportOptions,
) -> Result<ImportSummary> {
    if files.is_empty() {
        bail!("no markdown files to import");
    }
    let language = opts.language.as_deref().unwrap_or("en").to_string();

    let mut r = Renderer::new();
    for file in files {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        r.base_dir = file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        r.run(&text);
    }
    let Renderer {
        chapters,
        images,
        first_h1,
        ..
    } = r;

    if chapters.is_empty() {
        bail!("no content found in the markdown input");
    }

    let title_fallback = || {
        files[0]
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Imported book".into())
    };
    let book_title = title
        .map(str::to_string)
        .or(first_h1)
        .unwrap_or_else(title_fallback);

    let n = chapters.len();
    let rendered: Vec<RenderedChapter> = chapters
        .into_iter()
        .enumerate()
        .map(|(i, (title, body))| {
            let title = title.filter(|t| !t.is_empty()).unwrap_or_else(|| {
                if n == 1 {
                    book_title.clone()
                } else {
                    format!("Section {}", i + 1)
                }
            });
            RenderedChapter { title, body }
        })
        .collect();

    let cover_asset = match cover {
        Some(path) => load_cover(path)?,
        None => None,
    };

    package_epub(
        output,
        &book_title,
        &language,
        &rendered,
        &images,
        cover_asset.as_ref(),
    )?;
    Ok(ImportSummary {
        chapters: rendered.len(),
        images: images.len() + cover_asset.is_some() as usize,
    })
}

/// Read a cover image and prepare it for embedding, re-encoding to WebP when that
/// comes out smaller (size-safe — same rule as the EPUB optimizer). Returns
/// `None` (with a warning) if the file can't be read or isn't a known image type.
fn load_cover(path: &Path) -> Result<Option<ImageAsset>> {
    let data = std::fs::read(path)
        .with_context(|| format!("failed to read cover image {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if let Some(webp) = crate::images::optimize_to_webp(&data, COVER_QUALITY) {
        return Ok(Some(ImageAsset {
            name: "cover.webp".into(),
            media_type: "image/webp".into(),
            data: webp,
        }));
    }
    match ext.as_deref().and_then(media_type_for) {
        Some(media_type) => Ok(Some(ImageAsset {
            name: format!("cover.{}", ext.unwrap()),
            media_type: media_type.to_string(),
            data,
        })),
        None => {
            eprintln!(
                "[EXPERIMENTAL] warning: ignoring cover '{}' (unsupported image type)",
                path.display()
            );
            Ok(None)
        }
    }
}

/// Find the `.md`/`.markdown` files (sorted by path) and an optional `cover.*`
/// image directly inside `dir` (non-recursive for files; cover is the shallowest).
fn collect_markdown_dir(dir: &Path) -> Result<(Vec<PathBuf>, Option<PathBuf>)> {
    let mut files = Vec::new();
    let mut cover = None;
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read folder {}", dir.display()))?
    {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        if is_markdown(&path) {
            files.push(path);
        } else if cover.is_none() && is_cover(&path) {
            cover = Some(path);
        }
    }
    files.sort();
    Ok((files, cover))
}

/// True if `path` has a Markdown extension.
fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md" | "markdown" | "mdown" | "mkd" | "mkdn")
    )
}

/// True if `path` is a `cover.<img-ext>` file (the conventional cover name).
fn is_cover(path: &Path) -> bool {
    let stem_is_cover = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("cover"))
        .unwrap_or(false);
    let is_image = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        .and_then(media_type_for)
        .is_some();
    stem_is_cover && is_image
}

/// Safely unpack a `.zip` into `dest`, returning the Markdown files (sorted by
/// in-archive path) and an optional `cover.*` image (the shallowest one).
///
/// Hardened against zip-slip (entries that escape `dest` are skipped) and zip
/// bombs (caps on entry count and total uncompressed size).
fn extract_markdown_zip(bytes: &[u8], dest: &Path) -> Result<(Vec<PathBuf>, Option<PathBuf>)> {
    use std::io::Cursor;

    const MAX_ENTRIES: usize = 5_000;
    const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024; // 200 MiB uncompressed

    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| anyhow::anyhow!("not a valid .zip: {e}"))?;
    if zip.len() > MAX_ENTRIES {
        bail!("the .zip has too many entries");
    }

    let mut total: u64 = 0;
    let mut md_files: Vec<PathBuf> = Vec::new();
    let mut covers: Vec<PathBuf> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // `enclosed_name` returns None for entries that would escape the target
        // (absolute paths, `..`) — the zip-slip guard.
        let rel = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        if entry.is_dir() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_TOTAL_BYTES {
            bail!("the .zip's contents are too large");
        }
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;

        if is_markdown(&rel) {
            md_files.push(out);
        } else if is_cover(&rel) {
            covers.push(out);
        }
    }

    // Markdown in filename order; cover = the shallowest (fewest path components).
    md_files.sort();
    covers.sort_by_key(|p| p.components().count());
    Ok((md_files, covers.into_iter().next()))
}

/// A `![alt](url)` being collected: we buffer the alt text, then resolve & embed
/// (or reference) the image when the tag closes.
struct PendingImage {
    url: String,
    alt: String,
}

/// Renders a CommonMark event stream into per-chapter XHTML bodies. A single
/// renderer can process several files in turn (set [`Renderer::base_dir`] before
/// each [`Renderer::run`]); chapters and images accumulate across them, so image
/// names stay globally unique.
struct Renderer {
    /// Folder the *current* file's relative image paths resolve against.
    base_dir: PathBuf,
    /// XHTML accumulated for the current chapter's `<body>`.
    body: String,
    /// Finished chapters: (optional title, body XHTML).
    chapters: Vec<(Option<String>, String)>,
    images: Vec<ImageAsset>,
    /// Title of the chapter currently being built (set from its `#` heading).
    cur_title: Option<String>,
    /// First `#` heading in the whole document — used as the book title.
    first_h1: Option<String>,
    /// Open container blocks (list/quote/table/item): a `#` only starts a new
    /// chapter at the top level (depth 0), never inside a list or quote.
    depth: usize,
    /// When inside a heading, collects its plain text (for the nav/title).
    heading_plain: Option<String>,
    /// When inside an image, collects its alt text.
    image: Option<PendingImage>,
    in_table_head: bool,
}

impl Renderer {
    fn new() -> Self {
        Renderer {
            base_dir: PathBuf::from("."),
            body: String::new(),
            chapters: Vec::new(),
            images: Vec::new(),
            cur_title: None,
            first_h1: None,
            depth: 0,
            heading_plain: None,
            image: None,
            in_table_head: false,
        }
    }

    /// Push literal XHTML markup (suppressed while collecting an image's alt).
    fn raw(&mut self, s: &str) {
        if self.image.is_none() {
            self.body.push_str(s);
        }
    }

    /// Push text content: routed to the image alt or heading-title buffer when
    /// one is open, and always escaped into the body.
    fn text(&mut self, s: &str) {
        if let Some(img) = &mut self.image {
            img.alt.push_str(s);
            return;
        }
        if let Some(h) = &mut self.heading_plain {
            h.push_str(s);
        }
        self.body.push_str(&esc(s));
    }

    /// Finalise the current chapter if it has any content.
    fn flush_chapter(&mut self) {
        let body = std::mem::take(&mut self.body);
        let title = self.cur_title.take();
        if body.trim().is_empty() {
            return;
        }
        self.chapters.push((title, body));
    }

    /// Resolve a closed `![alt](url)`: embed a local image, reference a remote
    /// one as-is, or fall back to the alt text if it can't be embedded.
    fn finish_image(&mut self) {
        let Some(img) = self.image.take() else {
            return;
        };
        let alt = esc(&img.alt);

        // Remote images are referenced as-is (not embedded — keeps us offline).
        if img.url.starts_with("http://") || img.url.starts_with("https://") {
            self.body
                .push_str(&format!("<img src=\"{}\" alt=\"{}\"/>", esc(&img.url), alt));
            return;
        }

        let path = self.base_dir.join(&img.url);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let media = ext.as_deref().and_then(media_type_for);
        match (media, std::fs::read(&path)) {
            (Some(media_type), Ok(data)) => {
                let name = format!("img{:03}.{}", self.images.len() + 1, ext.unwrap());
                self.body
                    .push_str(&format!("<img src=\"images/{name}\" alt=\"{alt}\"/>"));
                self.images.push(ImageAsset {
                    name,
                    media_type: media_type.to_string(),
                    data,
                });
            }
            _ => {
                // Missing file or unsupported type: keep the alt text so no
                // content is silently lost, and warn.
                eprintln!(
                    "[EXPERIMENTAL] warning: skipping image '{}' (not found or unsupported type)",
                    img.url
                );
                if !alt.is_empty() {
                    self.body.push_str(&alt);
                }
            }
        }
    }

    fn run(&mut self, text: &str) {
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        for event in Parser::new_ext(text, opts) {
            self.handle(event);
        }
        self.flush_chapter();
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                if let Some(img) = &mut self.image {
                    img.alt.push_str(&t);
                } else {
                    if let Some(h) = &mut self.heading_plain {
                        h.push_str(&t);
                    }
                    self.body.push_str("<code>");
                    self.body.push_str(&esc(&t));
                    self.body.push_str("</code>");
                }
            }
            Event::SoftBreak => {
                if let Some(img) = &mut self.image {
                    img.alt.push(' ');
                } else {
                    self.body.push('\n');
                }
            }
            Event::HardBreak => self.raw("<br/>\n"),
            Event::Rule => self.raw("<hr/>\n"),
            // Drop raw HTML to keep the output well-formed XHTML.
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::TaskListMarker(checked) => self.text(if checked { "[x] " } else { "[ ] " }),
            Event::FootnoteReference(_) => {}
            // Math isn't enabled; if it ever appears, keep the source as text.
            Event::InlineMath(t) | Event::DisplayMath(t) => self.text(&t),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.raw("<p>"),
            Tag::Heading { level, .. } => {
                let n = h_num(level);
                if n == 1 && self.depth == 0 {
                    self.flush_chapter();
                }
                self.raw(&format!("<h{n}>"));
                self.heading_plain = Some(String::new());
            }
            Tag::BlockQuote(_) => {
                self.depth += 1;
                self.raw("<blockquote>\n");
            }
            Tag::CodeBlock(kind) => {
                let cls = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        format!(" class=\"language-{}\"", esc(&lang))
                    }
                    _ => String::new(),
                };
                self.raw(&format!("<pre><code{cls}>"));
            }
            Tag::List(Some(start)) => {
                self.depth += 1;
                if start == 1 {
                    self.raw("<ol>\n");
                } else {
                    self.raw(&format!("<ol start=\"{start}\">\n"));
                }
            }
            Tag::List(None) => {
                self.depth += 1;
                self.raw("<ul>\n");
            }
            Tag::Item => {
                self.depth += 1;
                self.raw("<li>");
            }
            Tag::Emphasis => self.raw("<em>"),
            Tag::Strong => self.raw("<strong>"),
            Tag::Strikethrough => self.raw("<del>"),
            Tag::Link { dest_url, .. } => self.raw(&format!("<a href=\"{}\">", esc(&dest_url))),
            Tag::Image { dest_url, .. } => {
                self.image = Some(PendingImage {
                    url: dest_url.to_string(),
                    alt: String::new(),
                });
            }
            Tag::Table(_) => {
                self.depth += 1;
                self.raw("<table>\n");
            }
            Tag::TableHead => {
                self.depth += 1;
                self.in_table_head = true;
                self.raw("<thead>\n<tr>");
            }
            Tag::TableRow => {
                self.depth += 1;
                self.raw("<tr>");
            }
            Tag::TableCell => {
                self.depth += 1;
                self.raw(if self.in_table_head { "<th>" } else { "<td>" });
            }
            // Raw HTML blocks, footnote defs, metadata blocks: ignored (their
            // inner Text/Html events are dropped too).
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.raw("</p>\n"),
            TagEnd::Heading(level) => {
                let n = h_num(level);
                self.raw(&format!("</h{n}>\n"));
                let plain = self.heading_plain.take().unwrap_or_default();
                let plain = plain.trim().to_string();
                if n == 1 {
                    if self.cur_title.is_none() {
                        self.cur_title = Some(plain.clone());
                    }
                    if self.first_h1.is_none() {
                        self.first_h1 = Some(plain);
                    }
                }
            }
            TagEnd::BlockQuote(_) => {
                self.depth = self.depth.saturating_sub(1);
                self.raw("</blockquote>\n");
            }
            TagEnd::CodeBlock => self.raw("</code></pre>\n"),
            TagEnd::List(ordered) => {
                self.depth = self.depth.saturating_sub(1);
                self.raw(if ordered { "</ol>\n" } else { "</ul>\n" });
            }
            TagEnd::Item => {
                self.depth = self.depth.saturating_sub(1);
                self.raw("</li>\n");
            }
            TagEnd::Emphasis => self.raw("</em>"),
            TagEnd::Strong => self.raw("</strong>"),
            TagEnd::Strikethrough => self.raw("</del>"),
            TagEnd::Link => self.raw("</a>"),
            TagEnd::Image => self.finish_image(),
            TagEnd::Table => {
                self.depth = self.depth.saturating_sub(1);
                self.raw("</table>\n");
            }
            TagEnd::TableHead => {
                self.depth = self.depth.saturating_sub(1);
                self.in_table_head = false;
                self.raw("</tr>\n</thead>\n");
            }
            TagEnd::TableRow => {
                self.depth = self.depth.saturating_sub(1);
                self.raw("</tr>\n");
            }
            TagEnd::TableCell => {
                self.depth = self.depth.saturating_sub(1);
                self.raw(if self.in_table_head { "</th>" } else { "</td>" });
            }
            _ => {}
        }
    }
}

fn h_num(level: HeadingLevel) -> u8 {
    use HeadingLevel::*;
    match level {
        H1 => 1,
        H2 => 2,
        H3 => 3,
        H4 => 4,
        H5 => 5,
        H6 => 6,
    }
}

/// Map a lowercase file extension to an image media type, or `None` if we don't
/// recognise it as an embeddable image.
fn media_type_for(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "jxl" => "image/jxl",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(md: &str) -> Renderer {
        let mut r = Renderer::new();
        r.run(md);
        r
    }

    #[test]
    fn splits_chapters_at_top_level_h1() {
        let r = render("# One\n\nalpha\n\n# Two\n\nbeta\n");
        assert_eq!(r.chapters.len(), 2);
        assert_eq!(r.chapters[0].0.as_deref(), Some("One"));
        assert_eq!(r.chapters[1].0.as_deref(), Some("Two"));
        assert_eq!(r.first_h1.as_deref(), Some("One"));
    }

    #[test]
    fn h1_inside_a_list_does_not_split() {
        // A '#' only starts a chapter at the top level.
        let r = render("# Top\n\n- item\n- item\n");
        assert_eq!(r.chapters.len(), 1);
    }

    #[test]
    fn renders_inline_formatting_as_xhtml() {
        let r = render("# T\n\nsome **bold**, *italic*, `code` and ~~gone~~.\n");
        let body = &r.chapters[0].1;
        assert!(body.contains("<strong>bold</strong>"));
        assert!(body.contains("<em>italic</em>"));
        assert!(body.contains("<code>code</code>"));
        assert!(body.contains("<del>gone</del>"));
    }

    #[test]
    fn escapes_xml_in_text() {
        let r = render("# T\n\n5 < 6 & 7 > 2\n");
        assert!(r.chapters[0].1.contains("5 &lt; 6 &amp; 7 &gt; 2"));
    }

    #[test]
    fn drops_raw_html_to_stay_well_formed() {
        let r = render("# T\n\n<div>raw</div>\n\nafter\n");
        let body = &r.chapters[0].1;
        assert!(!body.contains("<div>"));
        assert!(body.contains("after"));
    }

    #[test]
    fn missing_image_falls_back_to_alt_text() {
        let r = render("# T\n\n![a caption](nope.png)\n");
        let body = &r.chapters[0].1;
        assert!(!body.contains("<img"));
        assert!(body.contains("a caption"));
    }

    #[test]
    fn embeds_a_local_image() {
        // 1x1 transparent PNG.
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let dir = std::env::temp_dir().join(format!("epublift_md_img_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pic.png"), PNG).unwrap();
        let md = dir.join("book.md");
        std::fs::write(&md, "# Title\n\n![cover](pic.png)\n").unwrap();

        let out = dir.join("book.epub");
        let summary = import(&md, &out, &ImportOptions::default()).unwrap();
        assert_eq!(summary.images, 1);
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        assert!(bytes.windows(20).any(|w| w == b"application/epub+zip"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Read one entry out of a written EPUB (a zip) as a UTF-8 string.
    fn read_entry(epub: &Path, name: &str) -> String {
        use std::io::Read;
        let f = std::fs::File::open(epub).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let mut e = zip.by_name(name).unwrap();
        let mut s = String::new();
        e.read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn folder_of_markdown_becomes_one_book_with_a_cover() {
        // A non-trivial PNG that actually shrinks when re-encoded to WebP, so the
        // cover exercises the size-safe optimization path.
        let img = image::RgbImage::from_fn(256, 256, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let dir = std::env::temp_dir().join(format!("epublift_md_dir_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        image::DynamicImage::ImageRgb8(img)
            .save(dir.join("cover.png"))
            .unwrap();
        // Filenames intentionally out of read order; the import sorts them.
        std::fs::write(dir.join("01_second.md"), "# Beta\n\nsecond chapter\n").unwrap();
        std::fs::write(dir.join("00_first.md"), "# Alpha\n\nfirst chapter\n").unwrap();

        let out = dir.join("DEHB.epub");
        let summary = import_dir(&dir, &out, &ImportOptions::default()).unwrap();
        // Two `#` chapters + the cover image.
        assert_eq!(summary.chapters, 2);
        assert_eq!(summary.images, 1);

        let opf = read_entry(&out, "OEBPS/content.opf");
        // Title defaults to the folder name, and the cover is wired up both ways.
        assert!(opf.contains("<dc:title>") && opf.contains("epublift_md_dir"));
        assert!(opf.contains("properties=\"cover-image\""));
        assert!(opf.contains("<meta name=\"cover\" content=\"cover-image\"/>"));
        // Chapters are appended in sorted filename order.
        assert!(read_entry(&out, "OEBPS/ch001.xhtml").contains("first chapter"));
        assert!(read_entry(&out, "OEBPS/ch002.xhtml").contains("second chapter"));
        // The cover page is present and the cover was optimized to WebP.
        assert!(read_entry(&out, "OEBPS/cover.xhtml").contains("images/cover.webp"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_of_markdown_imports_every_file() {
        use std::io::Write;
        // Build an in-memory zip with two chapters under a top-level folder.
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("book/00_a.md", opts).unwrap();
            zw.write_all(b"# One\n\nalpha body\n").unwrap();
            zw.start_file("book/01_b.md", opts).unwrap();
            zw.write_all(b"# Two\n\nbeta body\n").unwrap();
            zw.finish().unwrap();
        }

        let out =
            std::env::temp_dir().join(format!("epublift_md_zip_test_{}.epub", std::process::id()));
        let summary = import_zip(&buf, &out, Some("My Book"), &ImportOptions::default()).unwrap();
        assert_eq!(summary.chapters, 2);

        let opf = read_entry(&out, "OEBPS/content.opf");
        assert!(opf.contains("<dc:title>My Book</dc:title>"));
        assert!(read_entry(&out, "OEBPS/ch001.xhtml").contains("alpha body"));
        assert!(read_entry(&out, "OEBPS/ch002.xhtml").contains("beta body"));

        let _ = std::fs::remove_file(&out);
    }
}
