//! Generation of the human-readable optimization audit report.

use crate::Report;
use crate::images::ImageMetric;
use anyhow::Result;
use chrono::Local;
use std::fs;
use std::path::Path;

/// Group an integer with comma thousands separators (Python `{:,}`).
fn comma_i(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let bytes = digits.as_bytes();
    let len = bytes.len();
    let mut out = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    if neg { format!("-{}", out) } else { out }
}

/// Format a float with comma grouping and one decimal place (Python `{:,.1f}`).
fn comma_f1(x: f64) -> String {
    let neg = x < 0.0;
    let v = (x.abs() * 10.0).round() / 10.0;
    let int_part = v.trunc() as i64;
    let dec = ((v - int_part as f64) * 10.0).round() as i64;
    let s = format!("{}.{}", comma_i(int_part), dec);
    if neg { format!("-{}", s) } else { s }
}

fn mb(bytes: i64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

/// The distinct output formats across the converted images, as a label like
/// `"AVIF"` or `"AVIF/WebP"` (content-adaptive runs can mix). Falls back to
/// `"WebP"` when nothing was converted.
fn format_label(metrics: &[ImageMetric]) -> String {
    let mut seen: Vec<&'static str> = Vec::new();
    for m in metrics {
        if let Some(f) = m.format {
            let l = f.label();
            if !seen.contains(&l) {
                seen.push(l);
            }
        }
    }
    if seen.is_empty() {
        "WebP".to_string()
    } else {
        seen.join("/")
    }
}

/// Write the human-readable audit report for `report` to `report_path`.
pub fn write_report(report_path: &Path, report: &Report) -> Result<()> {
    let input_name = &report.input_name;
    let output_name = &report.output_name;
    let metrics = &report.image_metrics;
    let version_tag = report.target_version.tag();
    let outdated_features = &report.outdated_features;

    let original = report.original_size as i64;
    let final_s = report.final_size as i64;
    let saved = original - final_s;
    let pct = if original > 0 {
        saved as f64 / original as f64 * 100.0
    } else {
        0.0
    };

    let sep = "=".repeat(60);
    let dash = "-".repeat(60);

    let mut r: Vec<String> = Vec::new();
    r.push(sep.clone());
    r.push("                EPUBLIFT OPTIMIZATION REPORT".to_string());
    r.push(sep.clone());
    r.push(format!(
        "Timestamp: {}",
        Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    r.push(format!("Original File: {}", input_name));
    r.push(format!("Optimized File: {}", output_name));
    r.push(dash.clone());
    r.push("FILE SIZE COMPARISON".to_string());
    r.push(dash.clone());
    r.push(format!(
        "Original EPUB Size:  {:>14} bytes ({:.2} MB)",
        comma_i(original),
        mb(original)
    ));
    r.push(format!(
        "Lifted EPUB Size:    {:>14} bytes ({:.2} MB)",
        comma_i(final_s),
        mb(final_s)
    ));
    r.push(format!(
        "Absolute Size Saved:  {:>14} bytes ({:.2} MB)",
        comma_i(saved),
        mb(saved)
    ));
    r.push(format!("Percentage Saved:     {:>13.1}%", pct));
    r.push(dash.clone());
    r.push(format!("EPUB {version_tag} COMPLIANCE ACTIONS"));
    r.push(dash.clone());
    r.push("[x] Upgraded root <package> element to version='3.0'".to_string());
    r.push("[x] Added required 'dcterms:modified' UTC timestamp metadata".to_string());
    r.push(
        "[x] Parsed legacy toc.ncx and generated EPUB 3 Navigation Document (nav.xhtml)"
            .to_string(),
    );
    r.push("[x] Upgraded all content DOCTYPEs to modern HTML5 standards".to_string());
    r.push(
        "[x] Replaced legacy <guide> landmarks references with hidden <nav epub:type='landmarks'>"
            .to_string(),
    );
    r.push(dash.clone());
    r.push("IMAGE OPTIMIZATION BREAKDOWN".to_string());
    r.push(dash.clone());

    if metrics.is_empty() {
        r.push("No raster images (JPEG/PNG) were found or converted.".to_string());
    } else {
        let converted = metrics.iter().filter(|m| !m.kept).count();
        let kept = metrics.len() - converted;
        let fmt = format_label(metrics);
        r.push(format!(
            "{converted} image(s) re-encoded to {fmt}, {kept} kept as-is (re-encode was no smaller)."
        ));
        r.push(dash.clone());
        r.push(format!(
            "{:<30} | {:<13} | {:<9} | {:<10}",
            "Image Name", "Original (KB)", "Result (KB)", "Saved (%)"
        ));
        r.push(dash.clone());
        for m in metrics {
            let name: String = m.name.chars().take(29).collect();
            if m.kept {
                r.push(format!(
                    "{:<30} | {:>13} | {:>9} | {:>10}",
                    name,
                    comma_f1(m.original_size as f64 / 1024.0),
                    "—",
                    "kept"
                ));
            } else {
                r.push(format!(
                    "{:<30} | {:>13} | {:>9} | {:>9.1}%",
                    name,
                    comma_f1(m.original_size as f64 / 1024.0),
                    comma_f1(m.new_size as f64 / 1024.0),
                    m.percentage
                ));
            }
        }
    }

    if !outdated_features.is_empty() {
        r.push(dash.clone());
        r.push("OUTDATED / DEPRECATED FEATURES (EPUB 3.4)".to_string());
        r.push(dash.clone());
        r.push(
            "EPUB 3.4 marks these outdated/deprecated (or, for HTML syntax, removed).".to_string(),
        );
        r.push("Kept as-is; review them in the source for future-proofing.".to_string());
        for f in outdated_features {
            r.push(format!("[!] {f}"));
        }
    }

    r.push(sep);

    fs::write(report_path, r.join("\n"))?;
    Ok(())
}
