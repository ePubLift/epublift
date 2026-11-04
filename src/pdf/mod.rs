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
//! Turns a PDF into a reflowable EPUB. Reading the PDF is paginaverto's job (the
//! veripublica family's PDF reader): it classifies the input (born-digital, scan
//! with an OCR text layer, pure scan), extracts or recognises the text, and
//! structures it into chapters. We keep what makes it an EPUB: the title, the
//! language, [`reflow`] (XHTML + packaging), and — with `pdf-ocr` — the OCR
//! model download ([`ocr`]).

use anyhow::{Result, bail};
use std::path::Path;

#[cfg(feature = "pdf-ocr")]
mod ocr;
mod reflow;

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
    /// Content language (BCP-47, e.g. "tr"). Sets `dc:language`; required for
    /// scanned PDFs (OCR).
    pub language: Option<String>,
}

/// What an import produced (for the CLI/web to report).
#[derive(Debug, Clone, Copy)]
pub struct ImportSummary {
    pub chapters: usize,
    pub paragraphs: usize,
    /// The text came from OCR, not from a text layer.
    pub used_ocr: bool,
}

/// Whether OCR loses letters of a book in `language`. The engine's alphabet is
/// ASCII, so it can only guess at ş, ğ, ı, é or ü, and at other scripts. English
/// is the one language that needs nothing more.
pub fn ocr_loses_letters(language: &str) -> bool {
    let primary = language.split(['-', '_']).next().unwrap_or_default();
    !primary.eq_ignore_ascii_case("en")
}

/// Read the PDF into chapters. Without OCR first: only a scan needs the ~12 MB
/// models, so a text-layer PDF never loads (or downloads) them. The outer
/// error is ours (the models); the inner one is paginaverto's. The flag says
/// whether OCR ran.
fn read(
    input: &Path,
    opts: &ImportOptions,
) -> Result<(Result<paginaverto::Book, paginaverto::Error>, bool)> {
    let mut read_opts = paginaverto::Options::default();
    read_opts.language = opts.language.clone();
    let result = paginaverto::read(paginaverto::Input::Path(input), &read_opts);
    #[cfg(feature = "pdf-ocr")]
    if matches!(result, Err(paginaverto::Error::ScanNeedsOcr)) {
        if opts.language.as_deref().is_none_or(str::is_empty) {
            return Ok((Err(paginaverto::Error::LanguageRequired), false));
        }
        read_opts.ocr = ocr::models()?;
        return Ok((
            paginaverto::read(paginaverto::Input::Path(input), &read_opts),
            true,
        ));
    }
    Ok((result, false))
}

/// Import the PDF at `input`, writing a reflow EPUB to `output`.
pub fn import(input: &Path, output: &Path, opts: &ImportOptions) -> Result<ImportSummary> {
    if opts.mode == Mode::Fixed {
        bail!("[EXPERIMENTAL] --mode fixed (preserve page images) is not implemented yet");
    }

    let (book, used_ocr) = read(input, opts)?;
    let book = book.map_err(|e| match e {
        paginaverto::Error::Open(cause) => {
            anyhow::anyhow!(cause).context(format!("failed to open PDF {}", input.display()))
        }
        paginaverto::Error::NoText => {
            anyhow::anyhow!("no extractable text found in {}", input.display())
        }
        e => e.into(),
    })?;

    let title = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported book".into());
    let language = opts.language.as_deref().unwrap_or("en");

    reflow::write_epub(output, &title, language, &book.chapters)?;

    Ok(ImportSummary {
        chapters: book.chapters.len(),
        paragraphs: book
            .chapters
            .iter()
            .flat_map(|c| &c.blocks)
            .filter(|b| matches!(b, paginaverto::Block::Paragraph(_)))
            .count(),
        used_ocr,
    })
}

#[cfg(test)]
mod tests {
    use super::ocr_loses_letters;

    #[test]
    fn only_english_keeps_every_letter_through_ocr() {
        for language in ["en", "EN", "en-GB", "en_US"] {
            assert!(!ocr_loses_letters(language), "{language}");
        }
        for language in ["tr", "tr-TR", "de", "fr", "ru", "eng", ""] {
            assert!(ocr_loses_letters(language), "{language}");
        }
    }
}
