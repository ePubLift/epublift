# EPUB 3.4 — spec tracking & experimental work

This is our **watch + experiment** doc for the next EPUB version. EPUB 3.4 is an
in-progress W3C Working Draft; we track it here, and as items firm up we aim to be
**the first to emit 3.4-conformant `.epub` (and `.eparc`) files** for them.

> Treat the numbers below as a *snapshot*. The authoritative, evolving source is
> the W3C change log — check it periodically and update the "Watch log" section.

## Watch log

| Last checked | Spec status | Notes |
| :--- | :--- | :--- |
| 2026-06-20 | EPUB 3.4 — **W3C Working Draft, 06 May 2026** | First capture. Full substantive `#change-log` transcribed verbatim below; latest substantive change dated **04-May-2026**. Headlines for us: **AVIF + JPEG XL** core image types; HTML syntax **removed** (XHTML-only); `application/x-font-ttf` added; SHA-1 phase-out caution. |
| 2026-06-21 | EPUB 3.4 — **W3C Working Draft, 06 May 2026** (unchanged) | Re-checked: same publication date, **no new published substantive changes** since 04-May-2026. Reviewed the full list for actionable engine work beyond AVIF/JXL (done) — found `source-of` → `pageBreakSource` was missing from our per-item plan; added below. |

**How to re-check** (do this periodically):

1. Open the change log: <https://www.w3.org/TR/epub-34/#change-log>
2. Compare the publication date / status against the latest row above.
3. If anything is new, add a row here (date, status, what changed) and fold the
   new items into "Notable changes" below.
4. Editor's (latest) drafts, often ahead of the `/TR/` snapshot:
   - Core: <https://w3c.github.io/epub-specs/epub34/authoring/>
   - Overview: <https://w3c.github.io/epub-specs/epub34/overview/>

## Reference documents

| Document | URL | Status (as of 2026-06-20) |
| :--- | :--- | :--- |
| EPUB 3.4 (core) | <https://www.w3.org/TR/epub-34/> | W3C Working Draft, 06 May 2026 |
| EPUB 3 Overview | <https://www.w3.org/TR/epub-overview-34/> | W3C Group Note, 09 Mar 2026 |
| EPUB Reading Systems 3.4 | <https://www.w3.org/TR/epub-rs-34/> | Working Draft |
| EPUB Annotations — Use Cases & Requirements | <https://www.w3.org/TR/epub-anno-ucr/> | Note |
| W3C blog — "EPUB 3.3 published, work begins on new features" | <https://www.w3.org/blog/2025/epub3-3-recommendations-published-work-begins-on-new-features/> | 2025 |

## Substantive changes since EPUB 3.3 (verbatim change log)

Transcribed verbatim from <https://www.w3.org/TR/epub-34/#change-log> on
2026-06-20. The change log is non-normative and lists **only substantive
changes** — those that could affect the conformance of EPUB publications. Newest
first, with the W3C issue/PR reference.

- **04-May-2026** — Fixed outdated section number reference to ZIP64 extensions in the ZIP application note. (issue 2993)
- **15-Apr-2026** — Added recommendation against using variable bitrate MP3 files with media overlays. (2978)
- **14-Apr-2026** — Added **Opus in MP4 container** as a core media type and added additional media type with codec for **AAC LC**. (2979)
- **29-Jan-2026** — Moved manifest fallbacks for content to the outdated features. (2900)
- **23-Jan-2026** — Added **JPEG XL as a core media type for images**. (2896)
- **18-Dec-2025** — Renamed "obsolete but conforming features" to **"outdated features"**.
- **18-Dec-2025** — Deprecated the `rendition:align-x-center` property. (2847)
- **18-Dec-2025** — Added `rendition:flow`, `rendition:orientation`, `rendition:spread` (and their override equivalents) to the outdated features list. (2841)
- **18-Dec-2025** — Removed references to reflowable documents being allowed in spreads; clarified that spread placement properties only apply to pre-paginated documents. (PR 2844)
- **18-Dec-2025** — Added support for **roll layouts** in the new fixed-layout section. (2791)
- **18-Dec-2025** — Added a new section to explain layout options and classified pre-paginated layouts under fixed layouts; reorganised the rendering vocabulary sections. (PR 2844)
- **11-Nov-2025** — **Removed support for the HTML syntax.** (TPAC resolution)
- **10-Oct-2025** — Added caution that **SHA-1 is being phased out**, so methods other than font obfuscation are advised for protecting fonts. (2807)
- **10-Oct-2025** — Created an "obsolete but conforming" classification and moved **font obfuscation**, the `collection` element, legacy package-document features, and the prefixed CSS properties to it. (2807)
- **06-Oct-2025** — Added **AVIF as a core media type for images**. (2794)
- **26-Jun-2025** — Moved the `xsd`, `msv`, and `prism` reserved prefixes to the deprecated features section. (2739)
- **05-Jun-2025** — Consolidated all deprecated features under the deprecated features section. (PR 2735)
- **04-Jun-2025** — Added support for the **Internationalization Tag Set (ITS)**. (2732)
- **04-Jun-2025** — Added the prefix URL for expanding terms from the EPUB Structural Semantics Vocabulary. (2733)
- **02-Jun-2025** — Added the `pageBreakSource` property to the meta properties vocabulary to replace `source-of`. (2714)
- **26-May-2025** — Added `application/x-font-ttf` to the list of core media types for identifying **TTF fonts**. (667)
- **08-Apr-2025** — Added early support for HTML syntax. (2715) *(later removed — see 11-Nov-2025)*
- **08-Apr-2025** — Moved the navigation-document-in-spine authoring guidance; clarified reading systems do not suppress list styling in the spine. (2687)
- **08-Apr-2025** — Replaced the redundant last-modified-date requirements with a note that the date/time is always in UTC. (2662)
- **19-Feb-2025** — Clarified that resources referenced from `script` elements are exempt. (2649)
- **19-Feb-2025** — Clarified that script modules, such as WebAssembly, fall under the existing exemption for resources not used by reading systems. (2649)
- **19-Feb-2025** — Clarified that embedded resources only refers to the HTML and SVG definitions. (2656)

> **Not in this list:** *Annotation exchange* (the EPUB Annotation JSON format) is
> a **separate deliverable** (see the EPUB Annotations work), not a core-spec
> substantive change — the Overview note groups it under the 3.4 era, but it does
> not appear in the core change log above. Treat it as adjacent, not core 3.4.

### What matters to us (highlights)

- **Images → AVIF (06-Oct-2025) + JPEG XL (23-Jan-2026)** are now *core* media
  types (fallback-free). This is the headline work for the engine.
- **HTML syntax removed (11-Nov-2025)** → 3.4 is **XHTML-only**. We already emit
  XHTML, so this *validates* our existing output and our archival stance.
- **`application/x-font-ttf` added (26-May-2025)** → TTF is a recognised core
  font type; relevant to how we classify/handle fonts.
- **SHA-1 phase-out caution + font obfuscation moved to "obsolete but
  conforming" (10-Oct-2025)** → aligns with our choices: `.eparc` uses
  **SHA-256**, and we add no font obfuscation / DRM.
- **Roll layout (18-Dec-2025)** → new fixed-layout mode (webtoons / continuous
  vertical scroll) to preserve, not break, when modernising.
- **Deprecations / "outdated features"** (manifest fallbacks, several
  `rendition:*` properties, `collection`, legacy package features, prefixed CSS)
  → when emitting `--target 3.4`, prefer the current forms and don't introduce
  outdated ones.
- **Audio: Opus-in-MP4 + AAC LC (14-Apr-2026)** → pass through / store verbatim;
  we don't transcode audio.

## What this means for us — experimental plan

We treat each item by where it belongs: the deterministic transform engine
(`epublift` → `.epub` and the `.eparc` archival format), or explicitly *not* our
engine's job.

| 3.4 item | `.epub` (convert) | `.eparc` (archive) | Priority |
| :--- | :--- | :--- | :--- |
| **AVIF + JPEG XL** (core image types) | New encode targets in the image pipeline (pure-Rust [imazen] codecs), size-safe like WebP today. A `--target 3.4` emits AVIF/JXL instead of WebP. | **P2 media re-pack/transcode** unblocks: JPEG → **JXL-lossless-JPEG** (≈20% smaller, bit-exact JPEG reconstructable) as the *faithful* archival default; opt-in `--lossy-images` for AVIF/JXL compact masters. | **High** — the headline 3.4 work. |
| **HTML syntax removed → XHTML-only** | We already emit XHTML; this *validates* current output. Ensure `--target 3.4` never produces HTML-syntax docs and our validation rejects them. | No change (content-exact preserves whatever the source had). | Low (already compliant; add a guard/test) |
| **`application/x-font-ttf`** (TTF core type) | ✅ **DONE.** Font media-type hygiene: a `.ttf` manifest item with a legacy/non-core/missing media type (e.g. `application/x-font-ttf`, which was non-core before 3.4, or `application/octet-stream`) is normalized to **`font/ttf`** — the modern core type valid in 3.3 *and* 3.4. Applied during all modernization (a 3.3 conformance fix, not 3.4-gated); already-core types (`font/ttf`, `application/font-sfnt`) are left alone. | Fonts fold into the compressed stream; classification correct. | Done (`opf.rs`). |
| **SHA-1 phase-out / font obfuscation → "obsolete but conforming"** | We add no font obfuscation / DRM, so nothing to remove. | `.eparc` already uses **SHA-256** for fixity — aligned; no SHA-1 anywhere. | Low (already aligned) |
| **`pageBreakSource` replaces `source-of`** (meta property) | ✅ **DONE.** When emitting `--target 3.4`, derive a `<meta property="pageBreakSource">value</meta>` from a legacy `<meta refines="#id" property="source-of">pagebreak</meta>` (resolving the `refines` to its `dc:source` value). The legacy `source-of` is **kept** for back-compat (hybrid, like `toc.ncx`); skipped if a `pageBreakSource` already exists. Not added for 3.3 (out of vocabulary). | No change (content-exact). | Done (`opf.rs`). |
| **Deprecated / "outdated" features** (manifest fallbacks, several `rendition:*`, `collection`, deprecated `xsd`/`msv`/`prism` prefixes) | ✅ **Reported (lint).** We **detect and list** these in the audit report (an "OUTDATED / DEPRECATED FEATURES" section) but **don't strip** author content. Reliably OPF-detectable signals: manifest `fallback` attributes, the outdated `rendition:flow`/`orientation`/`spread`/`align-x-center` meta properties, the `collection` element, and deprecated reserved prefixes. (Prefixed-CSS detection — in content stylesheets — and font-obfuscation could be added later.) | Preserved verbatim on archive. | Done (lint; `opf.rs` + `report.rs`). |
| **Roll layout** (webtoons / continuous scroll) | Structural pass-through + validation; do not break FXL/roll metadata when modernising. | Stored faithfully; restore is content-exact. | Medium (test corpus) |
| **Opus-in-MP4 + AAC LC** (audio) | Pass through; we don't transcode audio. | Stored **verbatim** (already-compressed media). | Low |
| **Annotation exchange** *(separate deliverable, not core 3.4)* | **Not the engine's job** — annotations are a separate overlay referencing the immutable book. We only ensure we never clobber an annotations sidecar. | Preserve any annotation sidecar verbatim. | Low (awareness only) |

### Goal

Produce the **first** 3.4-conformant `.epub` outputs (AVIF/JXL images via
`--target 3.4`) and matching `.eparc` archives whose masters carry the modern
formats — turning the "convert shrinks images, archive shrinks text → they
compound" insight into a 3.4-native pipeline.

### Constraints we keep

- **Pure-Rust / C-free** shipped binary — AVIF/JXL via the [imazen] codec
  ecosystem, not C libraries.
- **Size-safe** — never grow a book; never re-encode when the result isn't
  smaller; never upscale quality.
- **Archive fidelity** — the archival master must be **≥ original quality**;
  lossy AVIF/JXL masters are forbidden as a default (generation loss). Lossless
  re-pack (JXL-lossless-JPEG, pixel-exact PNG) is the faithful path.

## Implementation status

**CLI experiment working (2026-06-20), behind the opt-in `epub34` feature.**

- Image codecs: pure-Rust **imazen** family — **`zenavif`** (rav1e/rav1d) for AVIF,
  **`zenjxl`** for JPEG XL — alongside the **`zenwebp`** we already ship, sharing
  the `zencodec` design. No C / FFI.
- New `EpubVersion::V3_4`; `LATEST` stays 3.3 (3.4 is opt-in, not the default).
- The image pipeline is now format-parameterised (`ImageFormat::{WebP,Avif,Jxl}`):
  per-format extension, manifest `media-type`, and encoder dispatch. The
  **size-safe** guard is unchanged — a re-encode is only kept when it's actually
  smaller, so a book never grows; otherwise the original is kept.
- CLI: `epublift -i book.epub --target 3.4` emits **AVIF**;
  `--target 3.4 --image-format jxl` emits **JPEG XL**. Default build is unchanged
  (3.3/WebP); `--target 3.4` without the feature errors cleanly.

```bash
# build with the experimental codecs
cargo build --release --features epub34
epublift -i book.epub --target 3.4                    # AVIF images
epublift -i book.epub --target 3.4 --image-format jxl # JPEG XL images
```

First validation (synthetic book): JPEG cover re-encoded to a valid AVIF
(`ftypavif`) and JPEG XL (`ff0a` codestream) with correct manifest media types;
a tiny PNG was left untouched by the size-safe guard.

**Calibrated codec comparison (done):** at equal perceptual quality the best codec
is **content-dependent** — WebP wins on diagram/line-art books, AVIF on
photographic ones. So AVIF must **not** be a blind `--target 3.4` default; the
source format (PNG vs JPEG) is a free content-type signal. Full methodology, the
two-book data, and the `img-calib` bench are in
**[`design/epub-3.4-image-codec-choice.md`](design/epub-3.4-image-codec-choice.md)**.

**Source-format heuristic wired (done):** `--target 3.4` now picks per image —
**JPEG → AVIF, PNG → WebP** (`--image-format avif|jxl` forces one format). This
already avoids the line-art→AVIF disaster (a diagram book stays WebP: 20.5 MB /
16 s vs forced AVIF 23.3 MB / 4 min).

**Quality calibration wired (done):** `--quality N` is the WebP reference scale,
mapped per codec (fit over 48 images / 11 books) so equal N ≈ equal perceptual
quality (butteraugli). At matched quality, `--target 3.4` is **≈0–11% smaller**
than 3.3 on photo books (historical photos most). Before calibration, raw q80
over-delivered quality and 3.4 was *larger* than 3.3.

**Per-image "smallest" mode (done): `--image-format best`** encodes every
candidate per image and keeps the smallest (valid because quality is calibrated).
It beats both fixed modes by correcting the heuristic's per-image misroutes — on
the photo book it picked 13 WebP + 3 AVIF (839 KB, vs 842 WebP-only / 847
AVIF-Auto) — at the cost of multiple encodes per image. Next: `restore` / web.

## Related

- Image-codec direction: see the project's notes on the imazen pure-Rust codecs.
- Archival format & the P2 media plan: [`docs/design/eparc-format.md`](design/eparc-format.md).
- Roadmap: [`../ROADMAP.md`](../ROADMAP.md).
- Zstandard-for-OCF research (separate track): [`docs/zstandard-research.md`](zstandard-research.md).

[imazen]: https://github.com/imazen
