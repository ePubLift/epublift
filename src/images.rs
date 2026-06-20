//! Image optimization: convert raster manifest images (JPEG/PNG) to WebP and
//! update every reference to them across the package.

use crate::opf::{ImageChange, ManifestItem};
use crate::util::{basename, quote, unquote, with_ext};
use anyhow::Result;
use image::{ColorType, DynamicImage};
use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use walkdir::WalkDir;
use zenwebp::{EncodeRequest, LossyConfig, PixelLayout};

/// Output image format the optimizer re-encodes raster images to. WebP is the
/// EPUB 3.3 target; AVIF and JPEG XL become core media types in EPUB 3.4 and are
/// emitted under `--target 3.4` (experimental, behind the `epub34` feature).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFormat {
    #[default]
    WebP,
    Avif,
    Jxl,
}

impl ImageFormat {
    /// File extension (no dot) for this format's output files.
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::WebP => "webp",
            ImageFormat::Avif => "avif",
            ImageFormat::Jxl => "jxl",
        }
    }

    /// OPF manifest media type for this format.
    pub fn media_type(self) -> &'static str {
        match self {
            ImageFormat::WebP => "image/webp",
            ImageFormat::Avif => "image/avif",
            ImageFormat::Jxl => "image/jxl",
        }
    }

    /// Short label for progress/log lines.
    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::WebP => "WebP",
            ImageFormat::Avif => "AVIF",
            ImageFormat::Jxl => "JPEG XL",
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
    /// format, which is a free content-type signal — publishers use JPEG for
    /// photographs and PNG for line-art/screenshots. Measured at equal perceptual
    /// quality: AVIF wins on photos, WebP wins on line-art (see
    /// docs/design/epub-3.4-image-codec-choice.md). So **JPEG → AVIF, PNG → WebP**.
    Auto,
}

impl FormatPolicy {
    /// The output format for a source of the given media type.
    pub fn format_for(self, media_type: &str) -> ImageFormat {
        match self {
            FormatPolicy::Fixed(f) => f,
            FormatPolicy::Auto => match media_type {
                "image/jpeg" | "image/jpg" => ImageFormat::Avif,
                _ => ImageFormat::WebP,
            },
        }
    }

    /// Short label for the conversion progress header.
    pub fn label(self) -> &'static str {
        match self {
            FormatPolicy::Fixed(f) => f.label(),
            FormatPolicy::Auto => "AVIF for photos, WebP for line-art",
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
    /// True when the original was kept because the WebP came out no smaller.
    pub kept: bool,
}

/// Result of the optimization pass.
pub struct OptimizeResult {
    pub metrics: Vec<ImageMetric>,
    /// Keyed by the raw manifest `href` -> how the manifest item is rewritten.
    pub manifest_changes: HashMap<String, ImageChange>,
    /// (old_reference, new_reference) pairs for updating document/style files.
    pub ref_pairs: Vec<(String, String)>,
}

/// Raster media types we convert, all mapped to `image/webp`.
fn is_target_media_type(mt: &str) -> bool {
    matches!(mt, "image/jpeg" | "image/jpg" | "image/png")
}

/// Convert all eligible manifest images to WebP at the given quality, deleting
/// originals and recording the manifest/reference changes.
pub fn optimize_images(
    package_dir: &Path,
    items: &[ManifestItem],
    cover_id: Option<&str>,
    quality: u8,
    policy: FormatPolicy,
    progress: &dyn Fn(&str),
) -> Result<OptimizeResult> {
    let mut result = OptimizeResult {
        metrics: Vec::new(),
        manifest_changes: HashMap::new(),
        ref_pairs: Vec::new(),
    };

    for item in items {
        if !is_target_media_type(&item.media_type) {
            continue;
        }

        // Output format for this image — fixed, or chosen from the source type
        // (the 3.4 content-adaptive heuristic). The rest of the loop uses `format`.
        let format = policy.format_for(&item.media_type);

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

        let new_href = with_ext(&decoded_href, format.extension());
        let new_img_path = package_dir.join(&new_href);
        let old_name = basename(&decoded_href).to_string();

        // Read the original bytes once: we need its size, its raw bytes (to
        // estimate JPEG quality), and its decoded pixels.
        let orig_bytes = match fs::read(&img_path) {
            Ok(b) => b,
            Err(e) => {
                progress(&format!("  [!] Failed to read image {old_name}: {e}"));
                continue;
            }
        };
        let orig_size = orig_bytes.len() as u64;

        let dynimg = match decode_image(&orig_bytes) {
            Ok(d) => d,
            Err(e) => {
                progress(&format!("  [!] Failed to decode image {old_name}: {e}"));
                continue;
            }
        };

        // (2) Never re-encode *above* the source quality. A chart saved as a
        // q43 JPEG gains nothing from a q80 WebP — it only inflates. For JPEG
        // sources we cap the WebP quality at the source's estimated quality.
        let is_jpeg = matches!(item.media_type.as_str(), "image/jpeg" | "image/jpg");
        let eff_quality = if is_jpeg {
            estimate_jpeg_quality(&orig_bytes)
                .map(|q| quality.min(q))
                .unwrap_or(quality)
        } else {
            quality
        };

        // (3) Encode grayscale as grayscale so we never pay for empty colour
        // channels. Encode to memory first so we can compare sizes.
        let encoded = match encode_image(&dynimg, eff_quality, format) {
            Ok(b) => b,
            Err(e) => {
                progress(&format!("  [!] Failed to convert image {old_name}: {e}"));
                continue;
            }
        };
        let new_size = encoded.len() as u64;

        // (1) Keep whichever is smaller. If the re-encode isn't actually smaller,
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
            });
            continue;
        }

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

fn push_unique(pairs: &mut Vec<(String, String)>, old: String, new: String) {
    if !pairs.iter().any(|(o, _)| *o == old) {
        pairs.push((old, new));
    }
}

/// Bounds on a single image decode, to defend against decode-bombs (e.g. a tiny
/// PNG declaring 100k×100k). Comfortably above any real e-book image.
const MAX_IMAGE_DIM: u32 = 16_384;
const MAX_IMAGE_ALLOC: u64 = 512 * 1024 * 1024; // 512 MiB

/// Decode an in-memory image, enforcing the decode-bomb limits.
fn decode_image(bytes: &[u8]) -> Result<DynamicImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    Ok(reader.decode()?)
}

/// Encode a decoded image to the requested output `format` at `quality` (1-100).
/// AVIF / JPEG XL require the experimental `epub34` build feature.
fn encode_image(dynimg: &DynamicImage, quality: u8, format: ImageFormat) -> Result<Vec<u8>> {
    match format {
        ImageFormat::WebP => encode_webp(dynimg, quality),
        #[cfg(feature = "epub34")]
        ImageFormat::Avif => encode_avif(dynimg, quality),
        #[cfg(feature = "epub34")]
        ImageFormat::Jxl => encode_jxl(dynimg, quality),
        #[cfg(not(feature = "epub34"))]
        ImageFormat::Avif | ImageFormat::Jxl => anyhow::bail!(
            "{} output requires building epublift with the `epub34` feature",
            format.label()
        ),
    }
}

/// Encode a decoded image to WebP bytes at `quality` (1-100), choosing the
/// pixel layout that matches the image: grayscale images stay grayscale (L8 /
/// La8) so we never spend bits on empty colour channels.
fn encode_webp(dynimg: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let config = LossyConfig::new().with_quality(quality as f32);
    let gray = matches!(
        dynimg.color(),
        ColorType::L8 | ColorType::La8 | ColorType::L16 | ColorType::La16
    );
    let alpha = dynimg.color().has_alpha();

    let memory = match (gray, alpha) {
        (true, false) => {
            let g = dynimg.to_luma8();
            let (w, h) = (g.width(), g.height());
            EncodeRequest::lossy(&config, &g, PixelLayout::L8, w, h).encode()?
        }
        (true, true) => {
            let g = dynimg.to_luma_alpha8();
            let (w, h) = (g.width(), g.height());
            EncodeRequest::lossy(&config, &g, PixelLayout::La8, w, h).encode()?
        }
        (false, true) => {
            let rgba = dynimg.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            EncodeRequest::lossy(&config, &rgba, PixelLayout::Rgba8, w, h).encode()?
        }
        (false, false) => {
            let rgb = dynimg.to_rgb8();
            let (w, h) = (rgb.width(), rgb.height());
            EncodeRequest::lossy(&config, &rgb, PixelLayout::Rgb8, w, h).encode()?
        }
    };
    Ok(memory)
}

/// Map a WebP-scale `--quality` (1-100, the reference) to the AVIF quality knob
/// that yields the *same* perceptual quality (butteraugli) — so `--quality N`
/// means the same thing across codecs. First-pass linear calibration on a
/// photographic (JPEG-source) book; AVIF reaches WebP's quality at a lower knob,
/// and its size advantage grows with quality. See
/// docs/design/epub-3.4-image-codec-choice.md.
#[cfg(feature = "epub34")]
fn calibrated_avif_quality(webp_quality: u8) -> f32 {
    (0.64 * webp_quality as f32 + 17.0).clamp(1.0, 100.0)
}

/// Map a WebP-scale `--quality` (1-100) to the JPEG XL butteraugli *distance*
/// that matches WebP's perceptual quality (lower distance = higher quality).
/// First-pass linear calibration on photographic content (see above).
#[cfg(feature = "epub34")]
fn calibrated_jxl_distance(webp_quality: u8) -> f32 {
    (-0.064 * webp_quality as f32 + 7.4).clamp(0.4, 15.0)
}

/// Encode to AVIF via the pure-Rust imazen `zenavif` (rav1e) encoder, `quality`
/// 1-100 (calibrated to the WebP scale). Grayscale is expanded to RGB (zenavif
/// encodes RGB/RGBA only). Behind the experimental `epub34` feature.
#[cfg(feature = "epub34")]
fn encode_avif(dynimg: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    use rgb::FromSlice;
    let cfg = zenavif::EncoderConfig::new().quality(calibrated_avif_quality(quality));
    let stop = almost_enough::StopToken::new(zenavif::Unstoppable);
    let encoded = if dynimg.color().has_alpha() {
        let rgba = dynimg.to_rgba8();
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let img = imgref::Img::new(rgba.as_raw().as_rgba(), w, h);
        zenavif::encode_rgba8(img, &cfg, stop)?
    } else {
        let rgb = dynimg.to_rgb8();
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);
        let img = imgref::Img::new(rgb.as_raw().as_rgb(), w, h);
        zenavif::encode_rgb8(img, &cfg, stop)?
    };
    Ok(encoded.avif_file)
}

/// Encode to JPEG XL via the pure-Rust imazen `zenjxl` encoder, lossy at the
/// distance mapped from `quality` 1-100. Grayscale is expanded to RGB. Behind
/// the experimental `epub34` feature. See docs/epub-3.4.md.
#[cfg(feature = "epub34")]
fn encode_jxl(dynimg: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    use rgb::FromSlice;
    let cfg = zenjxl::LossyConfig::new(calibrated_jxl_distance(quality));
    let encoded = if dynimg.color().has_alpha() {
        let rgba = dynimg.to_rgba8();
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let img = imgref::Img::new(rgba.as_raw().as_rgba(), w, h);
        zenjxl::encode_rgba8(img, &cfg)?
    } else {
        let rgb = dynimg.to_rgb8();
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);
        let img = imgref::Img::new(rgb.as_raw().as_rgb(), w, h);
        zenjxl::encode_rgb8(img, &cfg)?
    };
    Ok(encoded)
}

/// The standard JPEG Annex-K luminance quantization table (quality 50 baseline).
#[rustfmt::skip]
const STD_LUMA_QT: [u16; 64] = [
    16, 11, 10, 16,  24,  40,  51,  61,
    12, 12, 14, 19,  26,  58,  60,  55,
    14, 13, 16, 24,  40,  57,  69,  56,
    14, 17, 22, 29,  51,  87,  80,  62,
    18, 22, 37, 56,  68, 109, 103,  77,
    24, 35, 55, 64,  81, 104, 113,  92,
    49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103,  99,
];

/// Estimate the quality factor (1-100) a JPEG was saved at, by comparing its
/// luminance quantization table against the standard one. Returns `None` if the
/// JPEG can't be parsed. This is a heuristic — the IJG/libjpeg quality scale —
/// good enough to avoid re-encoding a low-quality source at a higher quality.
fn estimate_jpeg_quality(data: &[u8]) -> Option<u8> {
    let mut i = 2; // skip the SOI marker
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // Standalone markers (SOI/EOI/RSTn) carry no length payload.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let end = i + 2 + seg_len;
        if seg_len < 2 || end > data.len() {
            break;
        }
        if marker == 0xDB {
            // DQT: one or more tables, each prefixed by a precision/id byte.
            let mut p = i + 4;
            while p < end {
                let precision = data[p] >> 4; // 0 = 8-bit, 1 = 16-bit
                let table_id = data[p] & 0x0F;
                p += 1;
                let n = if precision == 1 { 128 } else { 64 };
                if p + n > end {
                    break;
                }
                if precision == 0 && table_id == 0 {
                    let table = &data[p..p + 64];
                    let mut sum = 0.0f64;
                    let mut count = 0;
                    for (k, &q) in table.iter().enumerate() {
                        let q = q as u16;
                        if q == 0 || q >= 255 {
                            continue; // saturated entries carry no quality signal
                        }
                        sum += q as f64 * 100.0 / STD_LUMA_QT[k] as f64;
                        count += 1;
                    }
                    if count == 0 {
                        return None;
                    }
                    let s = sum / count as f64;
                    let quality = if s < 100.0 {
                        (200.0 - s) / 2.0
                    } else {
                        5000.0 / s
                    };
                    return Some(quality.round().clamp(1.0, 100.0) as u8);
                }
                p += n;
            }
        }
        if marker == 0xDA {
            break; // start of scan — no more headers
        }
        i = end;
    }
    None
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

    #[test]
    fn estimates_jpeg_quality_within_tolerance() {
        // The estimate is a heuristic (encoders differ slightly from the IJG
        // scale), so allow some slack. What matters for capping is that it's in
        // the right neighbourhood and ordered by quality.
        let mut prev = 0u8;
        for q in [40u8, 60, 75, 90] {
            let est = estimate_jpeg_quality(&jpeg_at(q)).expect("should parse a real JPEG");
            assert!(
                (est as i32 - q as i32).abs() <= 10,
                "quality {q} estimated as {est}"
            );
            assert!(
                est >= prev,
                "estimate should rise with quality (q{q} -> {est})"
            );
            prev = est;
        }
    }

    #[test]
    fn estimate_returns_none_for_non_jpeg() {
        assert!(estimate_jpeg_quality(b"this is not a jpeg at all").is_none());
    }
}
