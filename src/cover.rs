//! The book's cover image: read the one it declares, and replace it (or add
//! one) with a JPEG or PNG the user chose — `meta set --cover`, `meta enrich
//! --cover` and the web Metadata form. See `docs/metadata.md` § Cover.
//!
//! The image goes in **as given**: no re-encode, no resize (Optimize does
//! that, and Kobo shows no WebP cover in a plain `.epub`). What has to change
//! around it is measured on our 544-book shelf:
//!
//! - 91% show the cover on the first spine page; 74% wrap it in an
//!   `<svg viewBox="0 0 W H">` sized to the old image, which would stretch a
//!   new image of another shape, so the viewBox (and an `<image>`/`<img>` that
//!   carries the old size) is set to the new size;
//! - 43 use the cover image in more pages (a title page, Gutenberg's
//!   `<link rel="icon">`); every reference follows the new file;
//! - 6% declare no cover: the image is declared and a cover page is put first
//!   in the spine; existing pages are not touched.
//!
//! A reference is rewritten only where it **resolves** to the cover's path —
//! never by matching a file name in text, which would also hit `backcover.jpg`.

use crate::util::{parse_xml, parse_xml_with_dtd, quote, unquote};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::ops::Range;

/// The largest cover we accept (the web form says the same).
pub const MAX_COVER_BYTES: usize = 10 * 1024 * 1024;

/// The two formats a cover may be given in: what every reading system shows,
/// Kobo's library view included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverFormat {
    Jpeg,
    Png,
}

impl CoverFormat {
    pub fn media_type(self) -> &'static str {
        match self {
            CoverFormat::Jpeg => "image/jpeg",
            CoverFormat::Png => "image/png",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            CoverFormat::Jpeg => "jpg",
            CoverFormat::Png => "png",
        }
    }

    /// Recognise the format from the file's first bytes (never its name).
    fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(CoverFormat::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(CoverFormat::Png)
        } else {
            None
        }
    }
}

/// A validated new cover: JPEG or PNG by content, at most [`MAX_COVER_BYTES`],
/// fully decodable within the decode-bomb limits.
#[derive(Debug, Clone)]
pub struct CoverImage {
    pub bytes: Vec<u8>,
    pub format: CoverFormat,
    pub width: u32,
    pub height: u32,
}

impl CoverImage {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() > MAX_COVER_BYTES {
            bail!(
                "the cover image is {:.1} MB; the limit is {} MB",
                bytes.len() as f64 / (1024.0 * 1024.0),
                MAX_COVER_BYTES / (1024 * 1024)
            );
        }
        let format =
            CoverFormat::sniff(&bytes).context("the cover image must be a JPEG or PNG file")?;
        // A full decode, not just the header: a truncated or corrupt file would
        // otherwise go into the book and show as a blank cover.
        let img = crate::images::decode_image(&bytes)
            .context("the cover image could not be read (damaged, or too large to decode)")?;
        Ok(Self {
            width: img.width(),
            height: img.height(),
            format,
            bytes,
        })
    }
}

/// The cover a book declares now.
#[derive(Debug, Clone)]
pub struct CurrentCover {
    /// Its path inside the EPUB.
    pub path: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
    /// From the image header; `None` for a format we cannot read the size of.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// What [`apply_cover`] did, for the report.
#[derive(Debug, Clone, Default)]
pub struct CoverChange {
    /// The old cover's path, when there was one.
    pub replaced: Option<String>,
    /// The new cover's path.
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Content documents whose references or cover sizing were updated.
    pub pages_updated: Vec<String>,
    /// The cover page added at the start of the spine (a book with no cover).
    pub cover_page_added: Option<String>,
    /// The old image stayed in the book because a page that uses it could not
    /// be parsed to update (it keeps a manifest entry, so nothing dangles).
    pub kept_old_image: bool,
}

/// An EPUB held in memory: entry names and contents, in archive order.
pub type Entries = Vec<(String, Vec<u8>)>;

fn entry<'a>(entries: &'a Entries, name: &str) -> Option<&'a Vec<u8>> {
    entries.iter().find(|(n, _)| n == name).map(|(_, b)| b)
}

fn set_entry(entries: &mut Entries, name: &str, bytes: Vec<u8>) {
    match entries.iter_mut().find(|(n, _)| n == name) {
        Some((_, b)) => *b = bytes,
        None => entries.push((name.to_string(), bytes)),
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// The directory part of an entry path (`""` for the root).
fn dir_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// Resolve `href` (as written in a document in `base_dir`) to an entry path.
/// `None` for an external URL, a pure fragment, or a path above the root.
fn resolve(base_dir: &str, href: &str) -> Option<String> {
    let href = href.trim();
    let href = href.split(['#', '?']).next().unwrap_or("");
    if href.is_empty() {
        return None;
    }
    // A scheme (`http:`, `data:`, `urn:`) before any `/` means "not in the book".
    if let Some(colon) = href.find(':')
        && href.find('/').is_none_or(|slash| colon < slash)
    {
        return None;
    }
    let href = unquote(href);
    let mut parts: Vec<&str> = if href.starts_with('/') || base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').collect()
    };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// The href that leads from a document in `from_dir` to entry `to`,
/// percent-encoded as an href.
fn relative(from_dir: &str, to: &str) -> String {
    let from: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').collect();
    let (to_dirs, file) = to_parts.split_at(to_parts.len() - 1);
    let common = from.iter().zip(to_dirs).take_while(|(a, b)| a == b).count();
    let mut out: Vec<String> = vec!["..".to_string(); from.len() - common];
    out.extend(to_dirs[common..].iter().map(|s| s.to_string()));
    out.push(file[0].to_string());
    quote(&out.join("/"))
}

/// Entry names as compared for collisions: case-folded, as epubcheck's
/// OPF-060 compares them (a reading system on a case-insensitive file system
/// cannot keep `Images/cover.jpg` and `images/cover.jpg` apart).
fn taken_names(entries: &Entries) -> HashSet<String> {
    entries.iter().map(|(n, _)| n.to_lowercase()).collect()
}

/// `path` with its extension replaced by `ext`, made unique among `taken`
/// (from [`taken_names`]).
fn with_extension_unique(path: &str, ext: &str, taken: &HashSet<String>) -> String {
    let dir = dir_of(path);
    let file = &path[dir.len()..].trim_start_matches('/');
    let stem = file.rsplit_once('.').map_or(*file, |(s, _)| s);
    let join = |name: String| {
        if dir.is_empty() {
            name
        } else {
            format!("{dir}/{name}")
        }
    };
    let first = join(format!("{stem}.{ext}"));
    if !taken.contains(&first.to_lowercase()) {
        return first;
    }
    (2..)
        .map(|n| join(format!("{stem}-{n}.{ext}")))
        .find(|p| !taken.contains(&p.to_lowercase()))
        .expect("an unused name exists")
}

// ---------------------------------------------------------------------------
// Image size from the header
// ---------------------------------------------------------------------------

/// Width and height from an image's header, without decoding it.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if let Some(wh) = webp_size(bytes) {
        return Some(wh);
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    reader.into_dimensions().ok()
}

/// WebP's size (our build of `image` reads JPEG and PNG only, and books we
/// optimized carry WebP covers).
fn webp_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 30 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    let le24 = |i: usize| u32::from(b[i]) | u32::from(b[i + 1]) << 8 | u32::from(b[i + 2]) << 16;
    match &b[12..16] {
        b"VP8X" => Some((le24(24) + 1, le24(27) + 1)),
        b"VP8 " => {
            let w = u32::from(u16::from_le_bytes([b[26], b[27]]) & 0x3FFF);
            let h = u32::from(u16::from_le_bytes([b[28], b[29]]) & 0x3FFF);
            Some((w, h))
        }
        b"VP8L" => {
            let bits = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        _ => None,
    }
}

/// The largest cover shown as it is in a preview; above this it is shrunk.
const PREVIEW_AS_IS_BYTES: usize = 1536 * 1024;

/// An image a browser can show for `cover`: the file itself when it is small
/// and a web format, else a JPEG at most 480 px tall. `None` when neither is
/// possible (a large WebP: our build of `image` cannot decode it).
pub fn preview(cover: &CurrentCover) -> Option<(String, Vec<u8>)> {
    let web_format = matches!(
        cover.media_type.as_str(),
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    );
    if web_format && cover.bytes.len() <= PREVIEW_AS_IS_BYTES {
        return Some((cover.media_type.clone(), cover.bytes.clone()));
    }
    let img = crate::images::decode_image(&cover.bytes).ok()?;
    let small = img.thumbnail(720, 480).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode_image(&small)
        .ok()?;
    Some(("image/jpeg".to_string(), out))
}

// ---------------------------------------------------------------------------
// The package document
// ---------------------------------------------------------------------------

struct Item {
    id: String,
    href: String,
    media_type: String,
    properties: Vec<String>,
}

/// What we need from the OPF, read once.
struct Package<'a> {
    doc: roxmltree::Document<'a>,
    version3: bool,
    items: Vec<Item>,
    /// The item the book declares as its cover, by index into `items`.
    cover: Option<usize>,
    /// `rendition:layout` is `pre-paginated` for the whole book.
    fixed_layout: bool,
}

fn local(n: &roxmltree::Node) -> &'static str {
    // `tag_name().name()` borrows from the document; the names we compare
    // against are fixed, so map to those.
    match n.tag_name().name() {
        "package" => "package",
        "metadata" => "metadata",
        "manifest" => "manifest",
        "item" => "item",
        "spine" => "spine",
        "itemref" => "itemref",
        "guide" => "guide",
        "reference" => "reference",
        "meta" => "meta",
        _ => "",
    }
}

impl<'a> Package<'a> {
    fn parse(opf: &'a str) -> Result<Self> {
        let doc = parse_xml(opf).context("could not parse the package document")?;
        let root = doc.root_element();
        let version3 = root
            .attribute("version")
            .is_some_and(|v| v.trim_start().starts_with('3'));
        let items: Vec<Item> = root
            .descendants()
            .filter(|n| n.is_element() && local(n) == "item")
            .filter_map(|n| {
                Some(Item {
                    id: n.attribute("id")?.to_string(),
                    href: n.attribute("href")?.to_string(),
                    media_type: n.attribute("media-type").unwrap_or("").to_string(),
                    properties: n
                        .attribute("properties")
                        .unwrap_or("")
                        .split_whitespace()
                        .map(str::to_string)
                        .collect(),
                })
            })
            .collect();
        // EPUB 3's property first, then EPUB 2's `<meta name="cover">`.
        let by_property = items
            .iter()
            .position(|i| i.properties.iter().any(|p| p == "cover-image"));
        let by_meta = root
            .descendants()
            .find(|n| n.is_element() && local(n) == "meta" && n.attribute("name") == Some("cover"))
            .and_then(|n| n.attribute("content"))
            .and_then(|id| items.iter().position(|i| i.id == id.trim()));
        let cover = by_property
            .or(by_meta)
            .filter(|&i| items[i].media_type.starts_with("image/"));
        let fixed_layout = root.descendants().any(|n| {
            n.is_element()
                && local(&n) == "meta"
                && n.attribute("refines").is_none()
                && n.attribute("property") == Some("rendition:layout")
                && n.text().map(str::trim) == Some("pre-paginated")
        });
        Ok(Self {
            doc,
            version3,
            items,
            cover,
            fixed_layout,
        })
    }

    fn element(&self, name: &str) -> Option<roxmltree::Node<'_, 'a>> {
        self.doc
            .root_element()
            .descendants()
            .find(|n| n.is_element() && local(n) == name)
    }
}

/// The cover the book declares, read from `entries`.
pub fn current_cover(entries: &Entries, opf_name: &str) -> Result<Option<CurrentCover>> {
    let opf = entry(entries, opf_name).context("the package document is missing")?;
    let opf = std::str::from_utf8(opf).context("the package document is not UTF-8")?;
    let pkg = Package::parse(opf)?;
    let Some(i) = pkg.cover else {
        return Ok(None);
    };
    let item = &pkg.items[i];
    let Some(path) = resolve(dir_of(opf_name), &item.href) else {
        return Ok(None);
    };
    let Some(bytes) = entry(entries, &path) else {
        return Ok(None);
    };
    let size = image_size(bytes);
    Ok(Some(CurrentCover {
        path,
        media_type: item.media_type.clone(),
        width: size.map(|s| s.0),
        height: size.map(|s| s.1),
        bytes: bytes.clone(),
    }))
}

// ---------------------------------------------------------------------------
// Editing text in place
// ---------------------------------------------------------------------------

/// Byte-range replacements on one text, applied back to front.
#[derive(Default)]
struct Edits(Vec<(Range<usize>, String)>);

impl Edits {
    fn replace(&mut self, r: Range<usize>, s: impl Into<String>) {
        self.0.push((r, s.into()));
    }
    fn insert(&mut self, at: usize, s: impl Into<String>) {
        self.0.push((at..at, s.into()));
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn apply(self, text: &str) -> String {
        // Back to front, so earlier ranges stay valid. Two inserts at one spot
        // go in last-made first, which leaves them in the order they were made.
        let mut edits: Vec<_> = self.0.into_iter().enumerate().collect();
        edits.sort_by_key(|(i, (r, _))| std::cmp::Reverse((r.start, *i)));
        let mut out = text.to_string();
        for (_, (r, s)) in edits {
            out.replace_range(r, &s);
        }
        out
    }
}

/// The qualified name of an element as written (`opf:item`), and its prefix
/// with the colon (`opf:`) or `""`.
fn written_name<'t>(text: &'t str, n: &roxmltree::Node) -> (&'t str, &'t str) {
    let start = n.range().start + 1;
    let len = text[start..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(0);
    let q = &text[start..start + len];
    let prefix = q.rfind(':').map_or("", |i| &q[..=i]);
    (q, prefix)
}

/// Just past the `>` of an element's start tag (quotes respected).
fn after_start_tag(text: &str, n: &roxmltree::Node) -> Result<usize> {
    let r = n.range();
    let mut quote: Option<char> = None;
    for (i, c) in text[r.start..r.end].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => {
                if text[..r.start + i].ends_with('/') {
                    bail!("<{}> in the package document is empty", n.tag_name().name());
                }
                return Ok(r.start + i + 1);
            }
            _ => {}
        }
    }
    bail!("could not read the package document")
}

/// The start of an element's end tag.
fn before_end_tag(text: &str, n: &roxmltree::Node) -> Result<usize> {
    let r = n.range();
    let s = &text[r.start..r.end];
    if s.ends_with("/>") {
        bail!("<{}> in the package document is empty", n.tag_name().name());
    }
    s.rfind("</")
        .map(|i| r.start + i)
        .context("could not read the package document")
}

/// An attribute's raw value range, by local name.
fn attr_range(n: &roxmltree::Node, name: &str) -> Option<Range<usize>> {
    n.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.range_value())
}

/// A number written in an attribute (`600`, `600px`, `600.0`).
fn number(v: &str) -> Option<f64> {
    v.trim().trim_end_matches("px").trim().parse().ok()
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.5
}

// ---------------------------------------------------------------------------
// Content documents
// ---------------------------------------------------------------------------

/// The attributes that can point at an image.
const LINK_ATTRS: [&str; 4] = ["src", "href", "poster", "data"];

/// One document's updates: references from `old` to `new` (when the path
/// changes) and sizing that was fitted to the old image. `None` = nothing to do.
fn update_document(
    text: &str,
    doc_path: &str,
    old: &str,
    new: &str,
    old_size: Option<(u32, u32)>,
    new_size: (u32, u32),
) -> Result<Option<String>> {
    let doc = parse_xml_with_dtd(text)?;
    let dir = dir_of(doc_path);
    let mut edits = Edits::default();
    let (nw, nh) = (f64::from(new_size.0), f64::from(new_size.1));
    let mut fitted_svgs = HashSet::new();

    for n in doc.descendants().filter(|n| n.is_element()) {
        let mut points_at_cover = false;
        for a in n.attributes() {
            if !LINK_ATTRS.contains(&a.name()) {
                continue;
            }
            if resolve(dir, a.value()).as_deref() != Some(old) {
                continue;
            }
            points_at_cover = true;
            if old != new {
                let fragment = a.value().find('#').map_or("", |i| &a.value()[i..]);
                edits.replace(a.range_value(), format!("{}{fragment}", relative(dir, new)));
            }
        }
        if !points_at_cover {
            continue;
        }
        let name = n.tag_name().name();
        let w = n.attribute("width");
        let h = n.attribute("height");
        let sized_to_old = |w: Option<&str>, h: Option<&str>| match (
            w.and_then(number),
            h.and_then(number),
            old_size,
        ) {
            (Some(w), Some(h), Some((ow, oh))) => same(w, f64::from(ow)) && same(h, f64::from(oh)),
            _ => false,
        };

        if name == "image" {
            // SVG wrapper: fit the viewBox to the new image when the image fills it.
            if let Some(svg) = n
                .ancestors()
                .find(|a| a.is_element() && a.tag_name().name() == "svg")
                && let Some(vb) = svg.attribute("viewBox")
            {
                let v: Vec<f64> = vb
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| s.parse().ok())
                    .collect();
                let fills = |a: Option<&str>, full: f64| match a {
                    None => true,
                    Some(s) if s.trim() == "100%" => true,
                    Some(s) => number(s).is_some_and(|x| same(x, full)),
                };
                if v.len() == 4
                    && same(v[0], 0.0)
                    && same(v[1], 0.0)
                    && fills(w, v[2])
                    && fills(h, v[3])
                    && fitted_svgs.insert(svg.id())
                {
                    if let Some(r) = attr_range(&svg, "viewBox") {
                        edits.replace(r, format!("0 0 {} {}", new_size.0, new_size.1));
                    }
                    for (attr, val) in [("width", w), ("height", h)] {
                        if val.and_then(number).is_some()
                            && let Some(r) = attr_range(&n, attr)
                        {
                            edits.replace(r, if attr == "width" { nw } else { nh }.to_string());
                        }
                    }
                }
            }
        } else if name == "img" && sized_to_old(w, h) {
            // An <img> written at the old image's exact size.
            if let Some(r) = attr_range(&n, "width") {
                edits.replace(r, new_size.0.to_string());
            }
            if let Some(r) = attr_range(&n, "height") {
                edits.replace(r, new_size.1.to_string());
            }
        }
    }
    Ok((!edits.is_empty()).then(|| edits.apply(text)))
}

/// A stylesheet's `url(…)` references from `old` to `new`.
fn update_css(text: &str, css_path: &str, old: &str, new: &str) -> Option<String> {
    let dir = dir_of(css_path);
    let mut edits = Edits::default();
    let mut from = 0;
    while let Some(i) = text[from..].find("url(") {
        let open = from + i + 4;
        let Some(close) = text[open..].find(')').map(|c| open + c) else {
            break;
        };
        let raw = &text[open..close];
        let value = raw.trim().trim_matches(|c| c == '"' || c == '\'');
        if resolve(dir, value).as_deref() == Some(old) {
            let start = open + raw.find(value).unwrap_or(0);
            edits.replace(start..start + value.len(), relative(dir, new));
        }
        from = close;
    }
    (!edits.is_empty()).then(|| edits.apply(text))
}

fn is_markup(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "xhtml" | "html" | "htm" | "svg" | "xml")
}

fn is_css(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".css")
}

// ---------------------------------------------------------------------------
// Replacing or adding the cover
// ---------------------------------------------------------------------------

/// Put `cover` into the book as its cover image. Returns the new package
/// document (also stored in `entries`) and what changed.
pub fn apply_cover(
    entries: &mut Entries,
    opf_name: &str,
    cover: &CoverImage,
) -> Result<CoverChange> {
    let opf = String::from_utf8(
        entry(entries, opf_name)
            .context("the package document is missing")?
            .clone(),
    )
    .context("the package document is not UTF-8")?;
    let pkg = Package::parse(&opf)?;
    let opf_dir = dir_of(opf_name).to_string();
    let mut opf_edits = Edits::default();
    let mut change = CoverChange {
        width: cover.width,
        height: cover.height,
        ..CoverChange::default()
    };
    let root = pkg.doc.root_element();
    let (_, prefix) = written_name(&opf, &root);
    let prefix = prefix.to_string();
    let item_node = |id: &str| {
        root.descendants()
            .find(|n| n.is_element() && local(n) == "item" && n.attribute("id") == Some(id))
    };
    let mut ids: HashSet<String> = root
        .descendants()
        .filter_map(|n| n.attribute("id"))
        .map(str::to_string)
        .collect();
    let mut fresh_id = |base: &str| {
        let id = (1..)
            .map(|n| {
                if n == 1 {
                    base.to_string()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|id| !ids.contains(id))
            .expect("an unused id exists");
        ids.insert(id.clone());
        id
    };

    let cover_id = match pkg.cover.and_then(|i| {
        let item = &pkg.items[i];
        resolve(&opf_dir, &item.href).map(|p| (item, p))
    }) {
        Some((item, old)) => {
            // Replace the declared cover.
            let old_size = entry(entries, &old).and_then(|b| image_size(b));
            let new = if item.media_type == cover.format.media_type() {
                old.clone()
            } else {
                with_extension_unique(&old, cover.format.extension(), &taken_names(entries))
            };
            let node = item_node(&item.id).context("could not read the package document")?;
            if new != old {
                if let Some(r) = attr_range(&node, "href") {
                    opf_edits.replace(r, relative(&opf_dir, &new));
                }
                match attr_range(&node, "media-type") {
                    Some(r) => opf_edits.replace(r, cover.format.media_type()),
                    None => bail!("the cover's manifest entry has no media-type"),
                }
            }
            if new != old {
                // The package's own references to the image (a guide entry).
                for n in root.descendants().filter(|n| n.is_element() && *n != node) {
                    if let Some(a) = n.attributes().find(|a| a.name() == "href")
                        && resolve(&opf_dir, a.value()).as_deref() == Some(old.as_str())
                    {
                        opf_edits.replace(a.range_value(), relative(&opf_dir, &new));
                    }
                }
            }
            if pkg.version3 && !item.properties.iter().any(|p| p == "cover-image") {
                add_property(&opf, &node, &mut opf_edits);
            }

            // Every page that uses the old image.
            let old_base = old.rsplit('/').next().unwrap_or(&old);
            let needle = [old_base.to_string(), quote(old_base)];
            let mut kept = false;
            let docs: Vec<String> = entries
                .iter()
                .filter(|(n, _)| n.as_str() != opf_name && !n.starts_with("META-INF/"))
                .filter(|(n, _)| is_markup(n) || (is_css(n) && new != old))
                .filter(|(_, b)| {
                    let t = String::from_utf8_lossy(b);
                    needle.iter().any(|s| t.contains(s.as_str()))
                })
                .map(|(n, _)| n.clone())
                .collect();
            for name in docs {
                let text =
                    String::from_utf8_lossy(entry(entries, &name).expect("listed")).into_owned();
                let updated = if is_css(&name) {
                    Ok(update_css(&text, &name, &old, &new))
                } else {
                    update_document(
                        &text,
                        &name,
                        &old,
                        &new,
                        old_size,
                        (cover.width, cover.height),
                    )
                };
                match updated {
                    Ok(Some(t)) => {
                        set_entry(entries, &name, t.into_bytes());
                        change.pages_updated.push(name);
                    }
                    Ok(None) => {}
                    // A page we cannot parse may still use the old file: keep it.
                    Err(_) => kept = new != old,
                }
            }
            if new != old {
                if kept {
                    let id = fresh_id("epublift-previous-cover");
                    let manifest = pkg
                        .element("manifest")
                        .context("the package has no manifest")?;
                    opf_edits.insert(
                        before_end_tag(&opf, &manifest)?,
                        format!(
                            "<{prefix}item id=\"{id}\" href=\"{}\" media-type=\"{}\"/>\n",
                            relative(&opf_dir, &old),
                            item.media_type
                        ),
                    );
                    change.kept_old_image = true;
                } else {
                    entries.retain(|(n, _)| n != &old);
                }
            }
            set_entry(entries, &new, cover.bytes.clone());
            change.replaced = Some(old);
            change.path = new;
            item.id.clone()
        }
        None => add_cover(
            entries,
            opf_name,
            &opf,
            &pkg,
            &prefix,
            cover,
            &mut opf_edits,
            &mut change,
            &mut fresh_id,
        )?,
    };

    // `<meta name="cover">`: still what many reading systems look at, EPUB 3 too.
    let meta = root
        .descendants()
        .find(|n| n.is_element() && local(n) == "meta" && n.attribute("name") == Some("cover"));
    match meta {
        Some(m) if m.attribute("content").map(str::trim) == Some(cover_id.as_str()) => {}
        Some(m) => match attr_range(&m, "content") {
            Some(r) => opf_edits.replace(r, cover_id.clone()),
            None => bail!("<meta name=\"cover\"> has no content"),
        },
        None => {
            let metadata = pkg
                .element("metadata")
                .context("the package has no metadata")?;
            opf_edits.insert(
                before_end_tag(&opf, &metadata)?,
                format!("<{prefix}meta name=\"cover\" content=\"{cover_id}\"/>\n"),
            );
        }
    }

    let new_opf = opf_edits.apply(&opf);
    set_entry(entries, opf_name, new_opf.into_bytes());
    Ok(change)
}

/// Add `properties="cover-image"` (or the token) to a manifest item.
fn add_property(opf: &str, node: &roxmltree::Node, edits: &mut Edits) {
    match node.attributes().find(|a| a.name() == "properties") {
        Some(a) => {
            let r = a.range_value();
            edits.replace(r.end..r.end, " cover-image");
        }
        None => {
            let (q, _) = written_name(opf, node);
            edits.insert(
                node.range().start + 1 + q.len(),
                " properties=\"cover-image\"",
            );
        }
    }
}

/// A book with no declared cover: add the image, a cover page first in the
/// spine, and (when the book has a guide) a guide entry. Returns the image id.
#[allow(clippy::too_many_arguments)]
fn add_cover(
    entries: &mut Entries,
    opf_name: &str,
    opf: &str,
    pkg: &Package,
    prefix: &str,
    cover: &CoverImage,
    edits: &mut Edits,
    change: &mut CoverChange,
    fresh_id: &mut dyn FnMut(&str) -> String,
) -> Result<String> {
    let opf_dir = dir_of(opf_name);
    let in_dir = |name: &str| {
        if opf_dir.is_empty() {
            name.to_string()
        } else {
            format!("{opf_dir}/{name}")
        }
    };
    let taken = taken_names(entries);
    let image_path =
        with_extension_unique(&in_dir("images/cover.x"), cover.format.extension(), &taken);
    let page_path = with_extension_unique(&in_dir("cover.x"), "xhtml", &taken);
    let image_id = fresh_id("cover-image");
    let page_id = fresh_id("cover");

    let manifest = pkg
        .element("manifest")
        .context("the package has no manifest")?;
    let spine = pkg.element("spine").context("the package has no spine")?;
    let property = if pkg.version3 {
        " properties=\"cover-image\""
    } else {
        ""
    };
    edits.insert(
        before_end_tag(opf, &manifest)?,
        format!(
            "<{prefix}item id=\"{image_id}\" href=\"{}\" media-type=\"{}\"{property}/>\n\
             <{prefix}item id=\"{page_id}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            relative(opf_dir, &image_path),
            cover.format.media_type(),
            relative(opf_dir, &page_path),
        ),
    );
    edits.insert(
        after_start_tag(opf, &spine)?,
        format!("\n<{prefix}itemref idref=\"{page_id}\"/>"),
    );
    // An empty `<guide/>` is left alone: the guide is optional, the cover
    // meta and the spine already say it.
    if let Some(guide) = pkg.element("guide")
        && let Ok(at) = after_start_tag(opf, &guide)
    {
        edits.insert(
            at,
            format!(
                "\n<{prefix}reference type=\"cover\" title=\"Cover\" href=\"{}\"/>",
                relative(opf_dir, &page_path)
            ),
        );
    }

    let lang = pkg
        .doc
        .root_element()
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "language")
        .and_then(|n| n.text())
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    // A fixed-layout page states its size; the cover's own is the natural one.
    let viewport = pkg.fixed_layout.then_some((cover.width, cover.height));
    let page = cover_page(
        pkg.version3,
        lang,
        viewport,
        &relative(dir_of(&page_path), &image_path),
    );
    entries.push((image_path.clone(), cover.bytes.clone()));
    entries.push((page_path.clone(), page.into_bytes()));
    change.path = image_path;
    change.cover_page_added = Some(page_path);
    Ok(image_id)
}

/// A page that shows the cover image, full height, centred.
fn cover_page(
    version3: bool,
    lang: Option<&str>,
    viewport: Option<(u32, u32)>,
    src: &str,
) -> String {
    let viewport = viewport.map_or(String::new(), |(w, h)| {
        format!("<meta name=\"viewport\" content=\"width={w}, height={h}\"/>")
    });
    let style = "html, body { margin: 0; padding: 0; height: 100%; }\n\
                 div { height: 100%; text-align: center; }\n\
                 img { max-width: 100%; max-height: 100%; }";
    let lang_attrs = lang.map_or(String::new(), |l| {
        if version3 {
            format!(" xml:lang=\"{l}\" lang=\"{l}\"")
        } else {
            format!(" xml:lang=\"{l}\"")
        }
    });
    if version3 {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n\
             <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\"{lang_attrs}>\n\
             <head><title>Cover</title>{viewport}<style>\n{style}\n</style></head>\n\
             <body epub:type=\"cover\"><div><img src=\"{src}\" alt=\"Cover\"/></div></body>\n</html>\n"
        )
    } else {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n\
             <html xmlns=\"http://www.w3.org/1999/xhtml\"{lang_attrs}>\n\
             <head><title>Cover</title><style type=\"text/css\">\n{style}\n</style></head>\n\
             <body><div><img src=\"{src}\" alt=\"Cover\"/></div></body>\n</html>\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .encode_image(&img)
            .unwrap();
        out
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([30, 30, 200]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn text(entries: &Entries, name: &str) -> String {
        String::from_utf8(entry(entries, name).unwrap().clone()).unwrap()
    }

    const OPF2: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:language>tr</dc:language><meta name="cover" content="cimg"/></metadata>
<manifest>
<item id="cimg" href="Images/cover.jpeg" media-type="image/jpeg"/>
<item id="back" href="Images/backcover.jpeg" media-type="image/jpeg"/>
<item id="tp" href="Text/titlepage.xhtml" media-type="application/xhtml+xml"/>
<item id="c1" href="Text/c1.xhtml" media-type="application/xhtml+xml"/>
<item id="css" href="Styles/s.css" media-type="text/css"/>
</manifest>
<spine><itemref idref="tp"/><itemref idref="c1"/></spine>
</package>"#;

    const TITLEPAGE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>c</title></head><body>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" version="1.1" width="100%" height="100%" viewBox="0 0 600 800" preserveAspectRatio="none"><image width="600" height="800" xlink:href="../Images/cover.jpeg"/></svg>
</body></html>"#;

    fn book() -> Entries {
        vec![
            ("mimetype".into(), b"application/epub+zip".to_vec()),
            ("OEBPS/content.opf".into(), OPF2.as_bytes().to_vec()),
            ("OEBPS/Images/cover.jpeg".into(), jpeg(600, 800)),
            ("OEBPS/Images/backcover.jpeg".into(), jpeg(10, 10)),
            ("OEBPS/Text/titlepage.xhtml".into(), TITLEPAGE.as_bytes().to_vec()),
            (
                "OEBPS/Text/c1.xhtml".into(),
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>1</title></head><body><img src="../Images/backcover.jpeg" alt=""/><img src="../Images/cover.jpeg" width="600" height="800" alt=""/></body></html>"#.to_vec(),
            ),
            (
                "OEBPS/Styles/s.css".into(),
                b"div.c { background: url(\"../Images/cover.jpeg\"); } div.b { background: url(../Images/backcover.jpeg); }".to_vec(),
            ),
        ]
    }

    #[test]
    fn resolves_and_relates_paths() {
        assert_eq!(
            resolve("OEBPS/Text", "../Images/a%20b.jpg#x").as_deref(),
            Some("OEBPS/Images/a b.jpg")
        );
        assert_eq!(resolve("OEBPS", "http://x/y.jpg"), None);
        assert_eq!(resolve("", "../a.jpg"), None);
        assert_eq!(
            relative("OEBPS/Text", "OEBPS/Images/a b.png"),
            "../Images/a%20b.png"
        );
        assert_eq!(relative("", "OEBPS/x.png"), "OEBPS/x.png");
        assert_eq!(relative("OEBPS", "OEBPS/x.png"), "x.png");
    }

    #[test]
    fn rejects_what_is_not_a_jpeg_or_png() {
        assert!(CoverImage::from_bytes(b"GIF89a....".to_vec()).is_err());
        let mut broken = jpeg(50, 50);
        broken.truncate(40);
        assert!(CoverImage::from_bytes(broken).is_err());
        let ok = CoverImage::from_bytes(png(40, 60)).unwrap();
        assert_eq!((ok.format, ok.width, ok.height), (CoverFormat::Png, 40, 60));
    }

    #[test]
    fn same_format_keeps_the_path_and_refits_the_svg() {
        let mut b = book();
        let new = CoverImage::from_bytes(jpeg(1200, 1600 + 200)).unwrap();
        let change = apply_cover(&mut b, "OEBPS/content.opf", &new).unwrap();
        assert_eq!(change.path, "OEBPS/Images/cover.jpeg");
        assert_eq!(entry(&b, "OEBPS/Images/cover.jpeg").unwrap(), &new.bytes);
        let tp = text(&b, "OEBPS/Text/titlepage.xhtml");
        assert!(tp.contains(r#"viewBox="0 0 1200 1800""#), "{tp}");
        assert!(
            tp.contains(r#"<image width="1200" height="1800" xlink:href="../Images/cover.jpeg"/>"#),
            "{tp}"
        );
        // The <img> written at the old size follows; the back cover is untouched.
        let c1 = text(&b, "OEBPS/Text/c1.xhtml");
        assert!(
            c1.contains(r#"src="../Images/cover.jpeg" width="1200" height="1800""#),
            "{c1}"
        );
        assert!(c1.contains("backcover.jpeg"));
        assert_eq!(text(&b, "OEBPS/content.opf"), OPF2);
    }

    #[test]
    fn new_format_renames_and_follows_every_reference() {
        let mut b = book();
        let new = CoverImage::from_bytes(png(300, 450)).unwrap();
        let change = apply_cover(&mut b, "OEBPS/content.opf", &new).unwrap();
        assert_eq!(change.path, "OEBPS/Images/cover.png");
        assert!(entry(&b, "OEBPS/Images/cover.jpeg").is_none());
        assert!(entry(&b, "OEBPS/Images/backcover.jpeg").is_some());
        let opf = text(&b, "OEBPS/content.opf");
        assert!(
            opf.contains(r#"<item id="cimg" href="Images/cover.png" media-type="image/png"/>"#),
            "{opf}"
        );
        assert!(opf.contains(r#"href="Images/backcover.jpeg""#));
        let tp = text(&b, "OEBPS/Text/titlepage.xhtml");
        assert!(
            tp.contains(r#"xlink:href="../Images/cover.png""#) && tp.contains("0 0 300 450"),
            "{tp}"
        );
        let c1 = text(&b, "OEBPS/Text/c1.xhtml");
        assert!(
            c1.contains(r#"src="../Images/backcover.jpeg""#)
                && c1.contains(r#"src="../Images/cover.png""#),
            "{c1}"
        );
        let css = text(&b, "OEBPS/Styles/s.css");
        assert!(
            css.contains("url(\"../Images/cover.png\")")
                && css.contains("url(../Images/backcover.jpeg)"),
            "{css}"
        );
    }

    #[test]
    fn a_book_without_a_cover_gets_one_first_in_the_spine() {
        let opf = OPF2
            .replace(r#"<meta name="cover" content="cimg"/>"#, "")
            .replace(
                "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"2.0\"",
                "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\"",
            );
        let mut b = book();
        set_entry(&mut b, "OEBPS/content.opf", opf.into_bytes());
        // An EPUB 3 book whose image is not declared as the cover.
        let new = CoverImage::from_bytes(jpeg(500, 750)).unwrap();
        let change = apply_cover(&mut b, "OEBPS/content.opf", &new).unwrap();
        assert_eq!(change.path, "OEBPS/images/cover.jpg");
        assert_eq!(
            change.cover_page_added.as_deref(),
            Some("OEBPS/cover.xhtml")
        );
        let opf = text(&b, "OEBPS/content.opf");
        assert!(opf.contains(r#"<item id="cover-image" href="images/cover.jpg" media-type="image/jpeg" properties="cover-image"/>"#), "{opf}");
        assert!(
            opf.contains("<spine>\n<itemref idref=\"cover\"/><itemref idref=\"tp\"/>"),
            "{opf}"
        );
        assert!(opf.contains(r#"<meta name="cover" content="cover-image"/>"#));
        let page = text(&b, "OEBPS/cover.xhtml");
        assert!(
            page.contains(r#"<img src="images/cover.jpg" alt="Cover"/>"#)
                && page.contains("xml:lang=\"tr\""),
            "{page}"
        );
        // The page that was first is untouched.
        assert_eq!(text(&b, "OEBPS/Text/titlepage.xhtml"), TITLEPAGE);
    }

    #[test]
    fn a_prefixed_package_gets_prefixed_elements() {
        let opf = r#"<?xml version="1.0"?>
<opf:package xmlns:opf="http://www.idpf.org/2007/opf" version="2.0"><opf:metadata><dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">T</dc:title></opf:metadata>
<opf:manifest><opf:item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></opf:manifest><opf:spine><opf:itemref idref="c1"/></opf:spine></opf:package>"#;
        let mut b: Entries = vec![
            ("content.opf".into(), opf.as_bytes().to_vec()),
            (
                "c1.xhtml".into(),
                b"<html xmlns=\"http://www.w3.org/1999/xhtml\"/>".to_vec(),
            ),
        ];
        let new = CoverImage::from_bytes(png(20, 30)).unwrap();
        apply_cover(&mut b, "content.opf", &new).unwrap();
        let opf = text(&b, "content.opf");
        assert!(opf.contains("<opf:itemref idref=\"cover\"/>"), "{opf}");
        assert!(
            opf.contains("<opf:meta name=\"cover\" content=\"cover-image\"/>"),
            "{opf}"
        );
        assert!(roxmltree::Document::parse(&opf).is_ok());
    }

    #[test]
    fn a_new_name_never_collides_after_case_folding() {
        let opf = OPF2.replace(r#"<meta name="cover" content="cimg"/>"#, "");
        let mut b = book();
        set_entry(&mut b, "OEBPS/content.opf", opf.into_bytes());
        b.push(("OEBPS/Images/cover.jpg".into(), jpeg(5, 5)));
        let change = apply_cover(
            &mut b,
            "OEBPS/content.opf",
            &CoverImage::from_bytes(jpeg(50, 75)).unwrap(),
        )
        .unwrap();
        assert_eq!(change.path, "OEBPS/images/cover-2.jpg");
    }

    #[test]
    fn a_guide_entry_pointing_at_the_image_follows_it() {
        let opf = OPF2.replace(
            "</spine>",
            "</spine>\n<guide><reference type=\"cover\" title=\"Cover\" href=\"Images/cover.jpeg\"/></guide>",
        );
        let mut b = book();
        set_entry(&mut b, "OEBPS/content.opf", opf.into_bytes());
        apply_cover(
            &mut b,
            "OEBPS/content.opf",
            &CoverImage::from_bytes(png(30, 45)).unwrap(),
        )
        .unwrap();
        let opf = text(&b, "OEBPS/content.opf");
        assert!(
            opf.contains(r#"<reference type="cover" title="Cover" href="Images/cover.png"/>"#),
            "{opf}"
        );
    }

    #[test]
    fn a_fixed_layout_cover_page_states_its_viewport() {
        let opf = OPF2
            .replace(
                r#"<meta name="cover" content="cimg"/>"#,
                r#"<meta property="rendition:layout">pre-paginated</meta>"#,
            )
            .replace("version=\"2.0\"", "version=\"3.0\"");
        let mut b = book();
        set_entry(&mut b, "OEBPS/content.opf", opf.into_bytes());
        let change = apply_cover(
            &mut b,
            "OEBPS/content.opf",
            &CoverImage::from_bytes(jpeg(800, 1200)).unwrap(),
        )
        .unwrap();
        let page = text(&b, change.cover_page_added.as_deref().unwrap());
        assert!(
            page.contains(r#"<meta name="viewport" content="width=800, height=1200"/>"#),
            "{page}"
        );
    }

    #[test]
    fn a_page_with_a_doctype_is_updated_too() {
        let mut b = book();
        let tp = TITLEPAGE.replace(
            "<html",
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n<html",
        );
        set_entry(&mut b, "OEBPS/Text/titlepage.xhtml", tp.into_bytes());
        let change = apply_cover(
            &mut b,
            "OEBPS/content.opf",
            &CoverImage::from_bytes(png(30, 45)).unwrap(),
        )
        .unwrap();
        assert!(!change.kept_old_image);
        assert!(text(&b, "OEBPS/Text/titlepage.xhtml").contains("../Images/cover.png"));
    }

    #[test]
    fn reads_the_declared_cover_and_its_size() {
        let b = book();
        let c = current_cover(&b, "OEBPS/content.opf").unwrap().unwrap();
        assert_eq!(
            (c.path.as_str(), c.width, c.height),
            ("OEBPS/Images/cover.jpeg", Some(600), Some(800))
        );
    }
}
