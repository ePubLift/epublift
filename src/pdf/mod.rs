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
#[derive(Debug, Clone)]
pub struct ImportSummary {
    pub chapters: usize,
    pub paragraphs: usize,
    /// The text came from OCR, not from a text layer.
    pub used_ocr: bool,
    /// Pages that could not be read, by problem: the problem's name
    /// (`ocr_failed`, `undecodable_images`, `image_not_read`) and its pages,
    /// numbered from 1. The rest of the book is written.
    pub unread_pages: Vec<(&'static str, Vec<u32>)>,
    /// OCR read the book, and its model lacks the letters of the book's
    /// language: the text is wrong or missing.
    pub ocr_misreads_language: bool,
}

/// Read the PDF. Without OCR first: only a scan needs the 10.5 MB of models, so
/// a text-layer PDF never loads (or downloads) them. The outer error is ours
/// (the models); the inner one is paginaverto's. The flag says whether OCR ran.
fn read(
    input: &Path,
    opts: &ImportOptions,
) -> Result<(Result<paginaverto::Document, paginaverto::Error>, bool)> {
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

/// Pages with a problem, grouped by the problem's name, in the order met.
fn unread_pages(doc: &paginaverto::Document) -> Vec<(&'static str, Vec<u32>)> {
    let mut out: Vec<(&'static str, Vec<u32>)> = Vec::new();
    for page in &doc.pages {
        for problem in &page.problems {
            let name = problem.name();
            match out.iter_mut().find(|(n, _)| *n == name) {
                Some((_, pages)) if pages.last() != Some(&page.number) => pages.push(page.number),
                Some(_) => {}
                None => out.push((name, vec![page.number])),
            }
        }
    }
    out
}

/// The title the PDF gives itself, its `/Info` `Title`, unless it is empty or
/// what a program left there: a file name, or "Microsoft Word - <file>".
fn info_title(doc: &paginaverto::Document) -> Option<String> {
    let title = doc.metadata.info.get("Title")?.trim();
    if is_placeholder_title(title) {
        return None;
    }
    Some(title.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn is_placeholder_title(title: &str) -> bool {
    const FILE_EXTENSIONS: &[&str] = &[
        ".doc", ".docx", ".odt", ".rtf", ".txt", ".pdf", ".indd", ".qxd", ".qxp", ".pages", ".tex",
        ".dvi", ".ps", ".ai", ".p65", ".pm6", ".fm", ".html", ".htm", ".xml",
    ];
    let lower = title.to_lowercase();
    !title.chars().any(char::is_alphabetic)
        || lower.starts_with("microsoft word - ")
        || lower.starts_with("untitled")
        || FILE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// Import the PDF at `input`, writing a reflow EPUB to `output`.
pub fn import(input: &Path, output: &Path, opts: &ImportOptions) -> Result<ImportSummary> {
    if opts.mode == Mode::Fixed {
        bail!("[EXPERIMENTAL] --mode fixed (preserve page images) is not implemented yet");
    }

    let (doc, used_ocr) = read(input, opts)?;
    let doc = doc.map_err(|e| match e {
        paginaverto::Error::Open(cause) => {
            anyhow::anyhow!(cause).context(format!("failed to open PDF {}", input.display()))
        }
        paginaverto::Error::NoText => {
            anyhow::anyhow!("no extractable text found in {}", input.display())
        }
        e => e.into(),
    })?;

    let title = info_title(&doc).unwrap_or_else(|| {
        input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Imported book".into())
    });
    // The option, else the PDF's own /Lang (paginaverto never guesses).
    let language = doc.language.as_deref().unwrap_or("en");

    let chapters = doc.chapters();
    reflow::write_epub(output, &title, language, &chapters)?;

    Ok(ImportSummary {
        chapters: chapters.len(),
        paragraphs: doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, paginaverto::BlockKind::Paragraph { .. }))
            .count(),
        used_ocr,
        unread_pages: unread_pages(&doc),
        ocr_misreads_language: used_ocr
            && opts
                .language
                .as_deref()
                .is_some_and(|l| !paginaverto::recognises(l)),
    })
}

#[cfg(test)]
mod tests {
    use super::is_placeholder_title;

    #[test]
    fn a_title_a_program_left_is_not_used() {
        for title in [
            "",
            "  ",
            "1234",
            "Microsoft Word - chapter1.docx",
            "book_final.indd",
            "scan.PDF",
            "Untitled-1",
        ] {
            assert!(is_placeholder_title(title.trim()), "{title:?}");
        }
        for title in [
            "Şeker Portakalı",
            "Thinking, Fast and Slow",
            "1984 Revisited",
        ] {
            assert!(!is_placeholder_title(title), "{title}");
        }
    }
}
