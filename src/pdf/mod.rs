// epublift - Optimize EPUB files: convert images to WebP and upgrade to EPUB 3.3.
// Copyright (C) 2024  Baris Kayadelen
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! [EXPERIMENTAL] PDF → EPUB import (`epublift import`).
//!
//! Turns a PDF into a reflowable EPUB. Real-world PDFs fall into three tiers:
//!   1. born-digital (has a text layer) → extract text directly,
//!   2. scan WITH an embedded OCR text layer (archive.org / Google Books /
//!      "searchable PDFs") → reuse that text layer,
//!   3. pure scan, no text layer → OCR (the `pdf-ocr` feature, a later phase).
//!
//! v1 (`pdf` feature) covers tiers 1 and 2: lossless, pure-Rust, light. Tier 3
//! needs `pdf-ocr`.
//!
//! Pipeline: [`extract`] (classify input; pull text with font size + position,
//! and page images) → [`structure`] (strip running heads by position,
//! de-hyphenate, detect headings/chapters by font size) → [`reflow`] (emit the
//! EPUB, reusing the crate's `opf`/`nav`/`images` writers).

use anyhow::{Context, Result, bail};
use std::path::Path;

mod extract;
#[cfg(feature = "pdf-ocr")]
mod ocr;
mod reflow;
mod structure;

/// Output layout mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Reflowable text (default): the real ebook — resizable, searchable.
    #[default]
    Reflow,
    /// Fixed-layout: preserve the page images (picture books, comics).
    Fixed,
}

/// Options controlling a PDF import.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub mode: Mode,
    /// Content language (BCP-47, e.g. "tr"). Drives de-hyphenation/metadata;
    /// becomes mandatory once OCR (`pdf-ocr`) lands.
    pub language: Option<String>,
}

/// What an import produced (for the CLI/web to report).
#[derive(Debug, Clone, Copy)]
pub struct ImportSummary {
    pub chapters: usize,
    pub paragraphs: usize,
}

/// Import the PDF at `input`, writing a reflow EPUB to `output`.
pub fn import(input: &Path, output: &Path, opts: &ImportOptions) -> Result<ImportSummary> {
    if opts.mode == Mode::Fixed {
        bail!("[EXPERIMENTAL] --mode fixed (preserve page images) is not implemented yet");
    }

    let doc = lopdf::Document::load(input)
        .with_context(|| format!("failed to open PDF {}", input.display()))?;

    // v1 (`pdf` feature) handles inputs that carry a text layer. Pure scans need
    // OCR, which is the `pdf-ocr` feature (a later phase).
    if extract::classify(&doc) == extract::InputKind::Scan {
        bail!(
            "this PDF looks like a scan with no text layer — OCR is needed, \
             which is not available yet (coming in a later release)"
        );
    }

    let pages: Vec<_> = doc
        .get_pages()
        .into_iter()
        .map(|(num, id)| extract::page_text(&doc, id, num))
        .collect();

    let chapters = structure::build_book(&pages);
    if chapters.is_empty() {
        // A text layer was detected (classify said so) but nothing decoded →
        // almost certainly CID/Type0 fonts, which v1 can't read yet.
        if pages.iter().all(|p| p.blocks.is_empty()) {
            bail!(
                "this PDF has a text layer, but its fonts can't be decoded yet \
                 (CID/Type0 — common in modern, non-Latin PDFs); support is coming \
                 in a later release"
            );
        }
        bail!("no extractable text found in {}", input.display());
    }

    // Quality gate: real prose is ~80% letters; if the decoded text is mostly
    // non-letters (control/replacement bytes), the fonts didn't decode (e.g. a
    // PDF 1.5 object stream lopdf can't resolve, or an unsupported encoding) —
    // refuse rather than emit a garbage (and invalid-XML) EPUB.
    let (mut nonspace, mut letters) = (0usize, 0usize);
    for ch in chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .flat_map(|p| p.chars())
    {
        if !ch.is_whitespace() {
            nonspace += 1;
            if ch.is_alphabetic() {
                letters += 1;
            }
        }
    }
    if nonspace >= 50 && (letters as f32) < nonspace as f32 * 0.45 {
        bail!(
            "couldn't decode this PDF's text — it likely uses fonts or a PDF \
             structure we can't read yet (e.g. object streams); support is \
             coming in a later release"
        );
    }

    let title = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported book".into());
    let language = opts.language.as_deref().unwrap_or("en");

    reflow::write_epub(output, &title, language, &chapters)?;

    Ok(ImportSummary {
        chapters: chapters.len(),
        paragraphs: chapters.iter().map(|c| c.paragraphs.len()).sum(),
    })
}
