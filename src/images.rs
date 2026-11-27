//! Image optimization: re-encode raster manifest images (JPEG/PNG) and update
//! every reference to them across the package.
//!
//! One image in, one image out is imagoverto's work: decoding, encoding, the
//! 1–100 quality scale, gray, alpha, limits. This module holds the book's
//! rules on top of it: which images, into what format for the EPUB version,
//! the quality cap at a source JPEG's own, keep the original when the result
//! is not smaller, and the manifest and reference rewrite.

use crate::opf::{ImageChange, ManifestItem};
use crate::util::{basename, quote, unquote, with_ext};
use anyhow::Result;
use image::DynamicImage;
use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use walkdir::WalkDir;

/// Output image format the optimizer re-encodes raster images to. WebP is the
/// EPUB 3.3 target; JPEG XL (and AVIF, once imagoverto writes it) become core
/// media types in EPUB 3.4 and are emitted under `--target 3.4` (experimental,
/// behind the `epub34` feature). JPEG and PNG, core in every EPUB version, are
/// the way back for readers that show none of the newer formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFormat {
    #[default]
    WebP,
    Avif,
    Jxl,
    Jpeg,
    Png,
}

/// Why AVIF output is refused: imagoverto 0.1 writes no AVIF (its encoder,
/// zenravif 0.1.3, wrote 9 of 120 shelf images visibly corrupt). It returns
/// with zenravif 0.2.
pub const AVIF_PAUSED: &str = "AVIF output is paused until its encoder is fixed: it corrupted some \
     images. Use JPEG XL (`jxl`) or WebP instead";

impl ImageFormat {
    /// File extension (no dot) for this format's output files.
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::WebP => "webp",
            ImageFormat::Avif => "avif",
            ImageFormat::Jxl => "jxl",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
        }
    }

    /// OPF manifest media type for this format.
    pub fn media_type(self) -> &'static str {
        match self {
            ImageFormat::WebP => "image/webp",
            ImageFormat::Avif => "image/avif",
            ImageFormat::Jxl => "image/jxl",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
        }
    }

    /// Short label for progress/log lines.
    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::WebP => "WebP",
            ImageFormat::Avif => "AVIF",
            ImageFormat::Jxl => "JPEG XL",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Png => "PNG",
        }
    }

    fn to_imagoverto(self) -> imagoverto::Format {
        match self {
            ImageFormat::WebP => imagoverto::Format::WebP,
            ImageFormat::Avif => imagoverto::Format::Avif,
            ImageFormat::Jxl => imagoverto::Format::Jxl,
            ImageFormat::Jpeg => imagoverto::Format::Jpeg,
            ImageFormat::Png => imagoverto::Format::Png,
        }
    }
}

/// How the output format is chosen for each image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatPolicy {
    /// The same format for every image (an explicit `--image-format`, or the
    /// EPUB 3.3 default of WebP).
    Fixed(ImageFormat),
    /// Content-adaptive (the EPUB 3.4 default): pick per image from its *source*
    /// format, a free content-type signal — publishers use JPEG for photographs
    /// and PNG for line-art/screenshots. **JPEG → JPEG XL, PNG → WebP** while
    /// AVIF output is paused (it was JPEG → AVIF; see
    /// docs/design/epub-3.4-image-codec-choice.md).
    Auto,
    /// Thorough: encode every candidate format per image and keep the smallest
    /// (at matched quality, since the quality scale is one across formats).
    Best,
}

impl FormatPolicy {
    /// The output format for a source of the given media type. Only meaningful
    /// for `Fixed`/`Auto`; `Best` selects per image by encoding all candidates,
    /// so it never calls this (the `WebP` arm is an unreachable safe default).
    pub fn format_for(self, media_type: &str) -> ImageFormat {
        match self {
            FormatPolicy::Fixed(f) => f,
            FormatPolicy::Auto => match media_type {
                "image/jpeg" | "image/jpg" => ImageFormat::Jxl,
                _ => ImageFormat::WebP,
            },
            FormatPolicy::Best => ImageFormat::WebP,
        }
    }

    /// Short label for the conversion progress header.
    pub fn label(self) -> &'static str {
        match self {
            FormatPolicy::Fixed(f) => f.label(),
            FormatPolicy::Auto => "JPEG XL for photos, WebP for line-art",
            FormatPolicy::Best => "smallest per image",
        }
    }

    /// The formats tried for one image.
    fn candidates(self, media_type: &str) -> Vec<ImageFormat> {
        match self {
            FormatPolicy::Fixed(f) => vec![f],
            FormatPolicy::Auto => vec![self.format_for(media_type)],
            FormatPolicy::Best => vec![ImageFormat::WebP, ImageFormat::Jxl],
        }
    }
}

/// Per-image size statistics, used by the report.
#[derive(Debug, Clone)]
pub struct ImageMetric {
    pub name: String,
    pub original_size: u64,
    pub new_size: u64,
    pub percentage: f64,
    /// True when the original was kept because the re-encode came out no smaller.
    pub kept: bool,
    /// The output format the image was re-encoded to (`None` when kept as-is).
    pub format: Option<ImageFormat>,
}

/// Result of the optimization pass.
pub struct OptimizeResult {
    pub metrics: Vec<ImageMetric>,
    /// Keyed by the raw manifest `href` -> how the manifest item is rewritten.
    pub manifest_changes: HashMap<String, ImageChange>,
    /// (old_reference, new_reference) pairs for updating document/style files.
    pub ref_pairs: Vec<(String, String)>,
}

/// Raster media types we convert.
fn is_target_media_type(mt: &str) -> bool {
    matches!(mt, "image/jpeg" | "image/jpg" | "image/png")
}

/// Convert all eligible manifest images by `policy` at the given quality,
/// deleting originals and recording the manifest/reference changes.
pub fn optimize_images(
    package_dir: &Path,
    items: &[ManifestItem],
    cover_id: Option<&str>,
    quality: u8,
    policy: FormatPolicy,
    progress: &dyn Fn(&str),
) -> Result<OptimizeResult> {
    if policy == FormatPolicy::Fixed(ImageFormat::Avif) {
        anyhow::bail!("{AVIF_PAUSED}.");
    }
    let mut result = OptimizeResult {
        metrics: Vec::new(),
        manifest_changes: HashMap::new(),
        ref_pairs: Vec::new(),
    };

    for item in items {
        if !is_target_media_type(&item.media_type) {
            continue;
        }

        // Hrefs are URL-encoded; decode to obtain the real on-disk path.
        let decoded_href = unquote(&item.href);
        let img_path = package_dir.join(&decoded_href);

        if !img_path.exists() {
            progress(&format!(
                "  [!] Warning: Image file not found: {}",
                basename(&decoded_href)
            ));
            continue;
        }

        let old_name = basename(&decoded_href).to_string();

        let orig_bytes = match fs::read(&img_path) {
            Ok(b) => b,
            Err(e) => {
                progress(&format!("  [!] Failed to read image {old_name}: {e}"));
                continue;
            }
        };
        let orig_size = orig_bytes.len() as u64;

        // Encode to memory first so we can compare sizes against the original.
        // `Best` tries every candidate and keeps the smallest — valid because
        // the quality scale is one across formats, so smallest bytes = smallest
        // at matched quality.
        let mut best: Option<(ImageFormat, Vec<u8>)> = None;
        let mut last_err = None;
        for cand in policy.candidates(&item.media_type) {
            match reencode(&orig_bytes, quality, cand) {
                Ok(bytes) if best.as_ref().is_none_or(|(_, b)| bytes.len() < b.len()) => {
                    best = Some((cand, bytes));
                }
                Ok(_) => {}
                Err(e) => last_err = Some(e),
            }
        }
        let (format, encoded) = match best {
            Some(x) => x,
            None => {
                let why = last_err.map(|e| format!(": {e}")).unwrap_or_default();
                progress(&format!("  [!] Failed to convert image {old_name}{why}"));
                continue;
            }
        };
        let new_size = encoded.len() as u64;

        // Keep whichever is smaller. If the re-encode isn't actually smaller,
        // leave the original untouched — no rewrite, no manifest/reference change —
        // so the output can never grow.
        if new_size >= orig_size {
            progress(&format!(
                "  [=] Kept original ({} not smaller): {} ({:.1}KB <= {:.1}KB)",
                format.label(),
                old_name,
                orig_size as f64 / 1024.0,
                new_size as f64 / 1024.0
            ));
            result.metrics.push(ImageMetric {
                name: old_name,
                original_size: orig_size,
                new_size: orig_size,
                percentage: 0.0,
                kept: true,
                format: None,
            });
            continue;
        }

        // The output extension/path follow the chosen format (known only now for
        // the `Best` policy, which selects per image).
        let new_href = with_ext(&decoded_href, format.extension());
        let new_img_path = package_dir.join(&new_href);
        if let Err(e) = fs::write(&new_img_path, &encoded) {
            progress(&format!(
                "  [!] Failed to write {}: {e}",
                basename(&new_href)
            ));
            continue;
        }
        let _ = fs::remove_file(&img_path);

        let pct = (orig_size as i64 - new_size as i64) as f64 / orig_size as f64 * 100.0;
        let new_name = basename(&new_href).to_string();

        result.metrics.push(ImageMetric {
            name: old_name.clone(),
            original_size: orig_size,
            new_size,
            percentage: pct,
            kept: false,
            format: Some(format),
        });

        progress(&format!(
            "  [+] Converted: {} -> {} ({:.1}KB -> {:.1}KB, {:.1}% saved)",
            old_name,
            new_name,
            orig_size as f64 / 1024.0,
            new_size as f64 / 1024.0,
            pct
        ));

        // Determine whether this is the cover image.
        let id_lower = item.id.to_lowercase();
        let href_lower = item.href.to_lowercase();
        let is_cover = cover_id.map(|c| c == item.id).unwrap_or(false)
            || id_lower.contains("cover")
            || href_lower.contains("cover");

        result.manifest_changes.insert(
            item.href.clone(),
            ImageChange {
                new_href_encoded: quote(&new_href),
                media_type: format.media_type().to_string(),
                is_cover,
            },
        );

        // Both the raw and decoded hrefs should be remapped to the new href.
        push_unique(&mut result.ref_pairs, item.href.clone(), new_href.clone());
        push_unique(
            &mut result.ref_pairs,
            decoded_href.clone(),
            new_href.clone(),
        );
    }

    Ok(result)
}

/// The libjpeg quality (1–100) a JPEG was saved at, read by imagoverto from
/// its quantization table (exact for libjpeg-scaled tables); `None` for
/// anything that is not a readable JPEG.
pub(crate) fn source_jpeg_quality(data: &[u8]) -> Option<u8> {
    imagoverto::inspect(data).ok()?.jpeg_quality
}

/// The libjpeg quality imagoverto writes for a JPEG target at `quality` on its
/// own (WebP) scale. A copy of imagoverto 0.1.4's `src/encode.rs` curve, which
/// is not public API: imagoverto announces a refit in its release notes.
fn imagoverto_jpeg_quality(quality: u8) -> u8 {
    let q = f32::from(quality).clamp(45.0, 95.0);
    (0.004615 * q * q - 0.17453 * q + 65.691).round() as u8
}

/// The highest imagoverto quality whose JPEG output is not above `source` on
/// the libjpeg scale; `None` when even its lowest (libjpeg 67) is above it.
fn jpeg_cap(source: u8) -> Option<u8> {
    (1..=100u8)
        .rev()
        .find(|&q| imagoverto_jpeg_quality(q) <= source)
}

/// The quality `original` is re-encoded at for `format`: `quality`, capped for
/// a JPEG source at the source's own (re-encoding above it only inflates).
/// `Ok(None)` is a lossless target (PNG). An error when the cap cannot hold:
/// a JPEG target for a source JPEG saved below anything imagoverto writes.
fn target_quality(original: &[u8], quality: u8, format: ImageFormat) -> Result<Option<u8>> {
    let source = source_jpeg_quality(original);
    Ok(match (format, source) {
        (ImageFormat::Png, _) => None,
        (ImageFormat::Jpeg, Some(src)) => match jpeg_cap(src) {
            Some(cap) => Some(quality.min(cap)),
            None => anyhow::bail!(
                "the source JPEG (quality {src}) is below the lowest JPEG quality written; kept"
            ),
        },
        (_, Some(src)) => Some(quality.min(src)),
        (_, None) => Some(quality),
    })
}

/// Re-encode one in-memory raster image to `format` by the optimizer's own
/// rules (the JPEG quality cap; gray, alpha and limits are imagoverto's),
/// without the keep-if-not-smaller decision: the bytes [`optimize_images`]
/// would weigh against the original. The building block of size estimates.
pub fn reencode(original: &[u8], quality: u8, format: ImageFormat) -> Result<Vec<u8>> {
    if format == ImageFormat::Avif {
        anyhow::bail!("{AVIF_PAUSED}.");
    }
    let mut options = imagoverto::Options::new(format.to_imagoverto());
    if let Some(q) = target_quality(original, quality, format)? {
        options = options.with_quality(q);
    }
    Ok(imagoverto::convert(original, &options)?.bytes)
}

/// One raster image of a book, as an estimate reads it.
pub struct EstimateInput<'a> {
    pub media_type: &'a str,
    pub bytes: &'a [u8],
}

/// What re-encoding a book's images to one format would come to.
///
/// Both totals cover every image: the measured ones by their real encode, the
/// rest by the measured images' ratio. When `images_measured == images`, the
/// totals are exact, not an estimate.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatEstimate {
    pub format: ImageFormat,
    /// The images as they are now.
    pub original_bytes: u64,
    /// Each image re-encoded, or kept where the re-encode is not smaller (the
    /// optimizer's default rule).
    pub smallest_bytes: u64,
    /// Every image re-encoded, whatever the size (a format chosen for a device).
    pub converted_bytes: u64,
    pub images: usize,
    pub images_measured: usize,
    /// Images that could not be decoded or encoded; they stay as they are and
    /// are counted at their original size.
    pub images_failed: usize,
}

impl FormatEstimate {
    pub fn is_exact(&self) -> bool {
        self.images_measured + self.images_failed == self.images
    }
}

/// Estimate the bytes of `images` re-encoded to `format` within `budget` of
/// encode time.
///
/// Measured on 77 shelf books (2026-10-29), a time budget beats a fixed sample
/// size: most books then fit whole and the answer is exact. When a book does
/// not fit, its largest images are encoded first for half the budget (they
/// carry most of the bytes), then an evenly spaced sample of the rest; the
/// unmeasured images are scaled by the sample's ratio. Typical error under
/// 1% for WebP (5 s) and JPEG XL (10 s), with rare books off by up to about
/// 18%. One image is always measured, even past the budget.
pub fn estimate_format(
    images: &[EstimateInput<'_>],
    format: ImageFormat,
    quality: u8,
    budget: std::time::Duration,
) -> FormatEstimate {
    let start = std::time::Instant::now();
    let mut order: Vec<usize> = (0..images.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(images[i].bytes.len()));

    let mut est = FormatEstimate {
        format,
        original_bytes: images.iter().map(|i| i.bytes.len() as u64).sum(),
        smallest_bytes: 0,
        converted_bytes: 0,
        images: images.len(),
        images_measured: 0,
        images_failed: 0,
    };
    let mut measured = vec![false; images.len()];
    let encode = |i: usize, est: &mut FormatEstimate, measured: &mut [bool]| {
        let img = &images[i];
        let orig = img.bytes.len() as u64;
        // imagoverto turns a codec panic into an error, except on wasm.
        match reencode(img.bytes, quality, format) {
            Ok(b) => {
                est.smallest_bytes += (b.len() as u64).min(orig);
                est.converted_bytes += b.len() as u64;
                est.images_measured += 1;
            }
            Err(_) => {
                est.smallest_bytes += orig;
                est.converted_bytes += orig;
                est.images_failed += 1;
            }
        }
        measured[i] = true;
    };

    // Largest first, for half the budget.
    let mut k = 0;
    while k < order.len() && (k == 0 || start.elapsed() < budget / 2) {
        encode(order[k], &mut est, &mut measured);
        k += 1;
    }
    // Then an evenly spaced sample of the rest, smallest stride first, so the
    // sample spreads over the whole size range before it thickens.
    let rest = &order[k..];
    let mut stride = rest.len().next_power_of_two();
    'sample: while stride >= 1 {
        for &i in rest.iter().step_by(stride) {
            if start.elapsed() >= budget {
                break 'sample;
            }
            if !measured[i] {
                encode(i, &mut est, &mut measured);
            }
        }
        stride /= 2;
    }

    // Scale the unmeasured images by the measured ones' ratio.
    let (mut m_orig, mut rest_orig) = (0u64, 0u64);
    for (i, img) in images.iter().enumerate() {
        if measured[i] {
            m_orig += img.bytes.len() as u64;
        } else {
            rest_orig += img.bytes.len() as u64;
        }
    }
    if rest_orig > 0 && m_orig > 0 {
        let scale = |v: u64| v + (rest_orig as f64 * v as f64 / m_orig as f64).round() as u64;
        est.smallest_bytes = scale(est.smallest_bytes);
        est.converted_bytes = scale(est.converted_bytes);
    }
    est
}

/// Re-encode a standalone raster image (e.g. an import cover) to WebP at
/// `quality`, returning the WebP bytes **only if they're smaller** than the
/// original (the same size-safe rule the EPUB optimizer uses). `None` means
/// "keep the original" — the conversion failed, or wasn't smaller.
///
/// Unlike [`optimize_images`], this works on in-memory bytes with no manifest,
/// so the Markdown importer can shrink a cover before embedding it.
#[cfg(feature = "markdown")]
pub fn optimize_to_webp(original: &[u8], quality: u8) -> Option<Vec<u8>> {
    let webp = reencode(original, quality, ImageFormat::WebP).ok()?;
    (webp.len() < original.len()).then_some(webp)
}

fn push_unique(pairs: &mut Vec<(String, String)>, old: String, new: String) {
    if !pairs.iter().any(|(o, _)| *o == old) {
        pairs.push((old, new));
    }
}

/// Bounds on a single image decode, to defend against decode-bombs (e.g. a tiny
/// PNG declaring 100k×100k), for the decodes still done here (covers, imported
/// images): imagoverto's own limits.
pub(crate) const MAX_IMAGE_DIM: u32 = imagoverto::MAX_DIMENSION;
pub(crate) const MAX_IMAGE_ALLOC: u64 = imagoverto::MAX_DECODED_BYTES;

/// Decode an in-memory JPEG or PNG, enforcing the decode-bomb limits. Used for
/// covers and imported images, which epublift resizes itself.
pub(crate) fn decode_image(bytes: &[u8]) -> Result<DynamicImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    Ok(reader.decode()?)
}

/// Scan XHTML/HTML/CSS/SVG/NCX files and update references to converted images.
pub fn update_document_references(
    root: &Path,
    ref_pairs: &[(String, String)],
    progress: &dyn Fn(&str),
) {
    if ref_pairs.is_empty() {
        return;
    }

    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let is_doc = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "xhtml" | "html" | "htm" | "css" | "svg" | "ncx"
                )
            })
            .unwrap_or(false);
        if !is_doc {
            continue;
        }

        let raw = match fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                progress(&format!(
                    "  [!] Warning: Failed to update references in {}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    e
                ));
                continue;
            }
        };
        let mut content = String::from_utf8_lossy(&raw).into_owned();
        let mut updated = false;

        for (old_href, new_href) in ref_pairs {
            let old_name = basename(old_href);
            let new_name = basename(new_href);
            let old_encoded = quote(old_href);
            let new_encoded = quote(new_href);

            if content.contains(old_href.as_str()) {
                content = content.replace(old_href.as_str(), new_href);
                updated = true;
            }
            if content.contains(&old_encoded) {
                content = content.replace(&old_encoded, &new_encoded);
                updated = true;
            }
            if content.contains(old_name) {
                content = content.replace(old_name, new_name);
                updated = true;
            }
        }

        if updated && let Err(e) = fs::write(path, content) {
            progress(&format!(
                "  [!] Warning: Failed to update references in {}: {}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                e
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// A detailed (gradient) JPEG encoded at a known IJG quality factor.
    fn jpeg_at(quality: u8) -> Vec<u8> {
        let img = RgbImage::from_fn(64, 64, |x, y| Rgb([(x * 4) as u8, (y * 4) as u8, 128]));
        let mut buf = Cursor::new(Vec::new());
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        DynamicImage::ImageRgb8(img)
            .write_with_encoder(enc)
            .unwrap();
        buf.into_inner()
    }

    fn png(img: &DynamicImage) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    /// Decode any of the five formats back to pixels, through imagoverto's
    /// lossless PNG output (the `image` crate here reads only JPEG and PNG).
    fn pixels(bytes: &[u8]) -> DynamicImage {
        let back = imagoverto::convert(bytes, &imagoverto::Options::new(imagoverto::Format::Png))
            .unwrap()
            .bytes;
        image::load_from_memory(&back).unwrap()
    }

    fn gradient() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(48, 32, |x, y| {
            Rgb([(x * 5) as u8, (y * 7) as u8, 90])
        }))
    }

    fn with_alpha(opaque: bool) -> DynamicImage {
        DynamicImage::ImageRgba8(image::RgbaImage::from_fn(16, 16, |x, _| {
            image::Rgba([200, 100, 50, if opaque || x < 8 { 255 } else { 0 }])
        }))
    }

    #[test]
    fn reads_the_source_jpeg_quality_exactly() {
        // The cap's input: the `image` crate scales the Annex K table the
        // libjpeg way, so the reading is exact at every quality.
        for q in 1u8..=100 {
            assert_eq!(source_jpeg_quality(&jpeg_at(q)), Some(q), "quality {q}");
        }
        assert_eq!(source_jpeg_quality(b"this is not a jpeg at all"), None);
        assert_eq!(source_jpeg_quality(&png(&gradient())), None);
    }

    #[test]
    fn a_jpeg_target_is_capped_on_the_libjpeg_scale() {
        // imagoverto's worked examples (note of 2026-11-27).
        for (source, cap) in [(75, 68), (80, 78), (85, 87), (90, 94), (91, 100), (99, 100)] {
            assert_eq!(jpeg_cap(source), Some(cap), "source {source}");
        }
        assert_eq!(jpeg_cap(67), Some(46));
        assert_eq!(jpeg_cap(66), None);
        // What is written never exceeds the source.
        for source in 67..=100u8 {
            let cap = jpeg_cap(source).unwrap();
            let written =
                source_jpeg_quality(&reencode(&jpeg_at(source), 100, ImageFormat::Jpeg).unwrap());
            assert!(written.unwrap() <= source, "source {source}, cap {cap}");
        }
    }

    #[test]
    fn a_jpeg_saved_below_any_jpeg_output_is_not_turned_into_one() {
        assert!(reencode(&jpeg_at(50), 80, ImageFormat::Jpeg).is_err());
        // Other targets cap on their own scale and still convert.
        assert!(reencode(&jpeg_at(50), 80, ImageFormat::WebP).is_ok());
    }

    #[test]
    fn a_webp_target_is_capped_at_the_source_quality() {
        assert_eq!(
            target_quality(&jpeg_at(43), 80, ImageFormat::WebP).unwrap(),
            Some(43)
        );
        assert_eq!(
            target_quality(&jpeg_at(95), 80, ImageFormat::WebP).unwrap(),
            Some(80)
        );
        assert_eq!(
            target_quality(&png(&gradient()), 80, ImageFormat::WebP).unwrap(),
            Some(80)
        );
        assert_eq!(
            target_quality(&jpeg_at(95), 80, ImageFormat::Png).unwrap(),
            None
        );
    }

    #[test]
    fn webp_keeps_a_gray_image_tones() {
        // A 0-255 ramp: zenwebp's L8 input clipped 16 to 0 and 235 to 253.
        let ramp = DynamicImage::ImageLuma8(image::GrayImage::from_fn(256, 16, |x, _| {
            image::Luma([x as u8])
        }));
        for q in [80u8, 95] {
            let back = pixels(&reencode(&png(&ramp), q, ImageFormat::WebP).unwrap()).to_luma8();
            for x in [16u32, 64, 128, 192, 235] {
                let v = back.get_pixel(x, 8)[0] as i32;
                assert!((v - x as i32).abs() <= 3, "q{q}: {x} came back as {v}");
            }
        }
    }

    #[test]
    fn transparency_survives_webp_and_refuses_jpeg() {
        let src = png(&with_alpha(false));
        assert!(
            pixels(&reencode(&src, 90, ImageFormat::WebP).unwrap())
                .color()
                .has_alpha()
        );
        assert!(reencode(&src, 80, ImageFormat::Jpeg).is_err());
        // An alpha channel that is all opaque carries nothing: JPEG is fine.
        assert!(reencode(&png(&with_alpha(true)), 80, ImageFormat::Jpeg).is_ok());
    }

    #[test]
    fn reencode_turns_a_webp_into_jpeg_and_png() {
        let webp = reencode(&png(&gradient()), 85, ImageFormat::WebP).unwrap();
        let jpeg = reencode(&webp, 85, ImageFormat::Jpeg).unwrap();
        assert_eq!(
            imagoverto::Format::sniff(&jpeg),
            Some(imagoverto::Format::Jpeg)
        );
        let png_back = reencode(&webp, 85, ImageFormat::Png).unwrap();
        assert_eq!(
            imagoverto::Format::sniff(&png_back),
            Some(imagoverto::Format::Png)
        );
    }

    #[test]
    fn jpeg_xl_comes_back_at_its_size() {
        let jxl = reencode(&jpeg_at(90), 85, ImageFormat::Jxl).unwrap();
        assert_eq!(
            imagoverto::Format::sniff(&jxl),
            Some(imagoverto::Format::Jxl)
        );
        let img = pixels(&jxl);
        assert_eq!((img.width(), img.height()), (64, 64));
    }

    #[test]
    fn avif_output_is_paused() {
        assert!(reencode(&jpeg_at(90), 85, ImageFormat::Avif).is_err());
        let dir = tempfile::tempdir().unwrap();
        let err = optimize_images(
            dir.path(),
            &[],
            None,
            80,
            FormatPolicy::Fixed(ImageFormat::Avif),
            &|_| {},
        );
        assert!(err.is_err());
        // The 3.4 default sends photos to JPEG XL now, and `best` never tries AVIF.
        assert_eq!(
            FormatPolicy::Auto.format_for("image/jpeg"),
            ImageFormat::Jxl
        );
        assert_eq!(
            FormatPolicy::Auto.format_for("image/png"),
            ImageFormat::WebP
        );
        assert!(
            !FormatPolicy::Best
                .candidates("image/jpeg")
                .contains(&ImageFormat::Avif)
        );
    }

    fn estimate_inputs() -> Vec<Vec<u8>> {
        // Different sizes and content, so the largest-first order matters.
        (1..=6u32)
            .map(|k| {
                let img = DynamicImage::ImageRgb8(RgbImage::from_fn(16 * k, 12 * k, |x, y| {
                    Rgb([(x * 9 + k) as u8, (y * 5) as u8, (x ^ y) as u8])
                }));
                let mut buf = Cursor::new(Vec::new());
                let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
                img.write_with_encoder(enc).unwrap();
                buf.into_inner()
            })
            .collect()
    }

    fn as_inputs(raw: &[Vec<u8>]) -> Vec<EstimateInput<'_>> {
        raw.iter()
            .map(|b| EstimateInput {
                media_type: "image/jpeg",
                bytes: b,
            })
            .collect()
    }

    #[test]
    fn an_estimate_with_time_to_spare_is_exact() {
        let raw = estimate_inputs();
        let est = estimate_format(
            &as_inputs(&raw),
            ImageFormat::WebP,
            80,
            std::time::Duration::from_secs(60),
        );
        assert!(est.is_exact());
        assert_eq!(est.images_measured, 6);
        let sizes: Vec<(u64, u64)> = raw
            .iter()
            .map(|b| {
                let n = reencode(b, 80, ImageFormat::WebP).unwrap().len() as u64;
                (n, n.min(b.len() as u64))
            })
            .collect();
        assert_eq!(est.converted_bytes, sizes.iter().map(|s| s.0).sum::<u64>());
        assert_eq!(est.smallest_bytes, sizes.iter().map(|s| s.1).sum::<u64>());
        assert_eq!(
            est.original_bytes,
            raw.iter().map(|b| b.len() as u64).sum::<u64>()
        );
    }

    #[test]
    fn an_estimate_without_time_measures_the_largest_image_and_scales() {
        let raw = estimate_inputs();
        let est = estimate_format(
            &as_inputs(&raw),
            ImageFormat::WebP,
            80,
            std::time::Duration::ZERO,
        );
        assert_eq!(est.images_measured, 1);
        assert!(!est.is_exact());
        let largest = raw.iter().max_by_key(|b| b.len()).unwrap();
        let one = reencode(largest, 80, ImageFormat::WebP).unwrap().len() as f64;
        let expected = one * est.original_bytes as f64 / largest.len() as f64;
        assert!((est.converted_bytes as f64 - expected).abs() <= 1.0);
    }

    #[test]
    fn an_image_that_cannot_be_read_counts_at_its_own_size() {
        let raw = estimate_inputs();
        let junk = b"not an image".to_vec();
        let inputs = vec![
            EstimateInput {
                media_type: "image/jpeg",
                bytes: &raw[0],
            },
            EstimateInput {
                media_type: "image/png",
                bytes: &junk,
            },
        ];
        let est = estimate_format(
            &inputs,
            ImageFormat::WebP,
            80,
            std::time::Duration::from_secs(60),
        );
        assert!(est.is_exact());
        assert_eq!((est.images_measured, est.images_failed), (1, 1));
        assert!(est.converted_bytes >= junk.len() as u64);
    }
}
