# PDF → EPUB import (experimental)

This document specifies ePubLift's **`import`** feature: turning a PDF into a
reflowable EPUB. It covers the CLI (`import` subcommand) and the web **Import
PDF** mode.

Status: **experimental** — shipped behind the opt-in `pdf` build feature. Text
extraction works well for PDFs that carry a text layer; scanned-only PDFs (no
text layer) need OCR, which is a later phase (`pdf-ocr`). We're shipping early to
gather real-world feedback — please report PDFs that convert badly.

## What it does

A PDF is not an e-book: it's fixed-page. `import` recovers the **text** and
re-flows it into a proper EPUB 3.3 (resizable, searchable). Real-world PDFs fall
into three tiers, and `import` detects which one it's looking at:

1. **Born-digital PDF** (has a real text layer) → extract the text directly.
   Lossless, fast, pure-Rust. *Best results.*
2. **Scan with an embedded OCR text layer** (archive.org, Google Books, most
   "searchable PDFs") → reuse that text layer. No re-OCR; quality is whatever
   the original OCR produced.
3. **Pure scan, no text layer** → needs OCR. **Not available yet** — `import`
   detects this and tells you OCR support is coming in a later release.

Tiers 1 and 2 are what the current `pdf` feature handles.

## CLI usage

```bash
epublift import -i book.pdf                 # → book.epub
epublift import -i book.pdf -o out.epub     # explicit output
epublift import -i book.pdf --language tr   # set the book's language (BCP-47)
```

Flags:

| Flag | Meaning |
|---|---|
| `-i, --input <PDF>` | the PDF to import (required) |
| `-o, --output <EPUB>` | output path (default: alongside the PDF) |
| `--language <code>` | content language (BCP-47, e.g. `tr`); sets `dc:language` |
| `--mode reflow\|fixed` | `reflow` (default). `fixed` (preserve page images) is **not implemented yet** |

The `import` subcommand only exists when the binary is built `--features pdf`
(the pre-built release binaries include it).

## Web usage

In the web UI, pick the **Import PDF** mode, drop a `.pdf`, choose the book's
language, and convert. The result reports the chapter and paragraph counts and a
one-time download link. The hosted service processes the upload in memory and
deletes it immediately, like every other mode.

## What works, and what doesn't yet

**Works:**

- Born-digital PDFs and searchable scans (tiers 1–2).
- Both simple (1-byte) fonts and composite **Type0 / CID** fonts (e.g. Identity-H
  with a `/ToUnicode` CMap) — common in modern, non-Latin PDFs.
- **Accurate word spacing** from the font's real glyph widths (`/Widths`,
  `/W`), not estimates. On a born-digital test book the converted text matched
  the publisher's own EPUB at **~99.8% word overlap**.
- Structure: running headers/footers and page numbers stripped, line-break
  hyphens re-joined ("in-\ncreased" → "increased"), paragraphs reassembled, and
  chapter headings detected (font-size on born-digital pages, ALL-CAPS
  otherwise) to build the spine + table of contents.
- **Figures** from born-digital books are carried into the EPUB — JPEG images
  verbatim, raw images re-encoded to PNG — placed per page (after that page's
  text). JPEG2000 / CCITT / JBIG2 / CMYK figures are skipped (no EPUB-core or
  pure-Rust path).

**Doesn't yet (known limits, honest):**

- **Scanned PDFs with no text layer** → OCR is a later phase; you get a clear
  "OCR coming" message, not a broken file.
- **Some PDF-1.5 object-stream PDFs** whose font objects the parser can't
  resolve → detected by a quality gate (if the decoded text is mostly garbage)
  and refused with a clear message rather than emitting a broken EPUB.
- **Tables and equations** are not yet preserved (text only; keeping them as
  images is planned). Figure **placement** is per-page (approximate), not at the
  exact original position, and the cover image appears as the first figure rather
  than as EPUB cover metadata.
- **Front-matter heading noise**: scholarly scans with long bibliographies can
  produce a few spurious chapter entries. Cosmetic; the body text is unaffected.

## Build features

- **`pdf`** — the text tiers (1–2). Pure-Rust (adds only `lopdf`); light.
- **`pdf-ocr`** — *reserved for the OCR phase* (will add a pure-Rust OCR engine +
  on-demand model download). Not functional yet.

## How it works (brief)

1. **Classify** the input by whether pages carry a text layer (checked via the
   content stream's text-showing operators, which is robust where naive text
   extraction fails).
2. **Extract** text. For tiers 1–2 with clean text, the text comes from the
   page's text layer. For composite fonts we decode codes through the font's own
   `/ToUnicode` CMap and track each run's exact end position from glyph widths,
   so inter-word spaces land correctly.
3. **Structure**: strip recurring running heads/feet, de-hyphenate, reassemble
   paragraphs, detect headings → chapters.
4. **Write** a minimal, valid EPUB 3.3.

Everything is pure-Rust and offline; no upload leaves your machine on the
self-hosted web service, and the CLI never touches the network.
