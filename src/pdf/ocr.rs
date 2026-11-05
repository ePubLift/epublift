//! [`pdf-ocr` feature] The OCR models for scanned pages with no text layer.
//!
//! Recognition itself is paginaverto's (ocrs finds the lines, on the rten
//! tensor engine, pure Rust); it takes the model bytes and never touches the
//! network. Fetching them is ours: ocrs's `text-detection.onnx` (2.5 MB) and
//! PaddlePaddle's PP-OCRv5 Latin recognition model (8.0 MB, Apache-2.0), which
//! reads the Latin-script alphabets, Turkish among them. They download on first
//! use into a per-user cache dir (override with `EPUBLIFT_OCR_MODELS`), over
//! the same pure-Rust rustls client used for metadata enrichment, and every
//! load is checked against the SHA-256 paginaverto publishes. A cache from
//! before 2026-11 holds the detection model in `.rten` form, which ocrs no
//! longer serves (403); it keeps being used.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use paginaverto::{
    LATIN_RECOGNITION, LATIN_RECOGNITION_URL, MODEL_BASE_URL, MODELS, ModelFile, ONNX_MODELS, Ocr,
};

/// Where the OCR models are cached.
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

/// The detection and recognition models, downloading them on first use (cached
/// in [`models_dir`]). A file whose SHA-256 is not the published one is
/// refused, whether downloaded now or found in the cache.
pub(crate) fn models() -> Result<Ocr> {
    let dir = models_dir();
    // An older cache's `.rten` detection model carries the same weights.
    let detection = if dir.join(MODELS[0].name).exists() {
        MODELS[0]
    } else {
        ONNX_MODELS[0]
    };
    let files = [
        (detection, format!("{MODEL_BASE_URL}/{}", detection.name)),
        (LATIN_RECOGNITION, LATIN_RECOGNITION_URL.to_string()),
    ];
    if files.iter().any(|(m, _)| !dir.join(m.name).exists()) {
        download_models(&dir, &files).context("could not download the OCR models")?;
    }
    Ok(Ocr::Models {
        detection: load_checked(&dir, &files[0].0)?,
        recognition: load_checked(&dir, &files[1].0)?,
    })
}

/// Read one model file and check it against its published SHA-256.
fn load_checked(dir: &Path, model: &ModelFile) -> Result<Vec<u8>> {
    let path = dir.join(model.name);
    let bytes = std::fs::read(&path).with_context(|| format!("load {}", path.display()))?;
    let actual = sha256_hex(&bytes);
    if actual != model.sha256 {
        bail!(
            "OCR model {} has SHA-256 {actual}, expected {}; delete it to download it again",
            path.display(),
            model.sha256
        );
    }
    Ok(bytes)
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Fetch the OCR models (10.5 MB together) into `dir` over the pure-Rust HTTPS
/// client.
fn download_models(dir: &Path, files: &[(ModelFile, String)]) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating model cache {}", dir.display()))?;
    let http = crate::http::RustlsHttp::new()?;
    for (ModelFile { name, .. }, url) in files {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        eprintln!("Downloading OCR model {name} (one-time, 10.5 MB for both)…");
        let bytes = http
            .get_bytes(url)
            .with_context(|| format!("downloading {name}"))?;
        // Write to a temp name then rename, so an interrupted download can't
        // leave a half-written model that looks valid.
        let tmp = dir.join(format!("{name}.part"));
        std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}
