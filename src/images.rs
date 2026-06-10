//! Image optimization: convert raster manifest images (JPEG/PNG) to WebP and
//! update every reference to them across the package.

use crate::opf::{ImageChange, ManifestItem};
use crate::util::{basename, quote, unquote, with_webp_ext};
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;
use zenwebp::{EncodeRequest, LossyConfig, PixelLayout};

/// Per-image size statistics, used by the report.
#[derive(Debug, Clone)]
pub struct ImageMetric {
    pub name: String,
    pub original_size: u64,
    pub new_size: u64,
    pub percentage: f64,
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

        let new_href = with_webp_ext(&decoded_href);
        let new_img_path = package_dir.join(&new_href);

        let orig_size = match fs::metadata(&img_path) {
            Ok(m) => m.len(),
            Err(e) => {
                progress(&format!(
                    "  [!] Failed to convert image {}: {}",
                    basename(&decoded_href),
                    e
                ));
                continue;
            }
        };

        // Decode + re-encode to WebP at the requested quality.
        match encode_webp(&img_path, &new_img_path, quality) {
            Ok(()) => {}
            Err(e) => {
                progress(&format!(
                    "  [!] Failed to convert image {}: {}",
                    basename(&decoded_href),
                    e
                ));
                continue;
            }
        }

        let new_size = fs::metadata(&new_img_path).map(|m| m.len()).unwrap_or(0);
        let savings = orig_size as i64 - new_size as i64;
        let pct = if orig_size > 0 {
            savings as f64 / orig_size as f64 * 100.0
        } else {
            0.0
        };

        let old_name = basename(&decoded_href).to_string();
        let new_name = basename(&new_href).to_string();

        result.metrics.push(ImageMetric {
            name: old_name.clone(),
            original_size: orig_size,
            new_size,
            percentage: pct,
        });

        // Remove the original raster image.
        let _ = fs::remove_file(&img_path);

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

/// Decode an image from disk and write it back out as WebP at `quality` (1-100).
fn encode_webp(src: &Path, dst: &Path, quality: u8) -> Result<()> {
    let dynimg = image::open(src)?;
    let config = LossyConfig::new().with_quality(quality as f32);

    let memory = if dynimg.color().has_alpha() {
        let rgba = dynimg.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        EncodeRequest::lossy(&config, &rgba, PixelLayout::Rgba8, w, h).encode()?
    } else {
        let rgb = dynimg.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        EncodeRequest::lossy(&config, &rgb, PixelLayout::Rgb8, w, h).encode()?
    };

    fs::write(dst, &memory)?;
    Ok(())
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
