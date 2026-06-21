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

#[cfg(feature = "archival")]
pub mod eparc;
mod images;
mod kepub;
mod nav;
mod opf;
mod report;
mod util;
#[cfg(feature = "zstd-experimental")]
pub mod zstd_ocf;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use opf::RewriteParams;

pub use images::{FormatPolicy, ImageFormat, ImageMetric};

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
/// `WebP` is the default and yields the smallest files on readers that support
/// it (Apple Books, Calibre, most apps). `KeepOriginal` leaves JPEG/PNG images
/// untouched for maximum device compatibility — notably **Kobo e-ink readers do
/// not render WebP** despite claiming EPUB 3.3 support, so a Kobo target needs
/// the originals kept. The enum leaves room for AVIF/JXL once EPUB 3.4 lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageStrategy {
    /// Convert JPEG/PNG to WebP (size-safe: never grows, never upscales).
    #[default]
    WebP,
    /// Leave images in their original format; only upgrade structure.
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
    /// How raster images are handled. Note: [`Options::kepub`] forces
    /// [`ImageStrategy::KeepOriginal`] regardless of this value, since Kobo
    /// devices can't render WebP.
    pub image_strategy: ImageStrategy,
    /// Produce a Kobo `.kepub.epub`: inject `koboSpan` markup into the content
    /// documents and name the output `<stem>.kepub.epub`. The result is still a
    /// valid EPUB 3 on top of the normal upgrades.
    pub kepub: bool,
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
/// a Kobo `book.kepub.epub` when [`Options::kepub`] is set, or a
/// `book_zstd-experimental.epub` when [`Packaging::Zstd`] is selected.
///
/// The `_zstd-experimental` suffix is deliberately *not* `_v3.x`: a Zstd archive
/// is non-conformant, so a version-looking name would misrepresent it. The
/// suffix is tied to the conformance axis and stays until the output actually
/// becomes conformant — not when our measurements mature.
pub fn default_output_path(input: &Path, options: &Options) -> PathBuf {
    let stem = output_stem(input, options.ascii);
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let base = if options.kepub {
        format!("{}.kepub", stem)
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
    progress("[*] Extracting original EPUB file...");
    extract_epub(&input_path, temp_path)?;

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
    // forces KeepOriginal because Kobo e-ink can't render WebP.
    let image_strategy = if options.kepub {
        ImageStrategy::KeepOriginal
    } else {
        options.image_strategy
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
    };
    let new_opf = opf::rewrite_opf(&opf_xml, &params)?;
    fs::write(&opf_path, new_opf)?;

    // Step 4b: Inject Kobo koboSpan markup when targeting .kepub.
    if options.kepub {
        progress("[*] Injecting Kobo koboSpan markup (.kepub)...");
        kepub::kobo_spanify(temp_path, &progress)?;
    }

    // Step 5: Repackage EPUB.
    match options.packaging {
        Packaging::Deflate => {
            progress("[*] Repackaging folder into EPUB file...");
            repackage_epub(temp_path, &output_path)?;
        }
        Packaging::Zstd { mode, level } => {
            #[cfg(feature = "zstd-experimental")]
            {
                progress(
                    "[*] Repackaging with EXPERIMENTAL Zstandard (method 93) — \
                     NOT a conformant EPUB; will not open in current readers...",
                );
                repackage_epub_zstd(temp_path, &output_path, mode, level)?;
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
        outdated_features: info.outdated_features,
    })
}

/// Upper bounds on extraction, to defend against zip-bombs when processing
/// untrusted input (e.g. the web service). Legitimate EPUBs are far below these.
const MAX_ARCHIVE_ENTRIES: usize = 50_000;
const MAX_TOTAL_UNCOMPRESSED: u64 = 1024 * 1024 * 1024; // 1 GiB

/// Extract every entry of the EPUB zip into `dest`. Enforces an entry-count cap
/// and a total-uncompressed-size budget (header sizes are attacker-controlled,
/// so the budget is enforced against bytes actually written).
fn extract_epub(input: &Path, dest: &Path) -> Result<()> {
    let file = File::open(input)?;
    let mut zip = ZipArchive::new(file)?;

    if zip.len() > MAX_ARCHIVE_ENTRIES {
        bail!(
            "EPUB has too many entries ({}); refusing to extract.",
            zip.len()
        );
    }

    let mut budget: u64 = MAX_TOTAL_UNCOMPRESSED;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let out_path = match entry.enclosed_name() {
            Some(p) => dest.join(p),
            None => continue, // skip unsafe (zip-slip) paths
        };

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&out_path)?;
            // Copy at most `budget + 1` bytes; overshooting means a bomb.
            let written = io::copy(&mut entry.by_ref().take(budget + 1), &mut out)?;
            if written > budget {
                bail!("EPUB is too large when decompressed (possible zip bomb).");
            }
            budget -= written;
        }
    }
    Ok(())
}

/// Read `META-INF/container.xml` to find the OPF package document path.
fn locate_opf(temp_dir: &Path) -> Result<PathBuf> {
    let container_path = temp_dir.join("META-INF").join("container.xml");
    if !container_path.exists() {
        bail!("Invalid EPUB: META-INF/container.xml is missing.");
    }

    let xml = fs::read_to_string(&container_path)?;
    let doc = roxmltree::Document::parse(&xml).context("Failed to parse container.xml")?;

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
fn repackage_epub(temp_dir: &Path, output: &Path) -> Result<()> {
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
        let rel = path.strip_prefix(temp_dir)?;
        let arcname = rel.to_string_lossy().replace('\\', "/");
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
fn collect_ocf_entries(temp_dir: &Path) -> Result<Vec<zstd_ocf::OcfEntry>> {
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
        let rel = path.strip_prefix(temp_dir)?;
        let arcname = rel.to_string_lossy().replace('\\', "/");
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
fn repackage_epub_zstd(temp_dir: &Path, output: &Path, mode: ZstdMode, level: i32) -> Result<()> {
    let entries = collect_ocf_entries(temp_dir)?;
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
