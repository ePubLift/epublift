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

//! Core library for ePubLift: modernize EPUB structure and re-encode images.
//!
//! The CLI (`src/main.rs`) is a thin front-end over [`convert`]. Library callers
//! build [`Options`], call [`convert`], and inspect the returned [`Report`].

pub mod cover;
#[cfg(feature = "metadata")]
pub mod enrich;
#[cfg(feature = "archival")]
pub mod eparc;
// Shared minimal EPUB writer used by the PDF and Markdown importers.
#[cfg(any(feature = "pdf", feature = "markdown"))]
mod epub_writer;
#[cfg(any(feature = "metadata", feature = "pdf-ocr", feature = "smart-import"))]
pub mod http;
mod images;
mod kepub;
#[cfg(feature = "markdown")]
pub mod markdown;
pub mod meta;
mod nav;
mod opf;
#[cfg(any(feature = "pdf", feature = "smart-import"))]
mod page_numbers;
#[cfg(feature = "pdf")]
pub mod pdf;
mod presentational;
pub mod unique_id;
mod upgrade_fixes;
// AI "Smart Import": cloud OCR (PDF → Markdown) feeding the offline Markdown core.
mod report;
#[cfg(feature = "smart-import")]
pub mod smart_import;
mod util;
#[cfg(feature = "zstd-experimental")]
pub mod zstd_ocf;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use opf::RewriteParams;

pub use images::{FormatPolicy, ImageFormat, ImageMetric};

/// This library's version: the engine inside both the `epublift` CLI and
/// `epublift-web`, so a web build can say which CLI release it matches.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The epubveri release linked in (the validator, and the XML guard every
/// parse goes through).
pub const EPUBVERI_VERSION: &str = epubveri::VERSION;

/// Target EPUB specification version for the converted output.
///
/// Only EPUB 3.3 is supported today; the enum exists so newer versions (e.g.
/// EPUB 3.4) can be added without changing the [`Options`]/[`convert`] surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EpubVersion {
    /// EPUB 3.3 — the current target: WebP images and a hybrid nav document.
    #[default]
    V3_3,
    /// EPUB 3.4 — experimental: AVIF / JPEG XL images become core media types.
    /// Requires the `epub34` build feature to emit AVIF/JXL. See docs/epub-3.4.md.
    V3_4,
}

impl EpubVersion {
    /// The newest *stable* target version; the default for new conversions.
    /// 3.4 is experimental and opted into explicitly, so LATEST stays 3.3.
    pub const LATEST: EpubVersion = EpubVersion::V3_3;

    /// Filename tag for version-stamped output, e.g. `"3.3"` → `name_v3.3.epub`.
    pub fn tag(self) -> &'static str {
        match self {
            EpubVersion::V3_3 => "3.3",
            EpubVersion::V3_4 => "3.4",
        }
    }
}

/// How raster images are handled during conversion.
///
/// `KeepOriginal` is the default: JPEG/PNG images are left untouched, for
/// every reading system — notably **Kobo e-ink readers do not render WebP**
/// despite claiming EPUB 3.3 support, and publishers and readers alike
/// distrust a format their devices may not show. `WebP` re-encodes them for
/// the smallest files on readers that support it (Apple Books, calibre, most
/// apps); despite its name it means "convert", and [`Options::image_policy`]
/// / the target version pick the format (AVIF/JXL under EPUB 3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageStrategy {
    /// Convert JPEG/PNG (size-safe: never grows, never upscales). WebP for
    /// EPUB 3.3; the format follows [`Options::image_policy`] otherwise.
    WebP,
    /// Leave images in their original format; only upgrade structure.
    #[default]
    KeepOriginal,
}

/// How the output container itself is compressed.
///
/// `Deflate` is the only **conformant** packaging and the shipping default —
/// it matches what every reading system implements (OCF restricts the ZIP
/// container to Stored + Deflate). `Zstd` is the **experimental research mode**
/// (see `docs/design/zstd-ocf-experimental.md`): it writes non-mimetype entries
/// with ZIP compression method 93 (Zstandard) to *measure* what Zstd would save
/// over Deflate for EPUB packaging. A Zstd-packaged file is **not a conformant
/// EPUB and will not open in current readers** — it exists purely to produce
/// numbers for a future W3C `epub-specs` discussion. The actual encoding is only
/// available when the crate is built with the `zstd-experimental` feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Packaging {
    /// Conformant Deflate packaging (the default).
    #[default]
    Deflate,
    /// Experimental, non-conformant Zstandard packaging at the given C-zstd
    /// level (1–22). `mode` selects how entries share context.
    Zstd { mode: ZstdMode, level: i32 },
}

/// How Zstandard packaging shares compression context across archive entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZstdMode {
    /// Each entry is compressed independently — the standards-plausible,
    /// conservative floor.
    #[default]
    PerEntry,
    /// One dictionary, trained from the book's own text entries and stored as
    /// `META-INF/zstd-dict.bin`, shared across text entries — the cross-chapter
    /// "big win" (explicitly non-standard: ZIP has no slot for a shared
    /// dictionary; storing it as a named entry is our concrete proposal).
    ///
    /// This is **size-safe**: the dictionary is kept only when the resulting
    /// archive actually beats per-entry (it wins on large multi-chapter text
    /// books, loses to its own stored bytes on small/single-file/image-heavy
    /// ones), mirroring the project's "never grow a book" image principle. So
    /// the output is never larger than [`ZstdMode::PerEntry`].
    SharedDict,
}

/// Options controlling a conversion.
///
/// Construct via [`Options::default`] and override fields as needed.
#[derive(Debug, Clone)]
pub struct Options {
    /// WebP encoding quality, 1–100 (clamped into range during conversion).
    pub quality: u8,
    /// Transliterate the auto-generated output name to ASCII.
    pub ascii: bool,
    /// Target EPUB version for the output.
    pub target_version: EpubVersion,
    /// How raster images are handled. Note: [`Options::kepub`] keeps originals
    /// by default (Kobo can't render WebP) unless [`Options::kepub_webp`] opts
    /// back into WebP — see that field.
    pub image_strategy: ImageStrategy,
    /// Produce a Kobo `.kepub.epub`: inject `koboSpan` markup into the content
    /// documents and name the output `<stem>.kepub.epub`. The result is still a
    /// valid EPUB 3 on top of the normal upgrades.
    pub kepub: bool,
    /// For `.kepub` output, emit WebP images instead of keeping originals.
    /// Default `false`: stock Kobo firmware shows WebP as blank, so `kepub`
    /// keeps the original JPEG/PNG. Set `true` only if the target Kobo has the
    /// WebP image plugin installed (see `kobo-webp-plugin/`). No effect unless
    /// [`Options::kepub`] is set.
    pub kepub_webp: bool,
    /// How the output raster image format is chosen. `None` uses the target
    /// version's default ([`FormatPolicy::Fixed`]`(WebP)` for 3.3, the
    /// content-adaptive [`FormatPolicy::Auto`] for 3.4). `Some(..)` overrides:
    /// `Fixed(f)` forces one format, `Best` keeps the smallest per image. AVIF /
    /// JXL (incl. `Auto`/`Best` emitting them) need the `epub34` feature.
    pub image_policy: Option<FormatPolicy>,
    /// AVIF encoder speed, 1 (slowest/best) to 10 (fastest). The default (`4`)
    /// favors size; an interactive caller (e.g. the web service) can raise it to
    /// stay responsive. Only affects AVIF output (the `epub34` feature).
    pub avif_speed: u8,
    /// Container packaging. Defaults to conformant [`Packaging::Deflate`];
    /// [`Packaging::Zstd`] is the experimental measurement mode and requires the
    /// `zstd-experimental` build feature.
    pub packaging: Packaging,
    /// Explicit output path; when `None`, [`default_output_path`] is used.
    pub output: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            quality: 80,
            ascii: false,
            target_version: EpubVersion::LATEST,
            image_strategy: ImageStrategy::default(),
            image_policy: None,
            avif_speed: 4,
            kepub: false,
            kepub_webp: false,
            packaging: Packaging::default(),
            output: None,
        }
    }
}

/// Structured outcome of a successful [`convert`] call.
pub struct Report {
    /// File name of the original input EPUB.
    pub input_name: String,
    /// File name of the written output EPUB.
    pub output_name: String,
    /// Full path the output EPUB was written to.
    pub output_path: PathBuf,
    /// Size of the input EPUB, in bytes.
    pub original_size: u64,
    /// Size of the output EPUB, in bytes.
    pub final_size: u64,
    /// Per-image conversion metrics.
    pub image_metrics: Vec<ImageMetric>,
    /// The EPUB version that was targeted.
    pub target_version: EpubVersion,
    /// EPUB 3.4 "outdated"/deprecated features found in the source (informational;
    /// the content is preserved, not stripped). Empty when none were detected.
    pub outdated_features: Vec<String>,
    /// Presentational attributes EPUB 3 no longer allows, written as CSS.
    pub presentational: presentational::Summary,
}

impl Report {
    /// Bytes saved (negative if the output grew).
    pub fn bytes_saved(&self) -> i64 {
        self.original_size as i64 - self.final_size as i64
    }

    /// Percentage of the original size that was saved.
    pub fn percent_saved(&self) -> f64 {
        if self.original_size > 0 {
            self.bytes_saved() as f64 / self.original_size as f64 * 100.0
        } else {
            0.0
        }
    }

    /// Write the human-readable text audit report to `path`.
    pub fn write_text_report(&self, path: &Path) -> Result<()> {
        report::write_report(path, self)
    }
}

/// The output file stem for `input`, optionally transliterated to ASCII.
pub fn output_stem(input: &Path, ascii: bool) -> String {
    let raw = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    if ascii {
        util::slugify_ascii(&raw)
    } else {
        raw
    }
}

/// The default output path next to `input`: version-stamped (e.g. `book_v3.3.epub`),
/// a Kobo `book_v3.3.kepub.epub` when [`Options::kepub`] is set (the version stamp
/// is kept so it's easy to tell from the original, and `.kepub.epub` is what Kobo
/// keys on), or a `book_zstd-experimental.epub` when [`Packaging::Zstd`] is selected.
///
/// The `_zstd-experimental` suffix is deliberately *not* `_v3.x`: a Zstd archive
/// is non-conformant, so a version-looking name would misrepresent it. The
/// suffix is tied to the conformance axis and stays until the output actually
/// becomes conformant — not when our measurements mature.
pub fn default_output_path(input: &Path, options: &Options) -> PathBuf {
    let stem = output_stem(input, options.ascii);
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let base = if options.kepub {
        format!("{}_v{}.kepub", stem, options.target_version.tag())
    } else if matches!(options.packaging, Packaging::Zstd { .. }) {
        stem
    } else {
        format!("{}_v{}", stem, options.target_version.tag())
    };
    if matches!(options.packaging, Packaging::Zstd { .. }) {
        parent.join(format!("{}_zstd-experimental.epub", base))
    } else {
        parent.join(format!("{}.epub", base))
    }
}

/// The OPF package document's entry name inside an open EPUB archive (from
/// `META-INF/container.xml`).
fn opf_entry_name<R: io::Read + io::Seek>(zip: &mut ZipArchive<R>) -> Result<String> {
    let container = read_named(zip, "META-INF/container.xml")?
        .context("Invalid EPUB: META-INF/container.xml is missing")?;
    let container = String::from_utf8(container).context("container.xml is not UTF-8")?;
    let doc = crate::util::parse_xml(&container).context("Failed to parse container.xml")?;
    doc.descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "rootfile"
                && n.attribute("full-path").is_some()
        })
        .and_then(|n| n.attribute("full-path"))
        .map(str::to_string)
        .context("Could not find rootfile element in container.xml")
}

/// Read the OPF package document's text from an EPUB without extracting the
/// archive (used by the read-only `meta show` path). The input is never modified.
pub fn read_opf_text(epub: &Path) -> Result<String> {
    let file = File::open(epub).with_context(|| format!("EPUB not found: {}", epub.display()))?;
    let mut zip = ZipArchive::new(file).context("Input is not a valid EPUB (zip) archive")?;
    let name = opf_entry_name(&mut zip)?;
    let opf = read_named(&mut zip, &name)?
        .with_context(|| format!("OPF package document not found in EPUB: {name}"))?;
    String::from_utf8(opf).context("OPF package document is not UTF-8")
}

/// Read the book's bibliographic metadata from `epub`. The input is never
/// modified.
pub fn read_metadata(epub: &Path) -> Result<meta::Metadata> {
    let xml = read_opf_text(epub)?;
    meta::parse_metadata(&xml)
}

/// Read the cover image `epub` declares, if any. The input is never modified.
pub fn read_cover(epub: &Path) -> Result<Option<cover::CurrentCover>> {
    let file = File::open(epub).with_context(|| format!("EPUB not found: {}", epub.display()))?;
    let mut zip = ZipArchive::new(file).context("Input is not a valid EPUB (zip) archive")?;
    let opf_name = opf_entry_name(&mut zip)?;
    let entries = read_entries(&mut zip)?;
    cover::current_cover(&entries, &opf_name)
}

/// The result of [`write_metadata_and_cover`].
#[derive(Debug)]
pub struct MetadataWrite {
    pub metadata: meta::Metadata,
    /// What changed about the cover, when a new one was given.
    pub cover: Option<cover::CoverChange>,
}

/// Apply a metadata `update` to `input`, writing the result to `output` (the
/// input is never modified). Only the OPF entry is rewritten; every other entry
/// is copied through. Returns the resulting metadata for display.
pub fn write_metadata(
    input: &Path,
    update: &meta::MetadataUpdate,
    output: &Path,
) -> Result<meta::Metadata> {
    Ok(write_metadata_and_cover(input, update, None, output)?.metadata)
}

/// [`write_metadata`], and when `new_cover` is given, make it the book's cover
/// (see [`cover::apply_cover`]). A cover change is checked: if the result has
/// more fatal errors and errors than the input, nothing is written.
pub fn write_metadata_and_cover(
    input: &Path,
    update: &meta::MetadataUpdate,
    new_cover: Option<&cover::CoverImage>,
    output: &Path,
) -> Result<MetadataWrite> {
    let original =
        fs::read(input).with_context(|| format!("EPUB not found: {}", input.display()))?;
    let mut zip = ZipArchive::new(io::Cursor::new(&original))
        .context("Input is not a valid EPUB (zip) archive")?;
    let opf_name = opf_entry_name(&mut zip)?;
    let mut entries = read_entries(&mut zip)?;
    drop(zip);

    let cover_change = match new_cover {
        Some(c) => Some(cover::apply_cover(&mut entries, &opf_name, c)?),
        None => None,
    };

    let opf_xml = entries
        .iter()
        .find(|(n, _)| *n == opf_name)
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .with_context(|| format!("OPF package document not found in EPUB: {opf_name}"))?;
    // A cover-only change leaves the metadata alone, except that EPUB 3's
    // `dcterms:modified` is refreshed; EPUB 2 has no such field.
    let refresh_only = update.is_empty() && cover_change.is_some();
    let epub3 = meta::parse_metadata(&opf_xml)?
        .epub_version
        .is_some_and(|v| v.trim_start().starts_with('3'));
    let new_opf = if refresh_only && !epub3 {
        opf_xml
    } else {
        let modified_ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        meta::apply_update(&opf_xml, update, &modified_ts)?
    };
    let new_opf = match &update.unique_identifier {
        Some(choice) => set_unique_identifier(&mut entries, &opf_name, &new_opf, choice, update)?,
        None => new_opf,
    };
    let metadata = meta::parse_metadata(&new_opf)?;
    if let Some(e) = entries.iter_mut().find(|(n, _)| *n == opf_name) {
        e.1 = new_opf.into_bytes();
    }

    let bytes = zip_entries(&entries)?;
    if cover_change.is_some() || update.unique_identifier.is_some() {
        // Never make a book worse: a new cover or identifier goes in only if the
        // book has no more fatal errors and errors than before.
        let count = |b: Vec<u8>| {
            let r = epubveri::validate_bytes(b);
            r.fatals() + r.errors()
        };
        let (before, after) = (count(original), count(bytes.clone()));
        if after > before {
            bail!(
                "this change would leave the book with {after} error(s) where it had \
                 {before}, so nothing was written"
            );
        }
    }
    fs::write(output, &bytes).with_context(|| format!("Failed to create {}", output.display()))?;
    Ok(MetadataWrite {
        metadata,
        cover: cover_change,
    })
}

/// The OPF with `choice` as its unique identifier; the NCX's `dtb:uid` follows.
/// Refused when the identifier would change on a book whose font obfuscation is
/// keyed on it: its fonts would stop working.
fn set_unique_identifier(
    entries: &mut cover::Entries,
    opf_name: &str,
    opf: &str,
    choice: &unique_id::UniqueIdentifier,
    update: &meta::MetadataUpdate,
) -> Result<String> {
    let md = meta::parse_metadata(opf)?;
    let value = unique_id::resolve(choice, &md, update.isbn.as_deref())?;
    let current = md.identifiers.iter().find(|i| i.unique);
    if current.is_none_or(|i| i.value.trim() != value) && entries_obfuscate_fonts(entries) {
        bail!(
            "META-INF/encryption.xml obfuscates this book's fonts with a key derived from its \
             unique identifier, so changing the identifier would break them; nothing was written"
        );
    }
    let new_opf = unique_id::apply_to_opf(opf, &value)?;
    if let Some(name) = unique_id::ncx_entry_name(&new_opf, opf_name)
        && let Some(entry) = entries.iter_mut().find(|(n, _)| *n == name)
        && let Some(ncx) = unique_id::sync_ncx(&String::from_utf8_lossy(&entry.1), &value)
    {
        entry.1 = ncx.into_bytes();
    }
    Ok(new_opf)
}

fn entries_obfuscate_fonts(entries: &cover::Entries) -> bool {
    entries
        .iter()
        .find(|(n, _)| n == "META-INF/encryption.xml")
        .is_some_and(|(_, b)| unique_id::obfuscates(&String::from_utf8_lossy(b)))
}

/// Whether the book's font obfuscation is keyed on its unique identifier, so the
/// identifier must stay.
pub fn fonts_obfuscated(epub: &Path) -> Result<bool> {
    let file =
        fs::File::open(epub).with_context(|| format!("EPUB not found: {}", epub.display()))?;
    let mut zip = ZipArchive::new(file).context("Input is not a valid EPUB (zip) archive")?;
    Ok(match zip.by_name("META-INF/encryption.xml") {
        Ok(entry) => {
            let mut text = Vec::new();
            entry.take(16 * 1024 * 1024).read_to_end(&mut text)?;
            unique_id::obfuscates(&String::from_utf8_lossy(&text))
        }
        Err(_) => false,
    })
}

/// Every file entry of an EPUB, in archive order, held in memory — with the
/// same entry-count and decompressed-size limits as [`extract_epub`].
fn read_entries<R: io::Read + io::Seek>(zip: &mut ZipArchive<R>) -> Result<cover::Entries> {
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        bail!(
            "EPUB has too many entries ({}); refusing to read it.",
            zip.len()
        );
    }
    let mut budget: u64 = MAX_TOTAL_UNCOMPRESSED;
    let mut entries = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let name = {
            let entry = zip.by_index_raw(i)?;
            if entry.is_dir() {
                continue;
            }
            entry.name()?.to_string()
        };
        let mut data = Vec::new();
        let read = entry_reader(zip, i)?
            .take(budget + 1)
            .read_to_end(&mut data)? as u64;
        if read > budget {
            bail!("EPUB is too large when decompressed (possible zip bomb).");
        }
        budget -= read;
        entries.push((name, data));
    }
    Ok(entries)
}

/// Zip `entries` as an EPUB: `mimetype` first and stored, the rest deflated.
fn zip_entries(entries: &cover::Entries) -> Result<Vec<u8>> {
    let mut writer = ZipWriter::new(io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mimetype = entries
        .iter()
        .find(|(n, _)| n == "mimetype")
        .map_or(&b"application/epub+zip"[..], |(_, b)| b.as_slice());
    writer.start_file("mimetype", stored)?;
    io::Write::write_all(&mut writer, mimetype)?;
    for (name, data) in entries.iter().filter(|(n, _)| n != "mimetype") {
        writer.start_file(name.as_str(), deflated)?;
        io::Write::write_all(&mut writer, data)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Modernize `input` and write an optimized EPUB.
///
/// Progress messages are delivered to `progress`; pass `|_| {}` to ignore them.
/// On success the optimized EPUB has been written to disk and a [`Report`]
/// describing the result is returned. The input file is never modified.
pub fn convert(input: &Path, options: &Options, progress: impl Fn(&str)) -> Result<Report> {
    let input_path = input
        .canonicalize()
        .with_context(|| format!("Input file not found: {}", input.display()))?;

    let output_path = options
        .output
        .clone()
        .unwrap_or_else(|| default_output_path(&input_path, options));

    if options.kepub && matches!(options.packaging, Packaging::Zstd { .. }) {
        bail!(
            "--kepub cannot be combined with Zstandard packaging: Kobo reads only a conformant EPUB."
        );
    }

    let quality = options.quality.clamp(1, 100);
    let original_size = fs::metadata(&input_path)?.len();

    let input_name = input_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let temp_dir = tempfile::Builder::new().prefix("epublift_").tempdir()?;
    let temp_path = temp_dir.path();

    // Step 1: Extract EPUB.
    let version = read_metadata(&input_path).ok().and_then(|m| m.epub_version);
    let repaired =
        upgrade_fixes::repair_legacy_version(&input_path, version.as_deref(), &progress)?;
    let source = repaired.as_ref().map_or(input_path.as_path(), |f| f.path());
    progress("[*] Extracting original EPUB file...");
    let mut entry_names = extract_epub(source, temp_path)?;

    // Step 2: Locate the OPF package document.
    let opf_path = locate_opf(temp_path)?;
    progress(&format!(
        "[+] Located package document (OPF): {}",
        opf_path
            .strip_prefix(temp_path)
            .unwrap_or(&opf_path)
            .display()
    ));
    let package_dir = opf_path.parent().unwrap_or(temp_path).to_path_buf();

    let opf_xml = fs::read_to_string(&opf_path).context("Failed to read OPF package document")?;
    let info = opf::parse_opf_info(&opf_xml)?;

    // Step 3: Optimize images — unless we're keeping originals. Kobo `.kepub`
    // keeps originals by default because stock Kobo e-ink can't render WebP;
    // `kepub_webp` opts back into WebP for devices with the WebP plugin
    // (see kobo-webp-plugin/).
    let image_strategy = match (options.kepub, options.kepub_webp) {
        (true, true) => ImageStrategy::WebP,
        (true, false) => ImageStrategy::KeepOriginal,
        (false, _) => options.image_strategy,
    };
    let opt = match image_strategy {
        ImageStrategy::WebP => {
            // Format selection: an explicit policy wins; otherwise the version
            // default — 3.3 → WebP, 3.4 → content-adaptive (per image, from the
            // source type: JPEG → AVIF, PNG → WebP).
            let policy = options
                .image_policy
                .unwrap_or(match options.target_version {
                    EpubVersion::V3_3 => FormatPolicy::Fixed(ImageFormat::WebP),
                    EpubVersion::V3_4 => FormatPolicy::Auto,
                });
            progress(&format!(
                "[*] Converting and compressing images ({})...",
                policy.label()
            ));
            let opt = images::optimize_images(
                &package_dir,
                &info.items,
                info.cover_id.as_deref(),
                quality,
                policy,
                options.avif_speed,
                &progress,
            )?;
            images::update_document_references(temp_path, &opt.ref_pairs, &progress);
            opt
        }
        ImageStrategy::KeepOriginal => {
            progress("[*] Keeping original images (no WebP conversion)...");
            images::OptimizeResult {
                metrics: Vec::new(),
                manifest_changes: HashMap::new(),
                ref_pairs: Vec::new(),
            }
        }
    };

    // Step 4: Upgrade structure to EPUB 3.3.
    progress("[*] Upgrading structure to EPUB 3.3 compliance...");

    // Decide whether we must generate a navigation document from toc.ncx.
    let mut add_nav = false;
    if !info.nav_exists
        && let Some(ncx_href) = &info.ncx_href
    {
        let ncx_path = package_dir.join(util::unquote(ncx_href));
        if ncx_path.exists() {
            progress("[+] Creating mandatory EPUB 3 Navigation Document from toc.ncx...");
            match nav::generate_nav_xhtml(
                &ncx_path,
                &package_dir.join("nav.xhtml"),
                &info.guide_refs,
            ) {
                Ok(()) => {
                    add_nav = true;
                    progress(
                        "  [+] Registered nav.xhtml with properties='nav' in package document.",
                    );
                }
                Err(e) => progress(&format!(
                    "  [!] Failed to generate Navigation Document: {}",
                    e
                )),
            }
        }
    }

    if info.has_guide {
        progress("  [+] Replaced legacy <guide> element with HTML5 landmarks navigation.");
    }

    // Standardize content DOCTYPEs and namespaces.
    util::standardize_xhtml_files(temp_path, &progress)?;

    // Presentational attributes HTML no longer allows (`<img width="40%">`,
    // `cellspacing`, …), written as the CSS HTML maps them to.
    let presentational = presentational::convert_book(temp_path, &package_dir, &progress)?;

    // Outdated/deprecated features for the audit report: the OPF-level ones
    // always; plus, for a 3.4 target, content documents still in HTML syntax
    // (EPUB 3.4 is XHTML-only — we flag, we don't rewrite them).
    let mut outdated = info.outdated_features.clone();
    if options.target_version == EpubVersion::V3_4 {
        let html = util::detect_html_syntax(temp_path);
        for w in &html {
            progress(&format!("  [!] {w}"));
        }
        outdated.extend(html);
    }

    // Rewrite the OPF with all upgrades and write it back.
    let params = RewriteParams {
        modified_ts: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        manifest_changes: opt.manifest_changes.into_iter().collect::<HashMap<_, _>>(),
        add_nav,
        remove_guide: info.has_guide,
        // `pageBreakSource` is an EPUB 3.4 meta property — only modernize to it
        // when targeting 3.4 (it would be out-of-vocabulary in a 3.3 package).
        page_break_source: match options.target_version {
            EpubVersion::V3_4 => info.page_break_source.clone(),
            _ => None,
        },
        add_stylesheet: presentational.stylesheet.clone(),
    };
    let new_opf = opf::rewrite_opf(&opf_xml, &params)?;
    fs::write(&opf_path, &new_opf)?;
    entry_names.add_manifest(temp_path, &opf_path, &new_opf)?;

    // Step 5: Repackage EPUB.
    match options.packaging {
        Packaging::Deflate => {
            progress("[*] Repackaging folder into EPUB file...");
            repackage_epub(temp_path, &output_path, &entry_names)?;
            // Step 5a: what EPUB 3 requires of a book EPUB 2 accepted, by epubsana.
            progress("[*] Applying EPUB 3 fixes (epubsana)...");
            upgrade_fixes::apply(&output_path, &progress)?;
            // Step 5b: Kobo koboSpan markup when targeting .kepub (kepubverto
            // works on the packaged book).
            if options.kepub {
                progress("[*] Adding Kobo koboSpan markup (.kepub)...");
                kepub::kobo_spanify(&output_path, &progress)?;
            }
        }
        Packaging::Zstd { mode, level } => {
            #[cfg(feature = "zstd-experimental")]
            {
                progress(
                    "[*] Repackaging with EXPERIMENTAL Zstandard (method 93) — \
                     NOT a conformant EPUB; will not open in current readers...",
                );
                repackage_epub_zstd(temp_path, &output_path, &entry_names, mode, level)?;
            }
            #[cfg(not(feature = "zstd-experimental"))]
            {
                let _ = (mode, level);
                bail!(
                    "Zstandard packaging requires building with the \
                     `zstd-experimental` feature."
                );
            }
        }
    }

    let final_size = fs::metadata(&output_path)?.len();
    let output_name = output_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    Ok(Report {
        input_name,
        output_name,
        output_path,
        original_size,
        final_size,
        image_metrics: opt.metrics,
        target_version: options.target_version,
        outdated_features: outdated,
        presentational,
    })
}

/// Upper bounds on extraction, to defend against zip-bombs when processing
/// untrusted input (e.g. the web service). Legitimate EPUBs are far below these.
pub(crate) const MAX_ARCHIVE_ENTRIES: usize = 50_000;
pub(crate) const MAX_TOTAL_UNCOMPRESSED: u64 = 1024 * 1024 * 1024; // 1 GiB

/// A reader over entry `index`'s uncompressed bytes. Stored and deflated
/// entries are inflated here from their raw bytes: zip 9 fails any archive read
/// past the uncompressed size the header declares (zip-rs/zip2#1022), with no
/// switch to turn that off, where zip 8 and epubcheck read such an entry whole.
/// The CRC is not checked, as in epubcheck and the family's tools. The header
/// size is never trusted: callers bound what they read. Anything else (another
/// method, an encrypted entry) goes through zip's own reader, as before.
pub(crate) fn entry_reader<'a, R: io::Read + io::Seek + 'a>(
    zip: &'a mut ZipArchive<R>,
    index: usize,
) -> zip::result::ZipResult<Box<dyn io::Read + 'a>> {
    let (encrypted, method) = {
        let raw = zip.by_index_raw(index)?;
        (raw.encrypted(), raw.compression())
    };
    Ok(match method {
        CompressionMethod::Stored if !encrypted => Box::new(zip.by_index_raw(index)?),
        CompressionMethod::Deflated if !encrypted => {
            Box::new(flate2::read::DeflateDecoder::new(zip.by_index_raw(index)?))
        }
        _ => Box::new(zip.by_index(index)?),
    })
}

/// Entry `name`'s bytes, at most [`MAX_TOTAL_UNCOMPRESSED`]; `None` when the
/// archive has no such entry.
fn read_named<R: io::Read + io::Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<Vec<u8>>> {
    let Some(index) = zip.index_for_name(name) else {
        return Ok(None);
    };
    let mut data = Vec::new();
    let read = entry_reader(zip, index)?
        .take(MAX_TOTAL_UNCOMPRESSED + 1)
        .read_to_end(&mut data)? as u64;
    if read > MAX_TOTAL_UNCOMPRESSED {
        bail!("EPUB is too large when decompressed (possible zip bomb).");
    }
    Ok(Some(data))
}

/// The names a book's entries must keep when a folder's name is spelled two ways
/// (`images/` and `Images/`). A file system that ignores case, as on macOS and
/// Windows, holds both in one folder, so walking the working directory would
/// move the entries and break the manifest. Empty for any other book, whose
/// names the working directory keeps as they are.
#[derive(Default)]
struct EntryNames(HashMap<String, String>);

impl EntryNames {
    /// The names to keep, given every file entry of the book.
    fn from_files(files: &HashSet<String>) -> Self {
        let mut spellings: HashMap<String, HashSet<&str>> = HashMap::new();
        for file in files {
            for (end, _) in file.match_indices('/') {
                let folder = &file[..end];
                spellings
                    .entry(folder.to_lowercase())
                    .or_default()
                    .insert(folder);
            }
        }
        if spellings.values().all(|s| s.len() == 1) {
            return Self::default();
        }
        Self(
            files
                .iter()
                .map(|file| (file.to_lowercase(), file.clone()))
                .collect(),
        )
    }

    /// Name the files the upgrade added or renamed, from the manifest it wrote.
    fn add_manifest(&mut self, root: &Path, opf_path: &Path, opf_xml: &str) -> Result<()> {
        if self.0.is_empty() {
            return Ok(());
        }
        let opf_name = self.archive_name(&relative_name(root, opf_path)?);
        let base = opf_name.rsplit_once('/').map_or("", |(dir, _)| dir);
        for item in opf::parse_opf_info(opf_xml)?.items {
            if let Some(name) = cover::resolve(base, &item.href) {
                self.0.entry(name.to_lowercase()).or_insert(name);
            }
        }
        Ok(())
    }

    /// The name in the archive of a file the working directory calls `name`.
    fn archive_name(&self, name: &str) -> String {
        self.0
            .get(&name.to_lowercase())
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }
}

/// `path` as an archive name relative to `root`.
fn relative_name(root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)?
        .to_string_lossy()
        .replace('\\', "/"))
}

/// Extract every entry of the EPUB zip into `dest`. Enforces an entry-count cap
/// and a total-uncompressed-size budget (header sizes are attacker-controlled,
/// so the budget is enforced against bytes actually written). Refuses a book two
/// of whose files this file system would hold as one.
fn extract_epub(input: &Path, dest: &Path) -> Result<EntryNames> {
    let file = File::open(input)?;
    let mut zip = ZipArchive::new(file)?;

    if zip.len() > MAX_ARCHIVE_ENTRIES {
        bail!(
            "EPUB has too many entries ({}); refusing to extract.",
            zip.len()
        );
    }

    let mut budget: u64 = MAX_TOTAL_UNCOMPRESSED;
    let mut files = HashSet::new();
    for i in 0..zip.len() {
        let (name, is_dir) = {
            let entry = zip.by_index_raw(i)?;
            match entry.enclosed_name() {
                Some(p) => (p, entry.is_dir()),
                None => continue, // skip unsafe (zip-slip) paths
            }
        };
        let out_path = dest.join(&name);

        if is_dir {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let name = name.to_string_lossy().replace('\\', "/");
            if out_path.exists() && !files.contains(&name) {
                bail!(
                    "'{name}' and another file in this EPUB differ only in case, and this \
                     file system cannot hold both."
                );
            }
            files.insert(name);
            let mut out = File::create(&out_path)?;
            // Copy at most `budget + 1` bytes; overshooting means a bomb.
            let written = io::copy(&mut entry_reader(&mut zip, i)?.take(budget + 1), &mut out)?;
            if written > budget {
                bail!("EPUB is too large when decompressed (possible zip bomb).");
            }
            budget -= written;
        }
    }
    Ok(EntryNames::from_files(&files))
}

/// Read `META-INF/container.xml` to find the OPF package document path.
fn locate_opf(temp_dir: &Path) -> Result<PathBuf> {
    let container_path = temp_dir.join("META-INF").join("container.xml");
    if !container_path.exists() {
        bail!("Invalid EPUB: META-INF/container.xml is missing.");
    }

    let xml = fs::read_to_string(&container_path)?;
    let doc = crate::util::parse_xml(&xml).context("Failed to parse container.xml")?;

    let rootfile = doc
        .descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "rootfile"
                && n.attribute("full-path").is_some()
        })
        .context("Could not find rootfile element in container.xml")?;

    let full_path = rootfile.attribute("full-path").unwrap();
    Ok(temp_dir.join(full_path))
}

/// Repackage the working directory into a valid EPUB zip. `mimetype` is written
/// first and stored uncompressed; everything else is deflated.
fn repackage_epub(temp_dir: &Path, output: &Path, names: &EntryNames) -> Result<()> {
    let file = File::create(output)?;
    let mut zip = ZipWriter::new(file);

    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // 1. mimetype first, uncompressed.
    let mimetype_path = temp_dir.join("mimetype");
    zip.start_file("mimetype", stored)?;
    if mimetype_path.exists() {
        let data = fs::read(&mimetype_path)?;
        io::Write::write_all(&mut zip, &data)?;
    } else {
        io::Write::write_all(&mut zip, b"application/epub+zip")?;
    }

    // 2. Everything else, deflated.
    for entry in WalkDir::new(temp_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let arcname = names.archive_name(&relative_name(temp_dir, path)?);
        if arcname == "mimetype" {
            continue;
        }
        zip.start_file(arcname, deflated)?;
        let data = fs::read(path)?;
        io::Write::write_all(&mut zip, &data)?;
    }

    zip.finish()?;
    Ok(())
}

/// Collect the working directory into ordered [`zstd_ocf::OcfEntry`]s — the
/// `mimetype` first (stored), every other file after — mirroring the entry
/// ordering of [`repackage_epub`].
#[cfg(feature = "zstd-experimental")]
fn collect_ocf_entries(temp_dir: &Path, names: &EntryNames) -> Result<Vec<zstd_ocf::OcfEntry>> {
    let mut entries = Vec::new();

    let mimetype_path = temp_dir.join("mimetype");
    let mimetype = if mimetype_path.exists() {
        fs::read(&mimetype_path)?
    } else {
        b"application/epub+zip".to_vec()
    };
    entries.push(zstd_ocf::OcfEntry {
        name: "mimetype".to_string(),
        data: mimetype,
    });

    for entry in WalkDir::new(temp_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let arcname = names.archive_name(&relative_name(temp_dir, path)?);
        if arcname == "mimetype" {
            continue;
        }
        entries.push(zstd_ocf::OcfEntry {
            name: arcname,
            data: fs::read(path)?,
        });
    }
    Ok(entries)
}

/// Repackage the working directory into an **experimental** Zstd-OCF archive
/// (`mimetype` stored, everything else compressed with method 93). The result
/// is intentionally non-conformant; see [`Packaging::Zstd`].
#[cfg(feature = "zstd-experimental")]
fn repackage_epub_zstd(
    temp_dir: &Path,
    output: &Path,
    names: &EntryNames,
    mode: ZstdMode,
    level: i32,
) -> Result<()> {
    let entries = collect_ocf_entries(temp_dir, names)?;
    let archive = match mode {
        ZstdMode::PerEntry => zstd_ocf::pack_zstd(&entries, level)?,
        // Size-safe: keep the dictionary only when it actually wins.
        ZstdMode::SharedDict => zstd_ocf::pack_zstd_best(&entries, level)?,
    };
    fs::write(output, archive)?;
    Ok(())
}

/// Decode an experimental Zstd-OCF archive back into a conformant Deflate EPUB.
///
/// This is the round-trip / "no data loss" path: every entry is decompressed
/// (CRC-checked inside [`zstd_ocf::unpack_zstd`]) and rewritten as a normal
/// Stored-mimetype + Deflate EPUB at `output`. The reconstructed container is a
/// valid EPUB that opens in any reader.
#[cfg(feature = "zstd-experimental")]
pub fn decode_zstd_epub(input: &Path, output: &Path) -> Result<()> {
    let archive = fs::read(input)
        .with_context(|| format!("Failed to read Zstd-OCF input: {}", input.display()))?;
    let entries = zstd_ocf::unpack_zstd(&archive)?;

    let file = File::create(output)?;
    let mut zip = ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in &entries {
        let opts = if entry.name == "mimetype" {
            stored
        } else {
            deflated
        };
        zip.start_file(&entry.name, opts)?;
        io::Write::write_all(&mut zip, &entry.data)?;
    }
    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_is_the_default_version() {
        assert_eq!(EpubVersion::default(), EpubVersion::V3_3);
        assert_eq!(Options::default().target_version, EpubVersion::LATEST);
    }

    #[test]
    fn default_output_is_version_stamped() {
        let opts = Options::default();
        let p = default_output_path(Path::new("/books/My Book.epub"), &opts);
        assert_eq!(p, PathBuf::from("/books/My Book_v3.3.epub"));
    }

    #[test]
    fn ascii_option_slugifies_the_output_name() {
        let opts = Options {
            ascii: true,
            ..Options::default()
        };
        let p = default_output_path(Path::new("/b/Işık Doğudan Yükselir.epub"), &opts);
        assert_eq!(p, PathBuf::from("/b/Isik_Dogudan_Yukselir_v3.3.epub"));
    }
}
