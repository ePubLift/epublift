//! [`pdf-ocr` feature] OCR for scanned pages with no text layer (Tier 3).
//!
//! TODO(v2): a later phase. Run pure-Rust OCR (ocrs + rten) on extracted page
//! images, with flat-field illumination preprocessing first (the spike showed
//! real-world scans are phone photos with shadow gradients; preprocessing is
//! necessary but not sufficient — recognition is weak on degraded/historical
//! print). Models (~12 MB: text-detection + text-recognition `.rten`) download
//! on first use and cache. Language becomes mandatory here.
