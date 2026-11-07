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
    /// The most pages OCR may read: a longer scan is refused with
    /// [`ScanTooLong`] before any page is recognised. `None` reads any length.
    pub max_ocr_pages: Option<usize>,
}

/// A scan longer than [`ImportOptions::max_ocr_pages`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanTooLong {
    pub pages: usize,
    pub max: usize,
}

impl std::fmt::Display for ScanTooLong {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this scan has {} pages; at most {} pages are read by OCR here",
            self.pages, self.max
        )
    }
}

impl std::error::Error for ScanTooLong {}

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
        if let Some(max) = opts.max_ocr_pages {
            let pages = paginaverto::inspect(paginaverto::Input::Path(input), &read_opts)
                .map(|i| i.pages.len())
                .map_err(anyhow::Error::from)?;
            if pages > max {
                return Err(ScanTooLong { pages, max }.into());
            }
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

/// What the PDF says of itself in an `/Info` entry (`Title`, `Author`), unless
/// it is empty or what a program left there: a file name, or "Microsoft Word -
/// <file>".
fn info_text(doc: &paginaverto::Document, key: &str) -> Option<String> {
    let text = doc.metadata.info.get(key)?.trim();
    if is_placeholder_title(text) {
        return None;
    }
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The title and author a file name gives, in the common "Title (Author)
/// (site.one, site.two).pdf" form: groups in parentheses that name only web
/// sites are dropped, and a last group that is left is the author (up to a
/// "[sort form]"). Underscores stand for spaces in a name that has none, and
/// "_ " for the ": " a file name cannot hold; a download's trailing numbers
/// ("[#1829082]-5445572") are dropped.
fn title_and_author_from_file_name(stem: &str) -> (Option<String>, Option<String>) {
    let mut name = if stem.contains(' ') {
        stem.replace("_ ", ": ")
    } else {
        stem.replace('_', " ")
    };
    loop {
        let trimmed = name.trim_end();
        let cut = if let Some(rest) = trimmed.strip_suffix(']') {
            rest.rfind('[')
                .filter(|&i| !rest[i + 1..].chars().any(char::is_alphabetic))
        } else {
            trimmed
                .rfind('-')
                .filter(|&i| i > 0 && trimmed[i + 1..].chars().all(|c| c.is_ascii_digit()))
                .filter(|&i| trimmed.len() - i > 4)
        };
        match cut {
            Some(i) => name.truncate(i),
            None => break,
        }
    }
    let is_site = |part: &str| {
        let part = part.trim();
        let labels: Vec<&str> = part.split('.').collect();
        labels.len() >= 2
            && labels
                .iter()
                .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            && labels
                .last()
                .is_some_and(|tld| tld.chars().all(|c| c.is_ascii_alphabetic()))
    };
    // The last "(…)" group at the end of `name`: its content and where it starts.
    let last_group = |name: &str| -> Option<(String, usize)> {
        let name = name.trim_end();
        if !name.ends_with(')') {
            return None;
        }
        let open = name.rfind('(')?;
        Some((name[open + 1..name.len() - 1].to_string(), open))
    };
    while let Some((inner, open)) = last_group(&name) {
        if !inner.split(',').all(is_site) {
            break;
        }
        name.truncate(open);
    }
    let mut author = None;
    if let Some((inner, open)) = last_group(&name) {
        let who = inner
            .split('[')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let title = name[..open].trim();
        if !title.is_empty()
            && who.chars().any(char::is_alphabetic)
            && !who.chars().any(|c| c.is_ascii_digit())
        {
            author = Some(who);
            name.truncate(open);
        }
    }
    let title = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = (!title.is_empty()).then_some(title);
    (title, author)
}

/// An `/Info` `Author` that names the computer or account a program ran on,
/// not a writer.
fn is_account_name(author: &str) -> bool {
    const ACCOUNTS: &[&str] = &[
        "admin",
        "administrator",
        "user",
        "owner",
        "windows user",
        "microsoft",
        "unknown",
        "anonymous",
        "lenovo",
        "hp",
        "dell",
        "asus",
        "acer",
        "toshiba",
        "samsung",
        "compaq",
        "zamzar",
    ];
    ACCOUNTS.contains(&author.trim().to_lowercase().as_str())
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

    // The PDF's own title and author, else what the file name says.
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (name_title, name_author) = title_and_author_from_file_name(&stem);
    let title = info_text(&doc, "Title")
        .or(name_title)
        .unwrap_or_else(|| "Imported book".into());
    // The file name first: an `/Info` `Author` is often the computer's account.
    let author = name_author.or_else(|| info_text(&doc, "Author").filter(|a| !is_account_name(a)));
    // The option, else the PDF's own /Lang (paginaverto never guesses).
    let language = doc.language.as_deref().unwrap_or("en");

    let chapters = doc.chapters();
    reflow::write_epub(
        output,
        &title,
        author.as_deref(),
        language,
        &chapters,
        doc.cover.as_ref(),
    )?;

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
    use super::{is_account_name, is_placeholder_title, title_and_author_from_file_name};

    #[test]
    fn a_computer_is_not_an_author() {
        assert!(is_account_name("Lenovo"));
        assert!(is_account_name(" Windows User "));
        assert!(!is_account_name("Daniel Kahneman"));
    }

    #[test]
    fn a_file_name_gives_title_and_author() {
        let t = |s: &str| {
            let (title, author) = title_and_author_from_file_name(s);
            (title.unwrap_or_default(), author.unwrap_or_default())
        };
        assert_eq!(
            t("Felsefe 101 (Paul Kleinman) (z-library.sk, 1lib.sk, z-lib.sk)"),
            ("Felsefe 101".into(), "Paul Kleinman".into())
        );
        assert_eq!(
            t("Harry Potter (J. K. Rowling [Rowling, J. K.]) (z-library.sk)"),
            ("Harry Potter".into(), "J. K. Rowling".into())
        );
        assert_eq!(
            t("Project_Hail_Mary(Andy_Weir)"),
            ("Project Hail Mary".into(), "Andy Weir".into())
        );
        assert_eq!(
            t("aliceinwonderlan00delarich"),
            ("aliceinwonderlan00delarich".into(), String::new())
        );
        assert_eq!(
            t("SANAT_ CIA DERSLERİ[#1829082]-5445572"),
            ("SANAT: CIA DERSLERİ".into(), String::new())
        );
        assert_eq!(t("Kitap 1-2"), ("Kitap 1-2".into(), String::new()));
        // A year or an edition in parentheses is not an author.
        assert_eq!(t("Kitap (2019)"), ("Kitap (2019)".into(), String::new()));
        // Nothing but a group: it stays the title.
        assert_eq!(
            t("(Paul Kleinman)"),
            ("(Paul Kleinman)".into(), String::new())
        );
    }

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
