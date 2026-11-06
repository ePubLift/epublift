# PDF → EPUB import (experimental)

This document specifies ePubLift's **`import`** feature: turning a PDF into a
reflowable EPUB. It covers the CLI (`import` subcommand) and the web **Import
PDF** mode. The same `import` command also accepts **Markdown** input (routed by
file extension) — see the [Markdown import guide](markdown-import.md).

Status: **experimental** — text extraction (behind the `pdf` build feature)
works well for PDFs that carry a text layer; scanned-only PDFs (no text layer)
are handled by **OCR** behind the heavier opt-in `pdf-ocr` feature (pure Rust;
the models download on first use). OCR is best-effort — see
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
3. **Pure scan, no text layer** → **OCR**: ocrs finds the lines and
   PaddlePaddle's PP-OCRv5 Latin model reads them, pure Rust on rten. Behind the
   `pdf-ocr` build feature; the 10.5 MB of models download on first use. See
   [Accuracy](#accuracy).

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
first run downloads the OCR models (10.5 MB) to a cache dir — set
`EPUBLIFT_OCR_MODELS` to use a directory you've pre-populated instead, with
ocrs's `text-detection.onnx` and PaddlePaddle's Latin recognition model saved as
`latin_PP-OCRv5_mobile_rec.onnx` (paginaverto's `LATIN_RECOGNITION_URL` gives the
pinned address). A cache from before cli-v4.5.2 holds the detection model as
`text-detection.rten`, which keeps working.

`import` warns about pages it could not read (OCR failed, an image that could not
be decoded, or a page that is only an image in a PDF read from its text layer)
and names them; the rest of the book is written.

The book's title is the one the PDF gives itself (its document properties),
unless that is empty or what a program left there (a file name, "Microsoft
Word - …", "Untitled"). Then it comes from the file name, read the common way:
in `Title (Author) (site.one, site.two).pdf` the groups naming web sites are
dropped, the last group left is the author, `_ ` stands for `: `, and a
download's trailing numbers (`[#1829082]-5445572`) go. The author is the file
name's, else the PDF's own unless that names a computer or account ("Lenovo",
"User"). `epublift meta set --title` / `--author` change them afterwards.

The book has a small style sheet: indented, justified and hyphenated
paragraphs, and figures that never overflow the screen. A reader's own
settings still win.

## Accuracy

Measured with paginaverto 0.3 on two Turkish books, against a reference:

- **Text layer (tiers 1–2):** on a 166-page novel, 98.85% of the words line up in
  order with Mistral OCR's reading of the same PDF, with 2,297 paragraphs
  against its 2,458 (some are merged). On a born-digital test book the text
  matched the publisher's own EPUB at ~99.8%.
- **OCR (tier 3):** on a 292-page scan, 88.4% of the words come out exactly
  right, in order, against the publisher's EPUB, with 2,220 paragraphs against
  its 2,061. Turkish letters come through, the capital `İ` too (983 of 1,059),
  when `--language tr` is given: OCR uses the language to choose between `İ`
  and `I`, `ı` and `i`. It took 234 s on an Apple M2 Pro. Quality drops on
  phone-photo, skewed, low-contrast or artistic pages, and lines that slant are
  cropped straight.
- **OCR reads Latin-script alphabets only.** A scan in Cyrillic, Greek, Arabic,
  Hebrew, an Indic script or CJK comes out wrong or missing, and so do the few
  Latin-script languages whose letters the model lacks (Azerbaijani `ə`,
  Vietnamese, Uzbek). `import` warns when its `--language` is one of them
  (paginaverto's `recognises` decides). [Smart Import](smart-import.md) handles
  more, but it sends the book to Mistral.

OCR is for getting readable, reflowable text out of an otherwise-unreadable image
PDF, not a faithful reproduction.

## Web usage

In the web UI, pick the **Import PDF** mode, drop a `.pdf`, choose the book's
language, and convert. The result reports the chapter and paragraph counts, the
pages that could not be read (as the CLI's warnings), and a one-time download
link. A scan without a text layer is read by OCR, up to 50 pages, so that it
finishes inside the web's 120-second request limit; a longer one is refused
before OCR starts, and the CLI (no limit) or [Smart Import](smart-import.md)
reads it. The Docker image carries the OCR models, checked by SHA-256 at build
time, so the service never downloads them. The hosted service processes the
upload in memory and deletes it immediately, like every other mode.

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
  headings detected (font size on born-digital pages, ALL-CAPS otherwise).
  Chapters follow the PDF's own outline when it is a real table of contents,
  else a printed contents page, else the headings; a heading inside a chapter
  becomes `h2` to `h6`. Italic text stays italic (text layers only).
- **Figures** from born-digital books are carried into the EPUB — JPEG images
  verbatim, raw images re-encoded to PNG — placed per page (after that page's
  text). JBIG2, JPEG 2000 and CCITT figures are decoded too; indexed-colour and
  CMYK raw figures are skipped, and images under 4,096 pixels stay out, as do
  rules, specks and near-empty boxes. In a scan with a text layer, a page that
  is only an image (a cover, a plate, an end paper) is kept as a figure unless
  it is blank or holds only a stamp.
- **OCR** (`pdf-ocr` feature) for scanned PDFs with no text layer, including
  JBIG2 and JPEG 2000 page images; Latin-script alphabets only. See
  [Accuracy](#accuracy).

**Doesn't yet (known limits, honest):**

- **Scanned PDFs in the `pdf`-only build** (no `pdf-ocr`) → reported, not
  converted: you get a clear "OCR is needed" message, not a broken file.
- **A PDF whose text still decodes to garbage** (a font encoding neither
  extractor can read) → detected by a quality gate and refused with a clear
  message rather than emitting a broken EPUB. Object-stream PDFs that used to
  land here now import (since cli-v2.0.0 / web-v1.17.0).
- **Paragraphs can come out coarse** on some born-digital PDFs: the text is
  complete and correctly spaced, but several source paragraphs may share one.
- **Footnotes, lists, captions, tables and equations** are not yet told apart:
  they come through as paragraphs. Figure **placement** is per-page (approximate), not at the
  exact original position, and the cover image appears as the first figure rather
  than as EPUB cover metadata.
- **Front-matter heading noise**: scholarly scans with long bibliographies can
  produce a few spurious chapter entries. Cosmetic; the body text is unaffected.

## Build features

- **`pdf`** — the text tiers (1–2). Pure-Rust; light: adds
  [paginaverto](https://github.com/veripublica/paginaverto) without its OCR.
- **`pdf-ocr`** — adds OCR for tier 3 (paginaverto's `ocr`: pure-Rust ocrs +
  rten, plus our rustls model downloader). Heavier; opt-in. Models (10.5 MB)
  download on first use and cache (override the location with
  `EPUBLIFT_OCR_MODELS`); each load is checked against the SHA-256 paginaverto
  publishes, and a file that doesn't match is refused.

## How it works (brief)

Steps 1–3 are [paginaverto](https://github.com/veripublica/paginaverto)'s, the
veripublica family's PDF reader; epublift writes the EPUB (step 4).

1. **Classify** the input by whether pages carry a text layer (checked via the
   content stream's text-showing operators, which is robust where naive text
   extraction fails).
2. **Extract** text. For tiers 1–2 with clean text, the text comes from the
   page's text layer. For composite fonts we decode codes through the font's own
   `/ToUnicode` CMap and track each run's exact end position from glyph widths,
   so inter-word spaces land correctly.
3. **Structure**: strip recurring running heads/feet, de-hyphenate, reassemble
   paragraphs, detect headings and their levels, split chapters at the PDF's
   outline or contents page.
4. **Write** a minimal, valid EPUB 3.3.

Everything is pure-Rust and offline; no upload leaves your machine on the
self-hosted web service (its image carries the OCR models), and the CLI touches
the network only to download the OCR models once.
