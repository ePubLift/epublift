//! Input classification + text extraction from a PDF, WITH font size and
//! position — the signals flat `extract_text` discards.
//!
//! We run the PDF text/graphics state machine over the page content stream
//! (tracking the CTM, text matrix, and current font size) and decode each
//! shown string to Unicode via the font's encoding (`get_font_encoding` +
//! `decode_text`, so we don't reimplement ToUnicode). The result is a list of
//! [`TextRun`]s carrying effective font size + device-space position, which
//! [`super::structure`] uses to strip running heads by position and detect
//! chapter headings by font size.
//!
//! TODO: extract the largest image per page verbatim (for figures) — ported
//! from the spike's `extract-images`.

use std::collections::HashMap;

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Encoding, Object, ObjectId};

/// One run of shown text with its rendered geometry.
#[derive(Debug, Clone)]
pub(crate) struct TextRun {
    pub text: String,
    /// Effective rendered font size (text-space size × matrix vertical scale).
    pub font_size: f32,
    /// Device-space start x of the run.
    pub x: f32,
    /// Device-space end x (start + true advance from glyph widths) — lets us
    /// place inter-word spaces exactly instead of estimating glyph widths.
    pub end_x: f32,
    pub y: f32,
}

/// Extracted text content of one page.
#[derive(Debug, Clone, Default)]
pub(crate) struct PageContent {
    /// Page geometry in PDF units (kept for column/fixed-layout detection).
    #[allow(dead_code)]
    pub width: f32,
    #[allow(dead_code)]
    pub height: f32,
    pub runs: Vec<TextRun>,
}

/// Which extraction tier an input falls into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputKind {
    /// Has a real text layer (born-digital, or a scan with embedded OCR text).
    TextLayer,
    /// Image pages with (almost) no text layer — needs OCR (`pdf-ocr`).
    Scan,
}

// ---- 2D affine matrices, PDF row-vector convention [a b c d e f] ----
type Mat = [f32; 6];
const IDENTITY: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` applied first, then `b` (PDF: combined = a × b for row vectors).
fn mat_mul(a: Mat, b: Mat) -> Mat {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn translate(tx: f32, ty: f32) -> Mat {
    [1.0, 0.0, 0.0, 1.0, tx, ty]
}

/// Record a shown string at the current text position (text matrix × CTM).
/// `adv_em` is the run's total advance in em units (from real glyph widths),
/// used to compute the device-space end x.
fn push_run(runs: &mut Vec<TextRun>, tm: Mat, ctm: Mat, font_size: f32, text: String, adv_em: f32) {
    if text.trim().is_empty() {
        return;
    }
    let m = mat_mul(tm, ctm);
    let yscale = (m[2] * m[2] + m[3] * m[3]).sqrt();
    let xscale = (m[0] * m[0] + m[1] * m[1]).sqrt();
    let width = adv_em * font_size * xscale;
    runs.push(TextRun {
        text,
        font_size: font_size * yscale,
        x: m[4],
        end_x: m[4] + width,
        y: m[5],
    });
}

fn num(o: &Object) -> f32 {
    o.as_float().unwrap_or(0.0)
}

/// Page width/height in PDF units, from the (possibly inherited) MediaBox.
fn media_box(doc: &Document, page_id: ObjectId) -> (f32, f32) {
    let mut cur = doc.get_object(page_id).ok().and_then(|o| o.as_dict().ok());
    for _ in 0..16 {
        let Some(dict) = cur else { break };
        if let Ok(arr) = dict.get(b"MediaBox").and_then(|o| o.as_array())
            && arr.len() == 4
        {
            let v: Vec<f32> = arr.iter().map(num).collect();
            return ((v[2] - v[0]).abs(), (v[3] - v[1]).abs());
        }
        cur = dict
            .get(b"Parent")
            .ok()
            .and_then(|p| doc.get_object(p.as_reference().ok()?).ok())
            .and_then(|o| o.as_dict().ok());
    }
    (612.0, 792.0) // US-Letter fallback
}

/// How to map a font's shown bytes to Unicode.
enum Dec<'a> {
    /// Simple (1-byte) fonts via lopdf's built-in encodings.
    Simple(Encoding<'a>),
    /// Any font with a /ToUnicode CMap, parsed ourselves (code length 1 or 2
    /// bytes). Covers Type0/CID and simple fonts with custom Differences
    /// encodings that lopdf 0.34 can't read; `ToUnicodeCMap::parse` is also
    /// crate-private, so we parse the CMap ourselves.
    Cid {
        map: HashMap<u32, String>,
        code_len: usize,
    },
}

/// A page font: how to decode its codes, plus per-code glyph widths (em units)
/// so we can compute exact run advances for word spacing.
struct Font<'a> {
    dec: Dec<'a>,
    widths: HashMap<u32, f32>,
    default_width: f32,
}

/// Per-page font name → decoder + glyph widths. Resolves fonts via our own
/// resource walk; lopdf's `get_page_fonts` misses inherited Resources (which
/// some PDFs use — then it finds no fonts and text decodes to garbage).
fn page_encodings<'a>(doc: &'a Document, page_id: ObjectId) -> HashMap<Vec<u8>, Font<'a>> {
    let mut map = HashMap::new();
    for (name, dict) in page_font_dicts(doc, page_id) {
        let subtype = dict
            .get(b"Subtype")
            .and_then(|o| o.as_name())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        // Prefer the font's own /ToUnicode CMap (authoritative for extraction;
        // handles Type0/CID and simple fonts with custom Differences encodings
        // lopdf can't read). Simple (1-byte) fonts read one byte per code
        // regardless of the CMap's declared source width; Type0 uses the width.
        let dec = if let Some((cmap, inferred)) = to_unicode_map(doc, dict) {
            let code_len = if subtype == "Type0" { inferred } else { 1 };
            Dec::Cid {
                map: cmap,
                code_len,
            }
        } else if let Ok(enc) = dict.get_font_encoding(doc) {
            Dec::Simple(enc)
        } else {
            continue;
        };
        let (widths, default_width) = font_widths(doc, dict, &subtype);
        map.insert(
            name,
            Font {
                dec,
                widths,
                default_width,
            },
        );
    }
    map
}

/// Per-code glyph widths (em units) and a default width, from `/Widths`
/// (simple fonts) or the descendant `/W` + `/DW` (Type0/CID).
fn font_widths(doc: &Document, dict: &Dictionary, subtype: &str) -> (HashMap<u32, f32>, f32) {
    let mut widths = HashMap::new();
    if subtype == "Type0" {
        let desc = dict
            .get(b"DescendantFonts")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_dict().ok());
        let Some(desc) = desc else {
            return (widths, 0.5);
        };
        let dw = desc
            .get(b"DW")
            .ok()
            .and_then(|o| o.as_float().ok())
            .unwrap_or(1000.0)
            / 1000.0;
        if let Some(w) = desc
            .get(b"W")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_array().ok())
        {
            let mut i = 0;
            while i < w.len() {
                let Some(c) = w[i].as_i64().ok() else { break };
                // form 1: c [w1 w2 …]   form 2: cFirst cLast w
                if let Some(arr) = w.get(i + 1).and_then(|o| o.as_array().ok()) {
                    for (j, wv) in arr.iter().enumerate() {
                        if let Ok(wf) = wv.as_float() {
                            widths.insert(c as u32 + j as u32, wf / 1000.0);
                        }
                    }
                    i += 2;
                } else if let (Some(clast), Some(wv)) = (
                    w.get(i + 1).and_then(|o| o.as_i64().ok()),
                    w.get(i + 2).and_then(|o| o.as_float().ok()),
                ) {
                    for code in c..=clast.min(c + 65535) {
                        widths.insert(code as u32, wv / 1000.0);
                    }
                    i += 3;
                } else {
                    break;
                }
            }
        }
        (widths, dw)
    } else {
        let first = dict
            .get(b"FirstChar")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        if let Some(arr) = dict
            .get(b"Widths")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_array().ok())
        {
            for (i, wv) in arr.iter().enumerate() {
                if let Ok(wf) = wv.as_float() {
                    widths.insert((first + i as i64) as u32, wf / 1000.0);
                }
            }
        }
        (widths, 0.5)
    }
}

/// The Resources dictionary for a page, walking the Parent chain (Resources can
/// be inherited from an ancestor node in the page tree).
fn page_resources(doc: &Document, page_id: ObjectId) -> Option<&Dictionary> {
    let mut cur = doc.get_object(page_id).ok().and_then(|o| o.as_dict().ok());
    for _ in 0..16 {
        let dict = cur?;
        if let Some(res) = dict
            .get(b"Resources")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_dict().ok())
        {
            return Some(res);
        }
        cur = dict
            .get(b"Parent")
            .ok()
            .and_then(|p| deref(doc, p))
            .and_then(|o| o.as_dict().ok());
    }
    None
}

/// Font name → font Dictionary for a page (resolving inherited Resources).
fn page_font_dicts(doc: &Document, page_id: ObjectId) -> Vec<(Vec<u8>, &Dictionary)> {
    let mut out = Vec::new();
    let Some(res) = page_resources(doc, page_id) else {
        return out;
    };
    let Some(fonts) = res
        .get(b"Font")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_dict().ok())
    else {
        return out;
    };
    for (name, val) in fonts.iter() {
        if let Some(fd) = deref(doc, val).and_then(|o| o.as_dict().ok()) {
            out.push((name.clone(), fd));
        }
    }
    out
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

/// Decode a shown string to (text, advance-in-em). The advance sums real glyph
/// widths so the caller can place inter-word spaces exactly.
fn decode_run(font: Option<&Font>, bytes: &[u8]) -> (String, f32) {
    let Some(font) = font else {
        return (
            String::from_utf8_lossy(bytes).into_owned(),
            bytes.len() as f32 * 0.5,
        );
    };
    let width = |code: u32| {
        font.widths
            .get(&code)
            .copied()
            .unwrap_or(font.default_width)
    };
    match &font.dec {
        Dec::Simple(e) => {
            let text = e
                .bytes_to_string(bytes)
                .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned());
            let adv = bytes.iter().map(|&b| width(b as u32)).sum();
            (text, adv)
        }
        Dec::Cid { map, code_len } => {
            let n = (*code_len).max(1);
            let mut out = String::new();
            let mut adv = 0.0_f32;
            let mut i = 0;
            while i + n <= bytes.len() {
                let mut code = 0u32;
                for k in 0..n {
                    code = (code << 8) | bytes[i + k] as u32;
                }
                if let Some(s) = map.get(&code) {
                    out.push_str(s);
                }
                adv += width(code);
                i += n;
            }
            (out, adv)
        }
    }
}

/// Build a (code→string, code-length-in-bytes) map from a font's `/ToUnicode`.
fn to_unicode_map(doc: &Document, font: &Dictionary) -> Option<(HashMap<u32, String>, usize)> {
    let obj = font.get(b"ToUnicode").ok()?;
    let obj = match obj {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    let stream = obj.as_stream().ok()?;
    let content = stream
        .decompressed_content()
        .or_else(|_| stream.get_plain_content())
        .ok()?;
    parse_to_unicode(&content)
}

/// Minimal ToUnicode CMap parser: handles `beginbfchar`/`beginbfrange` blocks
/// of hex `<src> <dst>` (and `<lo> <hi> [<d>…]` array ranges). dst is UTF-16BE.
/// Also returns the source code length in bytes, inferred from the src token
/// hex width (2 hex = 1 byte, 4 hex = 2 bytes).
fn parse_to_unicode(content: &[u8]) -> Option<(HashMap<u32, String>, usize)> {
    enum Tok {
        Hex(String),
        Open,
        Close,
        Word(String),
    }
    let s = String::from_utf8_lossy(content);
    let b = s.as_bytes();
    let mut toks: Vec<Tok> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'<' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'>' {
                j += 1;
            }
            toks.push(Tok::Hex(
                s[i + 1..j.min(b.len())].split_whitespace().collect(),
            ));
            i = j + 1;
        } else if c == b'[' {
            toks.push(Tok::Open);
            i += 1;
        } else if c == b']' {
            toks.push(Tok::Close);
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else {
            let mut j = i;
            while j < b.len() && !b[j].is_ascii_whitespace() && !matches!(b[j], b'<' | b'[' | b']')
            {
                j += 1;
            }
            toks.push(Tok::Word(s[i..j].to_string()));
            i = j;
        }
    }

    let mut map = HashMap::new();
    let mut src_hex_len = 0usize;
    let mut k = 0;
    while k < toks.len() {
        match &toks[k] {
            Tok::Word(w) if w == "beginbfchar" => {
                k += 1;
                while k + 1 < toks.len() {
                    let (Tok::Hex(src), Tok::Hex(dst)) = (&toks[k], &toks[k + 1]) else {
                        break;
                    };
                    src_hex_len = src_hex_len.max(src.len());
                    if let (Some(code), Some(text)) = (hex_u32(src), hex_string(dst)) {
                        map.insert(code, text);
                    }
                    k += 2;
                }
            }
            Tok::Word(w) if w == "beginbfrange" => {
                k += 1;
                loop {
                    if k + 2 >= toks.len() {
                        break;
                    }
                    let (Tok::Hex(lo), Tok::Hex(hi)) = (&toks[k], &toks[k + 1]) else {
                        break;
                    };
                    src_hex_len = src_hex_len.max(lo.len());
                    let (Some(lo), Some(hi)) = (hex_u32(lo), hex_u32(hi)) else {
                        break;
                    };
                    k += 2;
                    match &toks[k] {
                        Tok::Hex(dst) => {
                            if let Some(units) = hex_units(dst) {
                                for (off, code) in (lo..=hi.min(lo + 65535)).enumerate() {
                                    let mut u = units.clone();
                                    if let Some(last) = u.last_mut() {
                                        *last = last.wrapping_add(off as u16);
                                    }
                                    map.insert(code, String::from_utf16_lossy(&u));
                                }
                            }
                            k += 1;
                        }
                        Tok::Open => {
                            k += 1;
                            let mut off = 0u32;
                            while k < toks.len() {
                                match &toks[k] {
                                    Tok::Hex(d) => {
                                        if let Some(t) = hex_string(d) {
                                            map.insert(lo + off, t);
                                        }
                                        off += 1;
                                        k += 1;
                                    }
                                    _ => {
                                        k += 1;
                                        break;
                                    }
                                }
                            }
                        }
                        _ => break,
                    }
                }
            }
            _ => k += 1,
        }
    }
    if map.is_empty() {
        None
    } else {
        let code_len = if src_hex_len == 0 {
            2
        } else {
            src_hex_len.div_ceil(2)
        };
        Some((map, code_len))
    }
}

fn hex_u32(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim(), 16).ok()
}

/// Parse a hex string as a sequence of UTF-16 code units (4 hex digits each).
fn hex_units(s: &str) -> Option<Vec<u16>> {
    let s = s.trim();
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut units = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    for chunk in chars.chunks(4) {
        let h: String = chunk.iter().collect();
        units.push(u16::from_str_radix(&h, 16).ok()?);
    }
    Some(units)
}

fn hex_string(s: &str) -> Option<String> {
    hex_units(s).map(|u| String::from_utf16_lossy(&u))
}

/// Extract positioned text runs from one page.
pub(crate) fn extract_page(doc: &Document, page_id: ObjectId) -> PageContent {
    let (width, height) = media_box(doc, page_id);
    let mut page = PageContent {
        width,
        height,
        runs: Vec::new(),
    };

    let Ok(data) = doc.get_page_content(page_id) else {
        return page;
    };
    let Ok(content) = Content::decode(&data) else {
        return page;
    };
    let encodings = page_encodings(doc, page_id);

    // Graphics + text state.
    let mut ctm = IDENTITY;
    let mut ctm_stack: Vec<Mat> = Vec::new();
    let mut tm = IDENTITY;
    let mut tlm = IDENTITY;
    let mut font_size = 0.0_f32;
    let mut leading = 0.0_f32;
    let mut cur_enc: Option<&Font> = None;

    for op in &content.operations {
        let a = &op.operands;
        match op.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => {
                if let Some(m) = ctm_stack.pop() {
                    ctm = m;
                }
            }
            "cm" if a.len() == 6 => {
                let m = [
                    num(&a[0]),
                    num(&a[1]),
                    num(&a[2]),
                    num(&a[3]),
                    num(&a[4]),
                    num(&a[5]),
                ];
                ctm = mat_mul(m, ctm);
            }
            "BT" => {
                tm = IDENTITY;
                tlm = IDENTITY;
            }
            "Tf" if a.len() == 2 => {
                font_size = num(&a[1]);
                cur_enc = a[0].as_name().ok().and_then(|n| encodings.get(n));
            }
            "Td" if a.len() == 2 => {
                tlm = mat_mul(translate(num(&a[0]), num(&a[1])), tlm);
                tm = tlm;
            }
            "TD" if a.len() == 2 => {
                leading = -num(&a[1]);
                tlm = mat_mul(translate(num(&a[0]), num(&a[1])), tlm);
                tm = tlm;
            }
            "Tm" if a.len() == 6 => {
                tm = [
                    num(&a[0]),
                    num(&a[1]),
                    num(&a[2]),
                    num(&a[3]),
                    num(&a[4]),
                    num(&a[5]),
                ];
                tlm = tm;
            }
            "TL" if a.len() == 1 => leading = num(&a[0]),
            "T*" => {
                tlm = mat_mul(translate(0.0, -leading), tlm);
                tm = tlm;
            }
            "Tj" if !a.is_empty() => {
                if let Ok(bytes) = a[0].as_str() {
                    let (t, adv) = decode_run(cur_enc, bytes);
                    push_run(&mut page.runs, tm, ctm, font_size, t, adv);
                }
            }
            "'" if !a.is_empty() => {
                tlm = mat_mul(translate(0.0, -leading), tlm);
                tm = tlm;
                if let Ok(bytes) = a[0].as_str() {
                    let (t, adv) = decode_run(cur_enc, bytes);
                    push_run(&mut page.runs, tm, ctm, font_size, t, adv);
                }
            }
            "\"" if a.len() == 3 => {
                tlm = mat_mul(translate(0.0, -leading), tlm);
                tm = tlm;
                if let Ok(bytes) = a[2].as_str() {
                    let (t, adv) = decode_run(cur_enc, bytes);
                    push_run(&mut page.runs, tm, ctm, font_size, t, adv);
                }
            }
            "TJ" if !a.is_empty() => {
                if let Ok(arr) = a[0].as_array() {
                    let mut s = String::new();
                    let mut adv = 0.0_f32;
                    for el in arr {
                        match el {
                            Object::String(bytes, _) => {
                                let (t, a2) = decode_run(cur_enc, bytes);
                                s.push_str(&t);
                                adv += a2;
                            }
                            // Positive TJ numbers move LEFT (reduce advance).
                            Object::Integer(_) | Object::Real(_) => adv -= num(el) / 1000.0,
                            _ => {}
                        }
                    }
                    push_run(&mut page.runs, tm, ctm, font_size, s, adv);
                }
            }
            _ => {}
        }
    }

    page
}

/// Total bytes of shown text on a page (Tj/TJ/'/" string operands), without
/// decoding — used only to detect the presence of a text layer. This is robust
/// where `extract_text` returns empty (CID fonts).
pub(crate) fn shown_text_bytes(doc: &Document, page_id: ObjectId) -> usize {
    let Ok(data) = doc.get_page_content(page_id) else {
        return 0;
    };
    let Ok(content) = Content::decode(&data) else {
        return 0;
    };
    let mut bytes = 0;
    for op in &content.operations {
        match op.operator.as_str() {
            "Tj" | "'" | "\"" => {
                for o in &op.operands {
                    if let Ok(s) = o.as_str() {
                        bytes += s.len();
                    }
                }
            }
            "TJ" => {
                if let Some(Ok(arr)) = op.operands.first().map(|o| o.as_array()) {
                    for el in arr {
                        if let Ok(s) = el.as_str() {
                            bytes += s.len();
                        }
                    }
                }
            }
            _ => {}
        }
    }
    bytes
}

/// Clean text of a page plus the signals the hybrid structurer needs.
#[derive(Debug, Clone, Default)]
pub(crate) struct PageText {
    /// Clean text blocks (≈ paragraphs) from lopdf `extract_text`.
    pub blocks: Vec<String>,
    /// True when the page has no full-page image → clean typography, so the
    /// font-size heading signal is trustworthy (born-digital). False for
    /// OCR-text-layer scans, whose per-word geometry is noisy.
    pub born_digital: bool,
    /// Letters-only normalised texts shown in notably-large font on this page
    /// (heading candidates). Only meaningful when `born_digital`.
    pub big_font: Vec<String>,
    /// Figure images on this page, in document order, to carry into the EPUB.
    /// Only collected on born-digital pages (a scan page's lone full-page image
    /// is the page itself, not a figure).
    pub figures: Vec<Figure>,
}

/// An extracted figure image, ready to drop into the EPUB verbatim.
#[derive(Debug, Clone)]
pub(crate) struct Figure {
    pub data: Vec<u8>,
    pub media_type: &'static str,
    pub ext: &'static str,
}

/// True for a born-digital document (most pages are text, not full-page
/// images). Such a doc's full-page images are real illustrations worth keeping;
/// a searchable-scan doc's full-page images ARE the pages, so we don't.
pub(crate) fn born_digital_doc(doc: &Document) -> bool {
    let pages = doc.get_pages();
    if pages.is_empty() {
        return false;
    }
    let scan = pages
        .values()
        .filter(|&&id| has_full_page_image(doc, id))
        .count();
    scan * 2 < pages.len()
}

/// Extract a page's clean text blocks plus heading/regime signals. `figures` is
/// a doc-level decision (see [`born_digital_doc`]).
pub(crate) fn page_text(
    doc: &Document,
    page_id: ObjectId,
    page_num: u32,
    extract_figures: bool,
) -> PageText {
    let mut blocks: Vec<String> = doc
        .extract_text(&[page_num])
        .unwrap_or_default()
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    let born_digital = !has_full_page_image(doc, page_id);

    // Run our own positioned extractor when we need it: (a) as a CID/Type0
    // fallback when `extract_text` came up empty (composite fonts), and (b) for
    // the big-font heading signal on born-digital pages.
    let mut big_font = Vec::new();
    if blocks.is_empty() || born_digital {
        let pc = extract_page(doc, page_id);
        if blocks.is_empty() {
            blocks = blocks_from_runs(&pc);
        }
        if born_digital {
            big_font = big_font_texts(&pc);
        }
    }
    // Carry over real figures when the doc is born-digital (decided once at the
    // document level — see `born_digital_doc`).
    let figures = if extract_figures {
        page_figures(doc, page_id)
    } else {
        Vec::new()
    };
    PageText {
        blocks,
        born_digital,
        big_font,
        figures,
    }
}

/// Collect a page's image XObjects as EPUB-ready figures (JPEG verbatim, raw
/// 8-bit gray/RGB re-encoded to PNG; JPEG2000 / CCITT / JBIG2 / CMYK skipped).
fn page_figures(doc: &Document, page_id: ObjectId) -> Vec<Figure> {
    let mut out = Vec::new();
    let Some(res) = page_resources(doc, page_id) else {
        return out;
    };
    let Some(xobjects) = res
        .get(b"XObject")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_dict().ok())
    else {
        return out;
    };
    for (_name, val) in xobjects.iter() {
        let Object::Reference(id) = val else { continue };
        let Ok(stream) = doc.get_object(*id).and_then(|o| o.as_stream()) else {
            continue;
        };
        let is_image = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok())
            .map(|n| n == b"Image")
            .unwrap_or(false);
        if !is_image {
            continue;
        }
        if let Some(fig) = figure_from_stream(doc, stream) {
            out.push(fig);
        }
    }
    out
}

fn figure_from_stream(doc: &Document, stream: &lopdf::Stream) -> Option<Figure> {
    let dict = &stream.dict;
    let filter = dict.get(b"Filter").ok().and_then(|o| match o {
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        Object::Array(a) => a
            .iter()
            .filter_map(|x| x.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .next_back(),
        _ => None,
    });
    match filter.as_deref() {
        // A DCTDecode stream's bytes ARE a JPEG file — drop it in verbatim.
        Some("DCTDecode") => Some(Figure {
            data: stream.content.clone(),
            media_type: "image/jpeg",
            ext: "jpg",
        }),
        // Raw (uncompressed-after-Flate) pixels → encode an 8-bit gray/RGB PNG.
        Some("FlateDecode") | Some("LZWDecode") | None => raw_to_png(doc, stream),
        // JPEG2000 / CCITT / JBIG2 etc.: not an EPUB core type and no pure-Rust
        // decoder — skip the figure rather than embed something unreadable.
        _ => None,
    }
}

/// Encode a raw 8-bit DeviceGray/DeviceRGB image stream to PNG.
fn raw_to_png(doc: &Document, stream: &lopdf::Stream) -> Option<Figure> {
    let dict = &stream.dict;
    let w = dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    let bpc = dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(8);
    if bpc != 8 || w == 0 || h == 0 {
        return None;
    }
    let comps = image_components(doc, dict)?;
    let data = stream.decompressed_content().ok()?;
    if data.len() < (w as usize) * (h as usize) * comps {
        return None;
    }
    let mut png = Vec::new();
    let cursor = &mut std::io::Cursor::new(&mut png);
    let ok = match comps {
        1 => image::GrayImage::from_raw(w, h, data)
            .map(image::DynamicImage::ImageLuma8)
            .and_then(|i| i.write_to(cursor, image::ImageFormat::Png).ok()),
        3 => image::RgbImage::from_raw(w, h, data)
            .map(image::DynamicImage::ImageRgb8)
            .and_then(|i| i.write_to(cursor, image::ImageFormat::Png).ok()),
        _ => None,
    };
    ok?;
    Some(Figure {
        data: png,
        media_type: "image/png",
        ext: "png",
    })
}

/// Number of colour components for a raw image, or None for ones we don't
/// re-encode (Indexed, CMYK, Lab, …).
fn image_components(doc: &Document, dict: &Dictionary) -> Option<usize> {
    let cs = dict.get(b"ColorSpace").ok().and_then(|o| deref(doc, o))?;
    match cs {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"CalGray" | b"G" => Some(1),
            b"DeviceRGB" | b"CalRGB" | b"RGB" => Some(3),
            _ => None,
        },
        Object::Array(a) => {
            // e.g. [/ICCBased <stream>] — read the profile's component count /N.
            let head = a.first().and_then(|o| o.as_name().ok())?;
            if head == b"ICCBased" {
                let icc = a
                    .get(1)
                    .and_then(|o| deref(doc, o))
                    .and_then(|o| o.as_stream().ok())?;
                match icc.dict.get(b"N").ok().and_then(|o| o.as_i64().ok())? {
                    1 => Some(1),
                    3 => Some(3),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Reconstruct paragraph-blocks from positioned runs (used as the CID fallback;
/// geometry is clean on born-digital pages). Lines are clustered by y, then
/// merged into blocks at large vertical gaps or first-line indents.
fn blocks_from_runs(p: &PageContent) -> Vec<String> {
    let mut runs: Vec<&TextRun> = p
        .runs
        .iter()
        .filter(|r| !r.text.trim().is_empty())
        .collect();
    if runs.is_empty() {
        return Vec::new();
    }
    runs.sort_by(|a, b| b.y.total_cmp(&a.y)); // top → bottom

    struct Line {
        y: f32,
        x: f32,
        font: f32,
        text: String,
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut i = 0;
    while i < runs.len() {
        let y0 = runs[i].y;
        let tol = runs[i].font_size.max(1.0) * 0.6;
        let mut group: Vec<&TextRun> = Vec::new();
        while i < runs.len() && (y0 - runs[i].y).abs() <= tol {
            group.push(runs[i]);
            i += 1;
        }
        group.sort_by(|a, b| a.x.total_cmp(&b.x));
        // Join runs by x-gap: many PDFs (incl. CID) place each glyph/word as a
        // separate run, so a blind " ".join would space out every character.
        // Each run knows its true end_x (from glyph widths), so a real space is
        // a gap from the previous run's end to this run's start beyond ~0.2em.
        let mut text = String::new();
        let mut prev_end: Option<f32> = None;
        for r in &group {
            let t = r.text.trim();
            if t.is_empty() {
                continue;
            }
            if let Some(pe) = prev_end
                && r.x - pe > r.font_size * 0.2
            {
                text.push(' ');
            }
            text.push_str(t);
            prev_end = Some(r.end_x);
        }
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        let font = group.iter().map(|r| r.font_size).fold(0.0, f32::max);
        let x = group.iter().map(|r| r.x).fold(f32::INFINITY, f32::min);
        lines.push(Line {
            y: y0,
            x,
            font,
            text,
        });
    }

    let mut gaps: Vec<f32> = lines
        .windows(2)
        .map(|w| w[0].y - w[1].y)
        .filter(|g| *g > 0.0)
        .collect();
    gaps.sort_by(f32::total_cmp);
    let median_gap = gaps.get(gaps.len() / 2).copied().unwrap_or(0.0);
    let left = lines.iter().map(|l| l.x).fold(f32::INFINITY, f32::min);

    let mut blocks = Vec::new();
    let mut cur = String::new();
    let mut prev_y: Option<f32> = None;
    for l in &lines {
        let new_para = cur.is_empty()
            || prev_y
                .map(|py| median_gap > 0.0 && py - l.y > median_gap * 1.6)
                .unwrap_or(false)
            || l.x > left + l.font;
        if new_para {
            if !cur.trim().is_empty() {
                blocks.push(cur.trim().to_string());
            }
            cur = l.text.clone();
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(&l.text);
        }
        prev_y = Some(l.y);
    }
    if !cur.trim().is_empty() {
        blocks.push(cur.trim().to_string());
    }
    blocks
}

/// Letters-only, lowercased — for matching block text against big-font runs.
pub(crate) fn letters_only(s: &str) -> String {
    let lettered: String = s
        .chars()
        .map(|c| {
            if c.is_alphabetic() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    lettered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalised texts of runs whose font is ≥1.3× the page median (heading-sized).
fn big_font_texts(p: &PageContent) -> Vec<String> {
    if p.runs.is_empty() {
        return Vec::new();
    }
    let mut sizes: Vec<f32> = p.runs.iter().map(|r| r.font_size).collect();
    sizes.sort_by(f32::total_cmp);
    let median = sizes[sizes.len() / 2].max(1.0);
    p.runs
        .iter()
        .filter(|r| r.font_size >= median * 1.3)
        .map(|r| letters_only(&r.text))
        .filter(|s| s.len() > 2)
        .collect()
}

/// Does the page carry a large (≈ full-page) raster image? Signals a scan
/// rather than born-digital text.
fn has_full_page_image(doc: &Document, page_id: ObjectId) -> bool {
    let Some(dict) = doc.get_object(page_id).ok().and_then(|o| o.as_dict().ok()) else {
        return false;
    };
    let Some(resources) = dict
        .get(b"Resources")
        .ok()
        .and_then(|o| match o {
            Object::Reference(id) => doc.get_object(*id).ok(),
            other => Some(other),
        })
        .and_then(|o| o.as_dict().ok())
    else {
        return false;
    };
    let Some(xobjects) = resources
        .get(b"XObject")
        .ok()
        .and_then(|o| match o {
            Object::Reference(id) => doc.get_object(*id).ok(),
            other => Some(other),
        })
        .and_then(|o| o.as_dict().ok())
    else {
        return false;
    };
    for (_n, v) in xobjects.iter() {
        let Object::Reference(id) = v else { continue };
        let Ok(stream) = doc.get_object(*id).and_then(|o| o.as_stream()) else {
            continue;
        };
        let is_image = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok())
            .map(|n| n == b"Image")
            .unwrap_or(false);
        if !is_image {
            continue;
        }
        let area = stream
            .dict
            .get(b"Width")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0)
            * stream
                .dict
                .get(b"Height")
                .ok()
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0);
        if area > 400_000 {
            return true; // ~ a full page of pixels
        }
    }
    false
}

/// Classify the whole document by what fraction of pages carry a text layer.
pub(crate) fn classify(doc: &Document) -> InputKind {
    let pages = doc.get_pages();
    if pages.is_empty() {
        return InputKind::Scan;
    }
    const TEXT_FLOOR: usize = 100;
    let with_text = pages
        .values()
        .filter(|&&id| shown_text_bytes(doc, id) >= TEXT_FLOOR)
        .count();
    // A text layer on a healthy majority of pages → extract text directly.
    if with_text * 2 >= pages.len() {
        InputKind::TextLayer
    } else {
        InputKind::Scan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bfchar_cmap() {
        let cmap = b"begincmap
1 begincodespacerange <0000> <FFFF> endcodespacerange
2 beginbfchar
<0003> <0041>
<0004> <0042>
endbfchar
endcmap";
        let (map, code_len) = parse_to_unicode(cmap).unwrap();
        assert_eq!(code_len, 2, "2-byte codes inferred from 4-hex src");
        assert_eq!(map.get(&3).map(String::as_str), Some("A"));
        assert_eq!(map.get(&4).map(String::as_str), Some("B"));
    }

    #[test]
    fn parses_bfrange_cmap() {
        // single-dst range: 0x10..=0x12 → a, b, c (last unit increments)
        let (map, _) = parse_to_unicode(b"beginbfrange <0010> <0012> <0061> endbfrange").unwrap();
        assert_eq!(map.get(&0x10).map(String::as_str), Some("a"));
        assert_eq!(map.get(&0x11).map(String::as_str), Some("b"));
        assert_eq!(map.get(&0x12).map(String::as_str), Some("c"));
    }

    #[test]
    fn parses_bfrange_array_form() {
        let (map, _) =
            parse_to_unicode(b"beginbfrange <0020> <0021> [<0058> <005A>] endbfrange").unwrap();
        assert_eq!(map.get(&0x20).map(String::as_str), Some("X"));
        assert_eq!(map.get(&0x21).map(String::as_str), Some("Z"));
    }

    #[test]
    fn hex_helpers() {
        assert_eq!(hex_units("0041").unwrap(), vec![0x41]);
        // two UTF-16 code units → the "fi" ligature spelled out
        assert_eq!(hex_string("00660069").as_deref(), Some("fi"));
        assert_eq!(hex_u32("00ff"), Some(255));
        assert!(hex_units("zz").is_none());
    }

    #[test]
    fn empty_cmap_is_none() {
        assert!(parse_to_unicode(b"begincmap endcmap").is_none());
    }
}
