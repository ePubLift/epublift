# PDF → EPUB import (experimental)

This document specifies ePubLift's **`import`** feature: turning a PDF into a
reflowable EPUB. It covers the CLI (`import` subcommand) and the web **Import
PDF** mode. The same `import` command also accepts **Markdown** input (routed by
file extension) — see the [Markdown import guide](markdown-import.md).

Status: **experimental** — text extraction (behind the `pdf` build feature)
works well for PDFs that carry a text layer; scanned-only PDFs (no text layer)
are handled by **OCR** behind the heavier opt-in `pdf-ocr` feature (pure-Rust
ocrs + rten; models download on first use). OCR is best-effort — see
[Accuracy](#accuracy). We're shipping early to gather real-world feedback —
please report PDFs that convert badly.

## What it does

A PDF is not an e-book: it's fixed-page. `import` recovers the **text** and
re-flows it into a proper EPUB 3.3 (resizable, searchable). Real-world PDFs fall
into three tiers, and `import` detects which one it's looking at:

1. **Born-digital PDF** (has a real text layer) → extract the text directly.
   Lossless, fast, pure-Rust. *Best results.*
2. **Scan with an embedded OCR text layer** (archive.org, Google Books, most
   "searchable PDFs") → reuse that text layer. No re-OCR; quality is whatever
   the original OCR produced.
3. **Pure scan, no text layer** → **OCR** (pure-Rust ocrs + rten). Behind the
   `pdf-ocr` build feature; the ~12 MB models download on first use. Best-effort
   quality — see [Accuracy](#accuracy).

Tiers 1 and 2 need only the light `pdf` feature; tier 3 (OCR) needs `pdf-ocr`.

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

The `import` subcommand exists when the binary is built `--features pdf` (the
pre-built release binaries include it). **OCR of scanned PDFs additionally needs
`--features pdf-ocr`** (also in the pre-built binaries); on a scanned PDF the
first run downloads the OCR models (~12 MB) to a cache dir — set
`EPUBLIFT_OCR_MODELS` to use a directory you've pre-populated instead.

## Accuracy

- **Born-digital / searchable scans (tiers 1–2):** essentially lossless — on a
  born-digital test book the text matched the publisher's own EPUB at ~99.8%.
- **OCR (tier 3):** best-effort. On a clean, flat prose scan, word accuracy is
  around **~92%** (expect the odd `i`↔`1`, case slips, or a dropped line);
  quality drops further on phone-photo, skewed, low-contrast, or artistic pages.
  OCR is for getting readable, reflowable text out of an otherwise-unreadable
  image PDF — not a faithful reproduction.

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
- **OCR** (`pdf-ocr` feature) for scanned PDFs with no text layer — pure-Rust
  ocrs + rten, with flat-field illumination correction for phone-photo scans.
  Best-effort (~92% on clean prose); see [Accuracy](#accuracy).

**Doesn't yet (known limits, honest):**

- **Scanned PDFs in the `pdf`-only build** (no `pdf-ocr`) → reported, not
  converted: you get a clear "OCR is needed" message, not a broken file. OCR is
  also skipped when a scan's page images are JPEG2000 (no pure-Rust decoder).
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
- **`pdf-ocr`** — adds OCR for tier 3 (pure-Rust ocrs + rten + a rustls model
  downloader). Heavier; opt-in. Models (~12 MB) download on first use and cache
  (override the location with `EPUBLIFT_OCR_MODELS`).

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
