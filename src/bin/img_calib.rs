//! Image-codec calibration bench (dev-only, `--features img-calib`).
//!
//! Compares WebP / AVIF / JPEG XL at **equal perceptual quality** on a directory
//! of images, so codec sizes can be read off at the *same* butteraugli distance
//! (the metric JPEG XL's "distance" targets; <1.0 = visually identical).
//!
//! Usage:
//!   cargo run --release --features img-calib --bin img-calib -- <DIR> [SAMPLE] [AVIF_SPEED]
//!
//! For each format we sweep its quality knob over a small grid, and for every
//! sampled image: encode → decode → butteraugli vs the original, plus the encoded
//! byte size. We then anchor on WebP q80's mean butteraugli and interpolate each
//! other format's grid to that score, reporting size at equal quality.

use anyhow::{Context, Result};
use image::RgbImage;
use rgb::{FromSlice, Rgb};
use std::path::PathBuf;

type Rgb8 = Rgb<u8>;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(
        args.get(1)
            .context("usage: img-calib <DIR> [SAMPLE] [AVIF_SPEED]")?,
    );
    let sample: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(16);
    let avif_speed: u8 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);

    // Collect + evenly sample raster images.
    let mut paths: Vec<PathBuf> = walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| {
            matches!(
                p.extension()
                    .and_then(|x| x.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .as_deref(),
                Some("png" | "jpg" | "jpeg")
            )
        })
        .collect();
    paths.sort();
    let step = (paths.len() / sample).max(1);
    let picked: Vec<PathBuf> = paths.iter().step_by(step).take(sample).cloned().collect();

    // Silence panic spam: zenavif's pure-Rust decoder panics on some inputs; we
    // catch those and skip the image rather than crash the run.
    std::panic::set_hook(Box::new(|_| {}));

    // Decode the sampled originals to RGB8 once (skip < 8x8: butteraugli minimum).
    let mut origs: Vec<RgbImage> = Vec::new();
    for p in &picked {
        if let Ok(img) = image::open(p) {
            let rgb = img.to_rgb8();
            if rgb.width() >= 8 && rgb.height() >= 8 {
                origs.push(rgb);
            }
        }
    }
    // Pre-screen: keep only images the AVIF decoder can round-trip without
    // panicking, so all three formats are compared on the same image set.
    let before = origs.len();
    origs.retain(|o| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let b = enc_avif(o, 60, avif_speed);
            matches!(decode_rgb8(&b), Some((_, w, h)) if w == o.width() as usize && h == o.height() as usize)
        }))
        .unwrap_or(false)
    });
    let dropped = before - origs.len();
    let total_src: u64 = picked
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    eprintln!(
        "Calibrating on {} images ({:.2} MB source), AVIF speed {} ({dropped} dropped: AVIF-decode panic)\n",
        origs.len(),
        total_src as f64 / 1048576.0,
        avif_speed
    );

    // WebP is the reference quality scale (`--quality N` = "WebP quality N").
    // AVIF/JXL grids are wide enough to bracket every WebP anchor's butteraugli.
    let webp_anchors = [45u8, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95];
    let avif_q = [25u8, 35, 45, 55, 65, 72, 80, 88, 94, 98];
    let jxl_d = [5.0f32, 4.0, 3.0, 2.4, 1.8, 1.4, 1.0, 0.7, 0.5];

    // Per-format curves: (butteraugli score, knob, total KB).
    let avif_curve: Vec<(f64, f64, f64)> = avif_q
        .iter()
        .map(|&q| {
            let (s, kb) = run_point(&origs, |im| enc_avif(im, q, avif_speed));
            (s, q as f64, kb)
        })
        .collect();
    let jxl_curve: Vec<(f64, f64, f64)> = jxl_d
        .iter()
        .map(|&d| {
            let (s, kb) = run_point(&origs, |im| enc_jxl(im, d));
            (s, d as f64, kb)
        })
        .collect();

    // Calibration table: for each WebP quality (the reference), the AVIF q and
    // JXL distance that hit the SAME butteraugli, with size deltas vs WebP.
    println!(
        "{:>6} {:>11} {:>9} | {:>16} | {:>16}",
        "webp_q", "butteraugli", "webp_KB", "avif (q / KB / Δ)", "jxl (d / KB / Δ)"
    );
    println!("{}", "-".repeat(70));
    for &wq in &webp_anchors {
        let (s, kb_w) = run_point(&origs, |im| enc_webp(im, wq));
        let avk = interp_xy(&avif_curve, s, true).unwrap_or(f64::NAN);
        let avkb = interp_xy(&avif_curve, s, false).unwrap_or(f64::NAN);
        let jxk = interp_xy(&jxl_curve, s, true).unwrap_or(f64::NAN);
        let jxkb = interp_xy(&jxl_curve, s, false).unwrap_or(f64::NAN);
        println!(
            "{wq:>6} {s:>11.4} {kb_w:>9.1} | q{avk:>4.0} {avkb:>6.1} {:>+5.0}% | d{jxk:>4.2} {jxkb:>6.1} {:>+5.0}%",
            100.0 * (avkb - kb_w) / kb_w,
            100.0 * (jxkb - kb_w) / kb_w
        );
    }
    Ok(())
}

/// Encode every original with `enc`, decode it back, and return
/// (mean butteraugli vs original, total encoded KB).
fn run_point(origs: &[RgbImage], enc: impl Fn(&RgbImage) -> Vec<u8>) -> (f64, f64) {
    let mut score_sum = 0.0;
    let mut bytes = 0u64;
    let mut n = 0;
    for orig in origs {
        // Encode → decode → butteraugli, all under catch_unwind so a decoder
        // panic skips just this image. Bytes are counted only when the score
        // succeeds, so size and quality always cover the same image set.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let encoded = enc(orig);
            let sz = encoded.len() as u64;
            let (dec, w, h) = decode_rgb8(&encoded)?;
            if w != orig.width() as usize || h != orig.height() as usize {
                return None;
            }
            let a = imgref::Img::new(orig.as_raw().as_rgb(), w, h);
            let b = imgref::Img::new(dec.as_slice(), w, h);
            let res =
                butteraugli::butteraugli(a, b, &butteraugli::ButteraugliParams::new()).ok()?;
            Some((res.pnorm(3.0)?, sz))
        }));
        if let Ok(Some((s, sz))) = r {
            score_sum += s;
            bytes += sz;
            n += 1;
        }
    }
    (
        if n > 0 {
            score_sum / n as f64
        } else {
            f64::NAN
        },
        bytes as f64 / 1024.0,
    )
}

/// Linear-interpolate, at `target` butteraugli, either the knob (`want_knob`) or
/// the total KB, from a (score, knob, kb) curve. Returns `None` if `target` is
/// outside the swept butteraugli range.
fn interp_xy(curve: &[(f64, f64, f64)], target: f64, want_knob: bool) -> Option<f64> {
    let mut pts: Vec<(f64, f64)> = curve
        .iter()
        .map(|&(s, k, kb)| (s, if want_knob { k } else { kb }))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for w in pts.windows(2) {
        let (s0, v0) = w[0];
        let (s1, v1) = w[1];
        if (s0..=s1).contains(&target) {
            let t = if (s1 - s0).abs() < 1e-9 {
                0.0
            } else {
                (target - s0) / (s1 - s0)
            };
            return Some(v0 + t * (v1 - v0));
        }
    }
    None
}

// ---- encoders (RGB only; all imazen zen* codecs, pure Rust) -----------------

fn enc_webp(img: &RgbImage, q: u8) -> Vec<u8> {
    let cfg = zenwebp::LossyConfig::new().with_quality(q as f32);
    let (w, h) = (img.width(), img.height());
    zenwebp::EncodeRequest::lossy(&cfg, img, zenwebp::PixelLayout::Rgb8, w, h)
        .encode()
        .expect("webp encode")
}

fn enc_avif(img: &RgbImage, q: u8, speed: u8) -> Vec<u8> {
    let cfg = zenavif::EncoderConfig::new().quality(q as f32).speed(speed);
    let stop = almost_enough::StopToken::new(zenavif::Unstoppable);
    let im = imgref::Img::new(
        img.as_raw().as_rgb(),
        img.width() as usize,
        img.height() as usize,
    );
    zenavif::encode_rgb8(im, &cfg, stop)
        .expect("avif encode")
        .avif_file
}

fn enc_jxl(img: &RgbImage, distance: f32) -> Vec<u8> {
    let cfg = zenjxl::LossyConfig::new(distance);
    let im = imgref::Img::new(
        img.as_raw().as_rgb(),
        img.width() as usize,
        img.height() as usize,
    );
    zenjxl::encode_rgb8(im, &cfg).expect("jxl encode")
}

// ---- decoders → RGB8 --------------------------------------------------------

/// Decode WebP/AVIF/JPEG XL bytes to an RGB8 pixel vec, sniffing the format.
fn decode_rgb8(bytes: &[u8]) -> Option<(Vec<Rgb8>, usize, usize)> {
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let (px, w, h) = zenwebp::pixel::decode::<Rgb8>(bytes).ok()?;
        return Some((px, w as usize, h as usize));
    }
    // AVIF: ...ftyp....avif ; JPEG XL: ff 0a (codestream) or 00 00 00 0c 4a 58 4c 20 (container)
    if bytes.len() > 12 && &bytes[4..8] == b"ftyp" {
        // zenavif's pure-Rust rav1d decoder (0.1.6) is unreliable on Apple
        // Silicon — it panics on some AVIFs and silently corrupts others. For
        // *measurement only* we decode via macOS `sips` (system AVIF support).
        // The shipped binary never decodes AVIF; this is dev-bench-only.
        return decode_avif_via_sips(bytes);
    }
    let pb = zenjxl::decode(bytes, None, &[]).ok()?.pixels;
    Some(pb_to_rgb8(&pb))
}

/// Decode AVIF for measurement via macOS `sips` (reliable system AVIF support),
/// sidestepping the buggy pure-Rust decoder. Dev-bench-only.
fn decode_avif_via_sips(bytes: &[u8]) -> Option<(Vec<Rgb8>, usize, usize)> {
    let dir = tempfile::tempdir().ok()?;
    let av = dir.path().join("i.avif");
    let png = dir.path().join("o.png");
    std::fs::write(&av, bytes).ok()?;
    let ok = std::process::Command::new("sips")
        .args(["-s", "format", "png"])
        .arg(&av)
        .arg("--out")
        .arg(&png)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?
        .success();
    if !ok {
        return None;
    }
    let img = image::open(&png).ok()?.to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    Some((img.into_raw().as_rgb().to_vec(), w, h))
}

fn pb_to_rgb8(pb: &zenavif::PixelBuffer) -> (Vec<Rgb8>, usize, usize) {
    let (w, h) = (pb.width() as usize, pb.height() as usize);
    // Use `.rows()` (each row is exactly `width` long) so stride padding never
    // causes an over-read — `.pixels()` can run past the buffer on the last row.
    if let Some(img) = pb.try_as_imgref::<Rgb8>() {
        let mut out = Vec::with_capacity(w * h);
        for row in img.rows() {
            out.extend_from_slice(row);
        }
        return (out, w, h);
    }
    if let Some(img) = pb.try_as_imgref::<rgb::Rgba<u8>>() {
        let mut out = Vec::with_capacity(w * h);
        for row in img.rows() {
            out.extend(row.iter().map(|p| Rgb8::new(p.r, p.g, p.b)));
        }
        return (out, w, h);
    }
    (Vec::new(), 0, 0)
}
