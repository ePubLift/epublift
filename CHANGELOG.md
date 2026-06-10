# Changelog

All notable changes to **epublift** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
