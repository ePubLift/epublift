//! [`pdf-ocr` feature] OCR for scanned pages with no text layer (Tier 3).
//!
//! Pure-Rust OCR via ocrs (on the rten tensor engine). Each scanned page image
//! is flat-field illumination-corrected (real-world scans are often phone
//! photos with shadow gradients), then run through ocrs detection+recognition.
//!
//! Models (`text-detection.rten` + `text-recognition.rten`, ~12 MB) download on
//! first use into a per-user cache dir (override with `EPUBLIFT_OCR_MODELS`),
//! over the same pure-Rust rustls client used for metadata enrichment.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;

/// Where the models are published (the official ocrs model bucket).
const MODEL_BASE_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com";
const MODEL_FILES: [&str; 2] = ["text-detection.rten", "text-recognition.rten"];

/// Where to find `text-detection.rten` + `text-recognition.rten`.
fn models_dir() -> PathBuf {
    if let Ok(d) = std::env::var("EPUBLIFT_OCR_MODELS") {
        return PathBuf::from(d);
    }
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cache"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("epublift").join("ocr-models")
}

/// Load the ocrs detection + recognition models into an engine, downloading
/// them on first use (cached in [`models_dir`]).
pub(crate) fn load_engine() -> Result<OcrEngine> {
    let dir = models_dir();
    let det = dir.join(MODEL_FILES[0]);
    let rec = dir.join(MODEL_FILES[1]);
    if !det.exists() || !rec.exists() {
        download_models(&dir).context("could not download the OCR models")?;
    }
    let detection = Model::load_file(&det).with_context(|| format!("load {}", det.display()))?;
    let recognition = Model::load_file(&rec).with_context(|| format!("load {}", rec.display()))?;
    OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection),
        recognition_model: Some(recognition),
        ..Default::default()
    })
    .map_err(|e| anyhow!("ocrs engine init failed: {e}"))
}

/// Fetch the OCR models (~12 MB) into `dir` over the pure-Rust HTTPS client.
fn download_models(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating model cache {}", dir.display()))?;
    let http = crate::http::RustlsHttp::new()?;
    for name in MODEL_FILES {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        eprintln!("Downloading OCR model {name} (one-time, ~12 MB total)…");
        let url = format!("{MODEL_BASE_URL}/{name}");
        let bytes = http
            .get_bytes(&url)
            .with_context(|| format!("downloading {name}"))?;
        // Write to a temp name then rename, so an interrupted download can't
        // leave a half-written model that looks valid.
        let tmp = path.with_extension("rten.part");
        std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

/// OCR one page image (any format the `image` crate decodes), returning its
/// recognised text.
pub(crate) fn ocr_image(engine: &OcrEngine, image_bytes: &[u8]) -> Result<String> {
    let img = image::load_from_memory(image_bytes)?.into_rgb8();
    let img = preprocess(&img);
    let (w, h) = img.dimensions();
    let source =
        ImageSource::from_bytes(img.as_raw(), (w, h)).map_err(|e| anyhow!("image source: {e}"))?;
    let input = engine
        .prepare_input(source)
        .map_err(|e| anyhow!("prepare input: {e}"))?;
    engine
        .get_text(&input)
        .map_err(|e| anyhow!("recognize: {e}"))
}

/// Flat-field illumination correction: estimate the page's lighting by heavily
/// downscaling (a blurred background), then divide it out so paper becomes
/// uniformly bright and ink stays dark. Necessary for phone-photo scans.
fn preprocess(img: &image::RgbImage) -> image::RgbImage {
    use image::imageops::{self, FilterType};
    let gray = image::DynamicImage::ImageRgb8(img.clone()).into_luma8();
    let (w, h) = gray.dimensions();
    let (sw, sh) = ((w / 24).max(1), (h / 24).max(1));
    let small = imageops::resize(&gray, sw, sh, FilterType::Gaussian);
    let bg = imageops::resize(&small, w, h, FilterType::Triangle);

    let mut out = image::RgbImage::new(w, h);
    for (x, y, px) in gray.enumerate_pixels() {
        let p = px[0] as f32;
        let b = (bg.get_pixel(x, y)[0] as f32).max(1.0);
        let v = (p / b * 245.0).min(255.0) as u8; // paper → ~white, ink kept dark
        out.put_pixel(x, y, image::Rgb([v, v, v]));
    }
    out
}
