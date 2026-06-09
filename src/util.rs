//! Small shared helpers: URL (percent) encoding compatible with Python's
//! `urllib.parse.quote`/`unquote`, href manipulation, and XHTML DOCTYPE
//! standardization.

use any_ascii::any_ascii;
use anyhow::Result;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use regex::Regex;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

/// Characters that Python's `urllib.parse.quote` leaves untouched by default:
/// unreserved characters (`A-Z a-z 0-9 _ . - ~`) plus the `/` path separator
/// (because `quote` uses `safe='/'` by default).
const QUOTE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'_')
    .remove(b'.')
    .remove(b'-')
    .remove(b'~')
    .remove(b'/');

/// Percent-encode a string the way Python's `urllib.parse.quote` does.
pub fn quote(s: &str) -> String {
    utf8_percent_encode(s, QUOTE_SET).to_string()
}

/// Percent-decode a string the way Python's `urllib.parse.unquote` does.
pub fn unquote(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// Return the final path component (everything after the last `/`).
pub fn basename(href: &str) -> &str {
    match href.rfind('/') {
        Some(i) => &href[i + 1..],
        None => href,
    }
}

/// Replace the file extension of an href with `.webp`, mirroring
/// `pathlib.Path(href).with_suffix('.webp')`.
pub fn with_webp_ext(href: &str) -> String {
    let slash = href.rfind('/').map(|i| i as isize).unwrap_or(-1);
    match href.rfind('.') {
        Some(dot) if (dot as isize) > slash => format!("{}.webp", &href[..dot]),
        _ => format!("{}.webp", href),
    }
}

/// Transliterate a filename stem to an ASCII-safe slug for the `--ascii` option.
/// Unicode letters are romanized (e.g. Turkish `Işık Doğudan` → `Isik_Dogudan`),
/// whitespace becomes underscores, and any character outside `[A-Za-z0-9._-]` is
/// dropped. Runs of underscores are collapsed and leading/trailing ones trimmed.
/// Falls back to `output` if nothing printable remains.
pub fn slugify_ascii(stem: &str) -> String {
    let romanized = any_ascii(stem);
    let mut out = String::with_capacity(romanized.len());
    let mut prev_underscore = false;
    for ch in romanized.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
            out.push(ch);
            prev_underscore = false;
        } else if ch.is_whitespace() || ch == '_' {
            if !prev_underscore {
                out.push('_');
                prev_underscore = true;
            }
        }
        // anything else (punctuation, symbols) is dropped
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "output".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Standardize HTML/XHTML files to EPUB 3 best practices:
/// - Replace legacy DOCTYPE declarations with the HTML5 `<!DOCTYPE html>`.
/// - Ensure the XHTML namespace is declared on the `<html>` element.
pub fn standardize_xhtml_files(root: &Path) -> Result<()> {
    let doctype_re = Regex::new(r"(?i)<!DOCTYPE\s+html[^>]*>").unwrap();

    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let is_html = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "xhtml" | "htm"))
            .unwrap_or(false);
        if !is_html {
            continue;
        }

        let raw = match fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!(
                    "  [!] Warning: Could not modernize HTML tag in {}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    e
                );
                continue;
            }
        };
        let mut content = String::from_utf8_lossy(&raw).into_owned();

        content = doctype_re.replace_all(&content, "<!DOCTYPE html>").into_owned();

        if !content.contains("xmlns=\"http://www.w3.org/1999/xhtml\"") {
            content = content.replace("<html", "<html xmlns=\"http://www.w3.org/1999/xhtml\"");
        }

        if let Err(e) = fs::write(path, content) {
            eprintln!(
                "  [!] Warning: Could not modernize HTML tag in {}: {}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                e
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::slugify_ascii;

    #[test]
    fn slugifies_turkish_title() {
        assert_eq!(slugify_ascii("Işık Doğudan Yükselir"), "Isik_Dogudan_Yukselir");
    }

    #[test]
    fn collapses_and_trims_separators() {
        assert_eq!(slugify_ascii("  A  --  B!! "), "A_--_B");
        assert_eq!(slugify_ascii("Çöl: Bir_Öykü"), "Col_Bir_Oyku");
    }

    #[test]
    fn keeps_dots_and_dashes() {
        assert_eq!(slugify_ascii("Vol.2 - Part 3"), "Vol.2_-_Part_3");
    }

    #[test]
    fn empty_after_strip_falls_back() {
        assert_eq!(slugify_ascii("！？"), "output");
    }
}
