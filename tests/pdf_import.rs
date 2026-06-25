//! [EXPERIMENTAL] End-to-end PDF → EPUB import tests against real sample PDFs.
//!
//! The sample PDFs are copyrighted, so they are gitignored (`tests/*.pdf`) and
//! NOT present in CI. Each test therefore **skips gracefully** when its sample
//! is missing — so CI stays green — but runs for real on a dev machine that has
//! the files (`cargo test --features pdf`). They lock in the shipped behaviour:
//! born-digital prose quality + de-hyphenation, and CID/Type0 decoding + glyph-
//! width word spacing.
#![cfg(feature = "pdf")]

use std::io::Read;
use std::path::{Path, PathBuf};

use epublift::pdf::{self, ImportOptions, Mode};

fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}

/// Import a PDF and return (summary, all `<p>` text of the produced EPUB).
fn import_text(pdf: &Path) -> (pdf::ImportSummary, String) {
    let out = std::env::temp_dir().join(format!(
        "epublift_it_{}_{}.epub",
        std::process::id(),
        pdf.file_stem().unwrap().to_string_lossy()
    ));
    let opts = ImportOptions {
        mode: Mode::Reflow,
        language: Some("en".into()),
    };
    let summary = pdf::import(pdf, &out, &opts).expect("import failed");

    let bytes = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    // OCF requires the first entry to be an uncompressed `mimetype`.
    assert!(
        bytes.windows(20).any(|w| w == b"application/epub+zip"),
        "not a valid EPUB"
    );

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        if f.name().ends_with(".xhtml") {
            let mut s = String::new();
            f.read_to_string(&mut s).ok();
            for part in s.split("<p>").skip(1) {
                if let Some(end) = part.find("</p>") {
                    text.push_str(&part[..end]);
                    text.push(' ');
                }
            }
        }
    }
    (summary, text)
}

#[test]
fn holmes_reflow_quality() {
    let pdf = sample("adventuresofsher00doyl.pdf");
    if !pdf.exists() {
        eprintln!("skip holmes_reflow_quality: {} not present", pdf.display());
        return;
    }
    let (summary, text) = import_text(&pdf);
    assert!(
        summary.chapters >= 10,
        "too few chapters: {}",
        summary.chapters
    );
    assert!(
        summary.paragraphs > 1000,
        "too few paragraphs: {}",
        summary.paragraphs
    );
    // Distinctive clean prose from "A Scandal in Bohemia".
    assert!(
        text.contains("a broad-brimmed hat"),
        "expected prose missing"
    );
    // De-hyphenation: the joined word, never the split form.
    assert!(
        !text.contains("in- creased") && !text.contains("in-  creased"),
        "de-hyphenation regressed"
    );
}

#[test]
fn project_hail_mary_cid_spacing() {
    let pdf = sample("Project_Hail_Mary(Andy_Weir).pdf");
    if !pdf.exists() {
        eprintln!(
            "skip project_hail_mary_cid_spacing: {} not present",
            pdf.display()
        );
        return;
    }
    let (_summary, text) = import_text(&pdf);
    // CID/Type0 font decoded with correct, glyph-width-based word spacing
    // (the earlier "autom ated" / "bestI" artifacts must stay gone).
    assert!(
        text.contains("fully automated"),
        "CID decoding/spacing regressed"
    );
    assert!(
        text.contains("complex problems"),
        "CID decoding/spacing regressed"
    );
    assert!(
        !text.contains("autom ated") && !text.contains("bestI"),
        "spacing artifacts returned"
    );
}
