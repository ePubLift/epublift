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

/// Import a PDF and return (summary, all `<p>` text, figure-image count).
fn import_text(pdf: &Path) -> (pdf::ImportSummary, String, usize) {
    let out = std::env::temp_dir().join(format!(
        "epublift_it_{}_{}.epub",
        std::process::id(),
        pdf.file_stem().unwrap().to_string_lossy()
    ));
    let opts = ImportOptions {
        mode: Mode::Reflow,
        language: Some("en".into()),
        max_ocr_pages: None,
        keep_page_numbers: false,
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
    let mut images = 0usize;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().to_string();
        // Figures only: page 1's picture, when it is a cover, is the book's
        // cover, not a figure.
        if name.starts_with("OEBPS/images/") && !name.starts_with("OEBPS/images/cover.") {
            images += 1;
        }
        if name.ends_with(".xhtml") {
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
    (summary, text, images)
}

#[test]
fn holmes_reflow_quality() {
    let pdf = sample("adventuresofsher00doyl.pdf");
    if !pdf.exists() {
        eprintln!("skip holmes_reflow_quality: {} not present", pdf.display());
        return;
    }
    let (summary, text, images) = import_text(&pdf);
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
    // Holmes is a searchable scan whose figures are JPEG2000 — those are
    // skipped (not an EPUB core type), so no figures are embedded.
    assert_eq!(images, 0, "unexpected images from a JPEG2000 scan");
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
    let (_summary, text, images) = import_text(&pdf);
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
    // A born-digital book: its JPEG figures (cover + interior diagrams) are
    // carried into the EPUB.
    assert!(images >= 30, "figures not extracted: {images}");
    assert!(
        !text.contains("autom ated") && !text.contains("bestI"),
        "spacing artifacts returned"
    );
}

/// A scan longer than `max_ocr_pages` is refused before OCR starts (so before
/// the models are loaded or downloaded), with the page count.
#[cfg(feature = "pdf-ocr")]
#[test]
fn a_scan_over_the_page_limit_is_refused_before_ocr() {
    let pdf = sample("clean_prose_scan.pdf");
    if !pdf.exists() {
        eprintln!("skip: {} not present", pdf.display());
        return;
    }
    let out = std::env::temp_dir().join(format!("epublift_it_limit_{}.epub", std::process::id()));
    let opts = ImportOptions {
        mode: Mode::Reflow,
        language: Some("en".into()),
        max_ocr_pages: Some(1),
        keep_page_numbers: false,
    };
    let err = pdf::import(&pdf, &out, &opts).expect_err("a 2-page scan passed a 1-page limit");
    assert_eq!(
        err.downcast_ref::<pdf::ScanTooLong>(),
        Some(&pdf::ScanTooLong { pages: 2, max: 1 })
    );
    assert!(!out.exists());
}

/// A born-digital PDF of 14 pages of prose, a footer "X n X" on each, made
/// here so that nothing copyrighted is needed. Page 7 stops mid-sentence and
/// page 8 goes on in lower case, so paginaverto glues page 7's footer to the
/// front of page 8's first paragraph.
fn pdf_with_footers() -> Vec<u8> {
    let words = [
        "the", "quick", "brown", "fox", "jumps", "over", "a", "lazy", "dog", "while", "the",
        "river", "runs", "slowly", "past", "the", "old", "mill",
    ];
    let mut seed = 7u32;
    let mut line = |n: usize| -> String {
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                words[(seed >> 16) as usize % words.len()]
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    fn add(objects: &mut Vec<String>, o: String) -> usize {
        objects.push(o);
        objects.len()
    }
    let mut objects: Vec<String> = Vec::new();
    let font = add(
        &mut objects,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    );
    let mut contents = Vec::new();
    for page in 1..=14u32 {
        let mut lines: Vec<String> = Vec::new();
        if page == 1 {
            lines.push("CHAPTER ONE".into());
        }
        lines.push(if page == 8 {
            format!("middle of a sentence that goes on {}.", line(6))
        } else {
            format!("A {}.", line(10))
        });
        for _ in 0..18 {
            lines.push(format!("{}.", line(11)));
        }
        if page == 7 {
            *lines.last_mut().unwrap() = "This last sentence of the page stops in the".into();
        }
        let mut ops = String::from("BT /F1 11 Tf 72 740 Td 14 TL\n");
        for l in &lines {
            ops.push_str(&format!("({l}) Tj T*\n"));
        }
        ops.push_str(&format!("ET BT /F1 10 Tf 290 40 Td (X {page} X) Tj ET"));
        contents.push(add(
            &mut objects,
            format!("<< /Length {} >>\nstream\n{ops}\nendstream", ops.len()),
        ));
    }
    let pages_id = objects.len() + contents.len() + 1;
    let kids: Vec<String> = contents
        .iter()
        .map(|c| {
            let id = add(
                &mut objects,
                format!(
                    "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 612 792] \
                     /Resources << /Font << /F1 {font} 0 R >> >> /Contents {c} 0 R >>"
                ),
            );
            format!("{id} 0 R")
        })
        .collect();
    let pages = add(
        &mut objects,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            kids.len()
        ),
    );
    assert_eq!(pages, pages_id);
    let catalog = add(
        &mut objects,
        format!("<< /Type /Catalog /Pages {pages_id} 0 R >>"),
    );
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {catalog} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A page number dressed as "X n X" is neither a bare numeral nor a running
/// head to paginaverto: it comes through on every page, once glued to the
/// paragraph that continues over the page. `import` takes them all out and
/// joins that paragraph again; `keep_page_numbers` leaves them in.
#[test]
fn decorated_page_numbers_are_taken_out_unless_kept() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("footers.pdf");
    std::fs::write(&pdf, pdf_with_footers()).unwrap();

    let (summary, text, _) = import_text(&pdf);
    assert_eq!(summary.page_numbers_removed, 14);
    assert!(!text.contains("X 7 X"), "{text}");
    assert!(
        text.contains("stops in the middle of a sentence that goes on"),
        "{text}"
    );

    let out = dir.path().join("kept.epub");
    let opts = ImportOptions {
        mode: Mode::Reflow,
        language: Some("en".into()),
        max_ocr_pages: None,
        keep_page_numbers: true,
    };
    let kept = pdf::import(&pdf, &out, &opts).unwrap();
    assert_eq!(kept.page_numbers_removed, 0);
    assert_eq!(kept.paragraphs, summary.paragraphs + 14);
}
