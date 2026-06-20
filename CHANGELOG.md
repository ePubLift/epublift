# Changelog

All notable changes to **epublift** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The CLI and the web service version independently (`cli-v*` / `web-v*`); entries
are tagged with the component they belong to.

## [Unreleased]

### Added (experimental)
- **EPUB 3.4 image codecs — AVIF & JPEG XL (`epub34` feature).** Behind a new
  opt-in build feature, `epublift -i book.epub --target 3.4` re-encodes images to
  **AVIF** (or `--image-format jxl` for **JPEG XL**), the two formats that become
  core media types in EPUB 3.4. Pure-Rust imazen codecs (`zenavif`, `zenjxl`)
  alongside the existing `zenwebp`; the image pipeline is now format-parameterised
  and the size-safe "never grow a book" guard applies to all formats. The default
  build is unchanged (EPUB 3.3 / WebP). Spec tracking + plan in
  [`docs/epub-3.4.md`](docs/epub-3.4.md).

## [web-v1.5.0] - 2026-06-19

### Added (web)
- **Archive & restore in the browser (`epublift-web`).** The web UI gains a
  three-way mode switch — **Optimize** (the existing convert flow), **Archive**,
  and **Restore** — bringing the CLI's `.eparc` archival to the hosted/self-hosted
  service. *Archive* packs an uploaded `.epub` into a compact, lossless `.eparc`
  (new `POST /archive`); *Restore* turns an `.eparc` back into a working `.epub`
  (new `POST /restore`) — **content-exact by default**, or with a **Modernize**
  toggle that re-runs the optimizer (EPUB 3.3, WebP, keep-images, `.kepub`). Like
  conversion, every request is processed **in memory and deleted immediately** —
  nothing is stored or logged in any mode. New strings are localized across all
  **13 languages**, and the hero copy now reflects all three capabilities.
- **ASCII output name for Archive.** The *ASCII filenames* toggle now also applies
  in Archive mode, transliterating the downloaded `.eparc` name (e.g. `Işık` →
  `Isik`) for older devices/filesystems; the archived bytes and the manifest's
  recorded source name are unchanged.

### Fixed (web)
- **Graceful shutdown on SIGINT/SIGTERM.** The server now stops cleanly on the
  first Ctrl-C and on `docker stop` (instead of being force-killed after the
  ~10 s timeout), so a foreground `docker run` no longer leaves an orphaned
  container holding port 8080.

## [cli-v1.4.1] - 2026-06-18

### Added
- **ARM64 Linux (Raspberry Pi) prebuilt binary** — a static
  `aarch64-unknown-linux-musl` archive, so the archival mode's headline target
  (Raspberry Pi 3/4/5 and arm64 NAS/servers on a 64-bit OS) is download-and-run,
  no longer build-from-source.
- **Debian / Ubuntu / Raspberry Pi OS `.deb` packages** (amd64 + arm64), built
  with `cargo-deb` and attached to the release — install with
  `sudo apt install ./epublift_1.4.1_arm64.deb`.
- **Fedora / RHEL / openSUSE `.rpm` packages** (x86_64 + aarch64), built with
  `cargo-generate-rpm` — install with `sudo dnf install ./epublift-1.4.1-1.x86_64.rpm`.

## [cli-v1.4.0] - 2026-06-18

### Added
- **Archival mode — `archive` / `restore` (`.eparc`).** Two new CLI subcommands
  shrink a personal EPUB library to save disk and bring any book back on demand.
  `epublift archive <epub|dir>` packs each book into a compact, **lossless**
  `.eparc` archive (recurses directories, one archive per book, prints a per-book
  and library-wide size summary); `epublift restore <eparc|dir>` returns it.
  The archive is a stored ZIP holding `manifest.json` + a single solid Zstandard
  stream (`data.zst`, level 19) of the compressible entries — text **and fonts** —
  plus already-compressed media (images, WOFF/WOFF2, audio, video) stored
  **verbatim**, so an archive **never grows a book** (text-heavy books shrink
  ~30%; image-heavy ones less). Integrity is a whole-file **SHA-256** plus a
  per-entry **CRC32** in the manifest. `restore` is **content-exact by default**
  (the original book, byte-for-byte per entry); the archive acts as a *canonical
  master* you can re-target on the way out — `--target 3.3` (only 3.3 today;
  EPUB 3.4 lands when that spec ships), `--modernize`, `--keep-images` (for
  non-WebP readers like Kobo), and `--kepub` all re-run the optimizer on the
  restored book. The existing `epublift -i book.epub` optimize behavior and all
  its flags are unchanged. Pure Rust (`structured-zstd`, `crc32fast`, `sha2`,
  `serde`/`serde_json`) — no C in any artifact. Ships by default; build with
  `--no-default-features` for a convert-only binary. See
  [`docs/design/eparc-format.md`](docs/design/eparc-format.md) and the codec
  rationale in [`docs/design/eparc-codec-choice.md`](docs/design/eparc-codec-choice.md).
- **Experimental Zstandard-OCF packaging (research track, opt-in).** A new
  default-off `zstd-experimental` build feature adds `--zstd` (plus
  `--zstd-level`, `--zstd-mode per-entry|shared-dict`, and `--zstd-decode` for
  the lossless round-trip), which packs the container with ZIP compression
  **method 93 (Zstandard)** instead of Deflate and writes a
  `*_zstd-experimental.epub`. This is **deliberately non-conformant** — it will
  not open in current reading systems — and exists only to *measure* what Zstd
  would save over Deflate for EPUB packaging, to back a future W3C `epub-specs`
  discussion. Two modes: **per-entry** (each entry independent — the
  conservative floor) and **shared-dict** (a dictionary trained from the book's
  own text entries, stored as `META-INF/zstd-dict.bin` — the cross-chapter win,
  explicitly non-standard). `shared-dict` is **size-safe**: it keeps the trained
  dictionary only when the result actually beats per-entry (mirroring the
  existing "never grow a book" image principle), so it is never larger than
  per-entry — capturing the win on large multi-chapter text books (up to ~−18%
  vs per-entry observed) and falling back otherwise. Pure Rust (`structured-zstd`,
  `crc32fast`) via a
  small self-contained ZIP writer; no C in any shipped artifact. A dev-only
  `zstd-bench` binary (and `zstd-c-bench` feature) compares the pure-Rust encoder
  against reference C `libzstd` for ratio and speed, reporting per-entry and
  shared-dict separately. Default builds and the hosted web service are
  unchanged. See
  [`docs/design/zstd-ocf-experimental.md`](docs/design/zstd-ocf-experimental.md).

## [web-v1.4.0] - 2026-06-17

### Added
- **Multi-language web UI (i18n).** The browser converter is now available in
  **13 languages** — English, Spanish, Turkish, German, French, Portuguese,
  Italian, Dutch, Polish, Russian, Japanese, Korean, and Chinese (Simplified).
  The interface **auto-detects the browser language** on first visit (falling
  back to English) and remembers your choice; a language selector sits in the
  top-right corner (a 文A glyph + the current language). Implemented CSP-safely in a
  same-origin `/i18n.js` (no inline scripts); adding a language is one dictionary
  block. Technical tokens (EPUB, WebP, Kobo, code snippets) stay in English.

## [1.3.0] - 2026-06-16

### Added
- **Keep original images (`--keep-images`).** Skips the JPEG/PNG → WebP step and
  leaves images untouched, while still upgrading the structure to EPUB 3.3. This
  exists because **Kobo e-ink readers do not render WebP** despite advertising
  EPUB 3.3 support (confirmed on Forma/Sage; the same WebP files render fine in
  Apple Books) — so a book converted with WebP shows blank images on Kobo.
  Available on the CLI (`--keep-images`) and as a "Keep original images" toggle
  in the web UI. **`--kepub` implies `--keep-images`**, since its whole purpose
  is Kobo.
- **Kobo `.kepub` output (`--kepub`).** A new opt-in target injects Kobo's
  `koboSpan` markup into the content documents — unlocking accurate page turns,
  reading statistics and dictionary lookup on Kobo devices — and names the
  output `<name>.kepub.epub`. The transform mirrors the open-source `kepubify`:
  sentence-level `koboSpan` wrapping (skipping `script`/`style`/`pre`/`audio`/
  `video`/`svg`/`math`), each image in its own paragraph span, the body wrapped
  in `div#book-columns > div#book-inner`, and a `kobostylehacks` style on
  `<head>`. It composes with the normal EPUB 3 upgrades, so a `.kepub.epub` is
  still a valid EPUB. Available on the CLI (`--kepub`) and as a "Kobo (.kepub)
  output" toggle in the web UI.

## [1.2.2] - 2026-06-15

### Fixed
- **Images no longer grow during conversion.** Re-encoding an already-compressed
  image to WebP could produce a *larger* file (common for low-quality charts and
  diagrams), inflating the whole book. Three changes fix this:
  1. **Keep the smaller file** — if the WebP isn't actually smaller than the
     source, the original image is kept untouched (no rewrite, no manifest or
     reference change). The output can never grow.
  2. **Never exceed the source quality** — for JPEG sources the WebP quality is
     capped at the JPEG's estimated quality, so a q43 chart is no longer
     re-encoded at q80 (which only added bytes, not detail).
  3. **Grayscale stays grayscale** — grayscale images are encoded as WebP `L8`
     instead of being expanded to RGB, dropping empty colour channels.

  Example: a chart-heavy book that previously grew 4.23 MB → 5.01 MB now shrinks
  to 3.67 MB with no visible quality change.

### Changed
- The text report's image table now reports per-image results as **converted**
  or **kept**, with a summary count, and the section is retitled *Image
  Optimization Breakdown*.
- Refreshed dependencies to their latest compatible versions, and bumped
  `tower-http` 0.6 → 0.7 (web service). No behaviour change for epublift —
  `zenwebp` is already at the latest 0.4.4.

## [1.2.1] - 2026-06-15

### Added
- **Content-Security-Policy on `epublift-web`.** Every response now carries a
  strict CSP (`default-src 'none'`, plus `base-uri`/`form-action`/`object-src`
  `'none'` and `frame-ancestors 'none'`). The front-end script was moved out of
  the page into its own `/app.js`, so scripts run under `script-src 'self'` with
  no inline JS; `'unsafe-inline'` is granted to styles only, and fonts are
  limited to Google Fonts.
- **Documented egress-blocking opt-in in `docker-compose.yml`.** The converter
  makes no outbound connections, so `docker-compose.yml` now documents (with
  ready-to-use config) how to put it on an `internal` Docker network to cut its
  internet access entirely — the reverse proxy joins that network and reaches
  the service by name. The default still publishes a port so `docker compose up`
  works out of the box.

## [1.2.0] - 2026-06-10

### Added
- **Hosted web service (`epublift-web`).** A new pure-Rust Axum service — a Cargo
  workspace member over the `convert()` library — that converts EPUBs in the
  browser: drag-and-drop upload, quality slider, ASCII toggle, an in-page result
  report, and a downloadable `.txt` audit report. Uploads are processed in memory
  and deleted immediately; nothing is stored or logged. Ships as a hardened,
  multi-arch Docker image on GHCR (`ghcr.io/epublift/epublift-web`) with a
  `docker-compose.yml` for one-command self-hosting. Hardened with per-IP rate
  limiting, body-size/time limits, a concurrency cap, locked-down CORS, and
  security headers.

### Changed
- **Input hardening (CLI + library).** Extraction now rejects zip-bombs (caps on
  entry count and total uncompressed size, enforced against bytes actually
  written), and image decoding enforces dimension/allocation limits against
  decode-bombs. These bounds sit far above any real e-book, so normal
  conversions are unaffected.

## [1.1.0] - 2026-06-10

Completes the **Distribution & foundation** milestone. No change to conversion
behavior since 1.0.3.

### Added
- **Integration test corpus** (`tests/convert.rs`): end-to-end tests that build
  legacy EPUB 2 fixtures and run them through the `convert()` API, asserting on
  the returned `Report` and the produced EPUB (image conversion, reference
  rewrites, `nav.xhtml` generation, OPF upgrade, DOCTYPE modernization, hybrid
  `toc.ncx` retention, and output naming).

### Notes
- This minor release rolls up the foundation work shipped across 1.0.1–1.0.3:
  cross-platform release binaries, the CI pipeline, and the extraction of the
  conversion pipeline into a public library. The library API added in 1.0.3
  (`convert`, `Options`, `Report`, `EpubVersion`) is the stable base for the
  planned desktop GUI.

## [1.0.3] - 2026-06-10

Foundation release. The conversion pipeline is now a reusable library; the CLI
is a thin front-end over it. CLI behavior is unchanged apart from one minor
output note below.

### Added
- **Public library API.** The crate now exposes `convert(input, &Options,
  progress) -> Report`, along with `Options`, `Report`, and an `EpubVersion`
  enum (with `LATEST` and a version tag that drives the `_v3.3` output name).
  This is the foundation for the planned desktop GUI and for richer end-to-end
  tests, and it lets other Rust programs embed ePubLift directly.

### Changed
- Extracted the pipeline from `main.rs` into `src/lib.rs`; the CLI now just
  builds `Options`, calls `convert`, and renders the returned `Report`. No
  change to the conversion result.
- The core library no longer writes to the console directly — progress and
  warnings are delivered through a caller-supplied callback. As a result, the
  few non-fatal warnings that previously went to **stderr** now render on the
  CLI's **stdout** alongside the rest of the progress output.

## [1.0.2] - 2026-06-10

Internal / tooling release. **No functional or user-facing changes** — the
binaries are functionally identical to 1.0.1.

### Added
- **Continuous integration.** A GitHub Actions CI workflow now runs
  `cargo fmt --check`, `cargo clippy -D warnings`, and the test suite on every
  pull request and push to `main`, complementing the tag-triggered release
  workflow.

### Changed
- Applied `rustfmt` and resolved all `clippy` lints across the codebase
  (collapsible let-chains, `is_multiple_of`, single-line `if`/`else`). Purely
  cosmetic; no behavior change, and the existing unit tests still pass.

## [1.0.1] - 2026-06-10

Distribution and documentation release. **No functional code changes** — the
binary is built from the same source as 1.0.0.

### Added
- **Cross-platform pre-built binaries.** A GitHub Actions release workflow now
  builds and publishes binaries on tag push for Linux (x86_64, static musl),
  Windows (x86_64), and macOS (Apple Silicon + Intel), each as an archive with a
  SHA256 checksum. Previously only a locally built macOS arm64 binary existed.
- **README install instructions** for downloading a pre-built binary, plus a
  first-run note covering the macOS Gatekeeper and Windows SmartScreen prompts
  that appear for the unsigned binaries.

### Notes
- The macOS and Windows binaries are **not yet code-signed or notarized**; see the
  README for the one-time steps to allow them. The Linux binary runs without any
  such prompt.

## [1.0.0] - 2026-06-09

First stable release. epublift upgrades EPUB files to **EPUB 3.3** while staying
backward-compatible with older EPUB 2 readers, and converts raster images to WebP
to shrink file size.

### Added
- **EPUB 3.3 modernization**: generates an EPUB 3.3 `nav.xhtml` navigation
  document while retaining the legacy `toc.ncx` and OPF guide pointers, producing
  a hybrid file that opens on both vintage EPUB 2 and modern EPUB 3.3 readers.
- **WebP image conversion**: JPEG/PNG manifest images are re-encoded to WebP at a
  configurable quality (`-q`, 1–100, default 80), with manifest media types and
  all document references rewritten to match.
- **Version-stamped output names**: optimized files are saved as
  `<input_name>_v3.3.epub` so the target EPUB spec version is visible at a glance
  (helps distinguish from future v3.4 output that older readers may not open).
- **`--ascii` flag**: optionally transliterates auto-generated output/report
  names to ASCII (e.g. `Işık Doğudan Yükselir` → `Isik_Dogudan_Yukselir`) for
  shell- and FAT32-friendly filenames. Off by default; original Unicode names are
  preserved otherwise.
- **Audit report**: per-image and total size savings are written to a report file
  (`-r`, default `<input_name>_report.txt`).
- **`gen-sample` companion binary**: builds a legacy EPUB 2.0 file with test
  images and outdated structures for safe end-to-end evaluation.

### Changed
- WebP encoding now uses the pure-Rust [`zenwebp`](https://crates.io/crates/zenwebp)
  crate instead of the C `libwebp` bindings, making epublift a **fully pure-Rust
  build** with no C compiler or system libraries required.

### Notes
- Built and tested with Rust 1.94+ (MSRV).
- Pure-Rust dependency stack: `zenwebp` (WebP), `zip`/`zlib-rs` (packaging),
  `quick-xml` + `roxmltree` (OPF/NCX), `image` (JPEG/PNG decode), `any_ascii`
  (transliteration).

[1.2.0]: https://github.com/ePubLift/epublift/releases/tag/v1.2.0
[1.1.0]: https://github.com/ePubLift/epublift/releases/tag/v1.1.0
[1.0.3]: https://github.com/ePubLift/epublift/releases/tag/v1.0.3
[1.0.2]: https://github.com/ePubLift/epublift/releases/tag/v1.0.2
[1.0.1]: https://github.com/ePubLift/epublift/releases/tag/v1.0.1
[1.0.0]: https://github.com/ePubLift/epublift/releases/tag/v1.0.0
