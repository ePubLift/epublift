# Markdown → EPUB import (experimental)

This document specifies ePubLift's **`import`** feature for **Markdown** input:
turning a CommonMark `.md` file (and its local images) into a reflowable EPUB.
It is the same `import` subcommand used for [PDF import](pdf-import.md) — the
input is routed by file extension.

Status: **experimental**, behind the `markdown` build feature. Pure-Rust and
fully offline (uses [`pulldown-cmark`](https://crates.io/crates/pulldown-cmark)).

## Why

A common workflow is to OCR a PDF with an AI tool (e.g. Mistral OCR) that emits
clean Markdown plus an `images/` folder — and then face the chore of "scripting
directories and a template to go from Markdown to EPUB". `import` does exactly
that step, locally, with no API and no network.

This is also the **output stage of "Smart Import"**: the optional, opt-in AI-OCR
layer produces Markdown, and this offline core turns it into an EPUB. The core
never depends on any cloud service.

## Usage

```sh
# routed by extension; .md / .markdown → this path
epublift import -i book.md -o book.epub --language en
```

| Flag | Meaning | Default |
| ---- | ------- | ------- |
| `-i, --input` | Path to the `.md` / `.markdown` file | *(required)* |
| `-o, --output` | Where to write the EPUB | alongside the input |
| `--language` | Content language (BCP-47, e.g. `tr`); sets `dc:language` | `en` |

(`--mode` is PDF-only and ignored for Markdown.)

## What it produces

A valid **EPUB 3.3** synthesised from scratch (the same writer the PDF import
uses). Chapters are split at **top-level `#` headings**; the first `#` becomes
the book title (falling back to the file name). Content before the first `#`
becomes a leading section.

### Supported Markdown

- Headings (`#`–`######`), paragraphs
- **Bold**, *italic*, ~~strikethrough~~, `inline code`
- Fenced & indented code blocks (fenced language → `class="language-…"`)
- Ordered/unordered lists, blockquotes, horizontal rules
- Tables (GitHub-style)
- Links
- **Local image embedding** — see below

The renderer walks the parser's event stream and emits **well-formed XHTML**
directly (CommonMark's HTML output is not valid XML, so we don't use it).

### Images

`![alt](path)` references are resolved **relative to the Markdown file's
directory**, embedded into the EPUB under `OEBPS/images/`, and re-pointed at the
packaged copy. Recognised types: PNG, JPEG, GIF, WebP, SVG, AVIF, JXL, BMP,
TIFF. A `http(s)://` image is left as an external reference (not downloaded, to
stay offline). If a local image can't be found or isn't a recognised type, it's
skipped with a warning and its alt text is kept so no content is lost.

## Limitations (v1)

- **Raw HTML** blocks/inline are dropped (to guarantee valid XHTML output).
- **Footnotes** and **task lists** are not enabled yet.
- **Math** is not rendered as MathML yet (source is kept as text if present).
- No CSS/theming yet — output uses the reader's default styles.

Please report Markdown that converts badly.
