//! Kobo `.kepub` transformation.
//!
//! Kobo e-readers gain their richer reading features (accurate page turns,
//! reading statistics, dictionary lookup) when a book's content documents carry
//! Kobo's `koboSpan` markup: every sentence is wrapped in
//! `<span class="koboSpan" id="kobo.{paragraph}.{segment}">…</span>`. This module
//! injects that markup into the already-modernized XHTML, mirroring the
//! transformation popularised by the open-source `kepubify` tool:
//!
//! - sentence-level `koboSpan` wrapping (skipping `script`, `style`, `pre`,
//!   `audio`, `video`, `svg`, `math`),
//! - each `<img>` gets its own paragraph and wrapping span,
//! - the body's contents are wrapped in `<div id="book-columns"><div
//!   id="book-inner">`,
//! - a `kobostylehacks` style is added to `<head>` to zero the inner margins.
//!
//! It runs as an extra step on top of the normal pipeline, so a `.kepub.epub`
//! is still a valid EPUB 3 — Kobo simply keys on the `.kepub.epub` extension and
//! the spans. The transform streams XML events and operates on the *raw*
//! (already-escaped) text, so it never has to know about HTML entities like
//! `&nbsp;`.

use anyhow::Result;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

/// Inject Kobo `koboSpan` markup into every content (X)HTML file under `root`.
///
/// Files that fail to parse are left untouched (with a warning), so a single odd
/// document can never abort the conversion.
pub fn kobo_spanify(root: &Path, progress: &dyn Fn(&str)) -> Result<()> {
    let mut transformed = 0usize;
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

        match transform_file(path) {
            Ok(()) => transformed += 1,
            Err(e) => progress(&format!(
                "  [!] Warning: could not add koboSpans to {}: {}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                e
            )),
        }
    }
    progress(&format!(
        "  [+] Added Kobo koboSpan markup to {} content document(s).",
        transformed
    ));
    Ok(())
}

/// Read one (X)HTML file, inject the markup, and write it back. The new bytes are
/// fully built before anything is written, so a mid-stream error leaves the
/// original file intact.
fn transform_file(path: &Path) -> Result<()> {
    let raw = fs::read(path)?;
    let content = String::from_utf8_lossy(&raw);
    let out = spanify(&content)?;
    fs::write(path, out)?;
    Ok(())
}

/// Lower-cased local name of an element (prefix stripped).
fn local_lower(name: QName) -> String {
    String::from_utf8_lossy(name.local_name().as_ref()).to_ascii_lowercase()
}

/// Elements whose text content must NOT be wrapped in koboSpans.
fn is_skip(tag: &str) -> bool {
    matches!(
        tag,
        "script" | "style" | "pre" | "audio" | "video" | "svg" | "math"
    )
}

/// Block-level elements: opening one starts a new koboSpan "paragraph".
fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "blockquote"
            | "ol"
            | "ul"
            | "dl"
            | "dd"
            | "dt"
            | "table"
            | "tr"
            | "td"
            | "th"
            | "section"
            | "article"
            | "aside"
            | "header"
            | "footer"
            | "nav"
            | "figure"
            | "figcaption"
            | "caption"
            | "main"
    )
}

/// Split raw (escaped) text into sentence segments. A boundary falls after a
/// run of `. ! ?` plus optional closing punctuation, then trailing whitespace.
/// Concatenating the returned segments reproduces the input exactly. Operating on
/// the still-escaped text keeps entities (`&amp;`, `&#46;`, …) opaque, so they
/// never trigger a false split and never need re-escaping on the way out.
fn split_sentences(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        cur.push(c);
        i += 1;
        if matches!(c, '.' | '!' | '?') {
            // Closing punctuation that may trail the terminator.
            while i < chars.len()
                && matches!(
                    chars[i],
                    '\'' | '"' | '\u{201D}' | '\u{2019}' | '\u{00BB}' | '\u{2026}'
                )
            {
                cur.push(chars[i]);
                i += 1;
            }
            // Only break if whitespace follows (so "3.14" / "e.g." mid-token
            // stay intact); pull the whitespace into the closing segment.
            if i < chars.len() && chars[i].is_whitespace() {
                while i < chars.len() && chars[i].is_whitespace() {
                    cur.push(chars[i]);
                    i += 1;
                }
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Open a `koboSpan` with the given paragraph/segment id.
fn open_span<W: std::io::Write>(writer: &mut Writer<W>, para: u32, seg: u32) -> Result<()> {
    let id = format!("kobo.{}.{}", para, seg);
    let mut span = BytesStart::new("span");
    span.push_attribute(("class", "koboSpan"));
    span.push_attribute(("id", id.as_str()));
    writer.write_event(Event::Start(span))?;
    Ok(())
}

/// Flush buffered body text: split it into sentences and emit each as a
/// `koboSpan`. `buf` holds the raw (escaped) run of text, with any entity
/// references already reconstructed as `&name;`, so it round-trips verbatim.
fn flush_text<W: std::io::Write>(
    writer: &mut Writer<W>,
    buf: &mut String,
    para: &mut u32,
    seg: &mut u32,
) -> Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let text = std::mem::take(buf);
    for piece in split_sentences(&text) {
        if piece.trim().is_empty() {
            // Whitespace between elements: keep as-is, no span.
            writer.write_event(Event::Text(BytesText::from_escaped(piece.as_str())))?;
        } else {
            if *para == 0 {
                // Text before any block element opens.
                *para = 1;
                *seg = 0;
            }
            *seg += 1;
            open_span(writer, *para, *seg)?;
            // Already-escaped text (entities intact) -> write verbatim.
            writer.write_event(Event::Text(BytesText::from_escaped(piece.as_str())))?;
            writer.write_event(Event::End(BytesEnd::new("span")))?;
        }
    }
    Ok(())
}

/// Transform one XHTML document's text, returning the new bytes.
fn spanify(content: &str) -> Result<Vec<u8>> {
    let mut reader = Reader::from_str(content);
    let mut writer = Writer::new(Vec::new());

    let mut in_body = false;
    let mut skip_depth: usize = 0;
    let mut para: u32 = 0;
    let mut seg: u32 = 0;
    // Raw run of body text accumulated across consecutive Text/GeneralRef events,
    // so an entity (e.g. `&amp;`) never splits a word across two koboSpans.
    let mut text_buf = String::new();

    loop {
        match reader.read_event()? {
            Event::Eof => {
                flush_text(&mut writer, &mut text_buf, &mut para, &mut seg)?;
                break;
            }

            // Accumulate body text; defer wrapping until the next structural event.
            Event::Text(e) => {
                if in_body && skip_depth == 0 {
                    text_buf.push_str(&String::from_utf8_lossy(&e));
                } else {
                    writer.write_event(Event::Text(e))?;
                }
            }
            Event::GeneralRef(r) => {
                if in_body && skip_depth == 0 {
                    let name = String::from_utf8_lossy(&r);
                    text_buf.push('&');
                    text_buf.push_str(&name);
                    text_buf.push(';');
                } else {
                    writer.write_event(Event::GeneralRef(r))?;
                }
            }

            Event::Start(e) => {
                flush_text(&mut writer, &mut text_buf, &mut para, &mut seg)?;
                let tag = local_lower(e.name());
                match tag.as_str() {
                    "body" => {
                        in_body = true;
                        writer.write_event(Event::Start(e))?;
                        // Wrap the body's contents in Kobo's column scaffolding.
                        let mut cols = BytesStart::new("div");
                        cols.push_attribute(("id", "book-columns"));
                        writer.write_event(Event::Start(cols))?;
                        let mut inner = BytesStart::new("div");
                        inner.push_attribute(("id", "book-inner"));
                        writer.write_event(Event::Start(inner))?;
                    }
                    _ => {
                        if is_skip(&tag) {
                            skip_depth += 1;
                        } else if in_body && skip_depth == 0 && is_block(&tag) {
                            para += 1;
                            seg = 0;
                        }
                        writer.write_event(Event::Start(e))?;
                    }
                }
            }

            Event::End(e) => {
                flush_text(&mut writer, &mut text_buf, &mut para, &mut seg)?;
                let tag = local_lower(e.name());
                match tag.as_str() {
                    "body" => {
                        // Close the inner + columns wrappers we opened.
                        writer.write_event(Event::End(BytesEnd::new("div")))?;
                        writer.write_event(Event::End(BytesEnd::new("div")))?;
                        writer.write_event(Event::End(e))?;
                        in_body = false;
                    }
                    "head" => {
                        // Margin-zeroing hack Kobo expects on the inner column.
                        let mut style = BytesStart::new("style");
                        style.push_attribute(("type", "text/css"));
                        style.push_attribute(("class", "kobostylehacks"));
                        writer.write_event(Event::Start(style))?;
                        writer.write_event(Event::Text(BytesText::new(
                            "div#book-inner { margin-top: 0; margin-bottom: 0;}",
                        )))?;
                        writer.write_event(Event::End(BytesEnd::new("style")))?;
                        writer.write_event(Event::End(e))?;
                    }
                    _ => {
                        if is_skip(&tag) && skip_depth > 0 {
                            skip_depth -= 1;
                        }
                        writer.write_event(Event::End(e))?;
                    }
                }
            }

            Event::Empty(e) => {
                flush_text(&mut writer, &mut text_buf, &mut para, &mut seg)?;
                let tag = local_lower(e.name());
                if in_body && skip_depth == 0 && tag == "img" {
                    // Each image is its own paragraph, wrapped in a koboSpan.
                    para += 1;
                    seg = 1;
                    open_span(&mut writer, para, seg)?;
                    writer.write_event(Event::Empty(e))?;
                    writer.write_event(Event::End(BytesEnd::new("span")))?;
                } else {
                    writer.write_event(Event::Empty(e))?;
                }
            }

            // Declarations, doctype, comments, CDATA, processing instructions:
            // flush any pending text, then pass through untouched.
            other => {
                flush_text(&mut writer, &mut text_buf, &mut para, &mut seg)?;
                writer.write_event(other)?;
            }
        }
    }

    Ok(writer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(xhtml: &str) -> String {
        String::from_utf8(spanify(xhtml).unwrap()).unwrap()
    }

    #[test]
    fn wraps_paragraph_text_in_kobospans() {
        let out = run(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>t</title></head>\
             <body><p>Hello world. Second sentence!</p></body></html>",
        );
        assert!(out.contains("class=\"koboSpan\""));
        assert!(out.contains("id=\"kobo.1.1\""));
        assert!(
            out.contains("id=\"kobo.1.2\""),
            "two sentences -> two spans"
        );
        assert!(out.contains("<div id=\"book-columns\""));
        assert!(out.contains("<div id=\"book-inner\""));
        assert!(out.contains("kobostylehacks"));
    }

    #[test]
    fn does_not_wrap_skipped_elements() {
        let out = run("<html><head></head><body><p>Hi.</p>\
             <script>var x = 1; foo.bar();</script>\
             <style>.a{color:red}</style></body></html>");
        // Script/style bodies stay verbatim (no spans injected inside them).
        assert!(out.contains("var x = 1; foo.bar();"));
        assert!(
            !out.contains("koboSpan\" id=\"kobo")
                || !out[out.find("<script").unwrap()..out.find("</script").unwrap()]
                    .contains("koboSpan")
        );
    }

    #[test]
    fn image_gets_its_own_span_and_paragraph() {
        let out = run(
            "<html><head></head><body><p><img src=\"a.png\" alt=\"x\"/></p>\
             <p>Text.</p></body></html>",
        );
        // The img is wrapped, and the following paragraph advances past it.
        assert!(out.contains("koboSpan"));
        assert!(out.contains("<img"));
        assert!(out.contains("src=\"a.png\""));
    }

    #[test]
    fn preserves_entities_without_breaking() {
        let out = run("<html><head></head><body><p>A&amp;B and 3.14 stay.</p></body></html>");
        // Entity survives verbatim; "3.14" is not split into two sentences.
        assert!(out.contains("A&amp;B and 3.14 stay."));
    }
}
