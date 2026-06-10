# Changelog

All notable changes to **epublift** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[1.0.1]: https://github.com/ePubLift/epublift/releases/tag/v1.0.1
[1.0.0]: https://github.com/ePubLift/epublift/releases/tag/v1.0.0
