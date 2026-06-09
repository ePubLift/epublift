# ePubLift Roadmap

This document outlines the direction of **ePubLift** — where it is today and what
comes next. It is a living plan: items may be reordered or rescoped as the EPUB
specification and the surrounding tooling evolve. Versioning follows
[Semantic Versioning](https://semver.org/).

## Vision

A fast, fully open, **pure-Rust** toolkit that modernizes EPUB files — upgrading
legacy structures to current standards and shrinking file size with modern image
codecs — usable both from the command line and, eventually, from a friendly
drag-and-drop desktop app. No C toolchain, no system libraries, no surprises.

---

## ✅ Shipped — v1.0.0 (2026-06-09)

First stable release.

- EPUB 2 → **EPUB 3.3** modernization with backward-compatible hybrid output
  (retains `toc.ncx` + OPF guide alongside generated `nav.xhtml`).
- JPEG/PNG → **WebP** conversion at configurable quality, with all references
  rewritten (CSS, XHTML, SVG, OPF manifest).
- **Version-stamped output names** (`<name>_v3.3.epub`).
- Optional **`--ascii`** transliteration of auto-generated filenames.
- Detailed **audit report** of per-image and total savings.
- **Fully pure-Rust** build (WebP via `zenwebp`; deflate via `zlib-rs`).

---

## 🔜 Near term — v1.1: Distribution & foundation

Goal: make ePubLift easy to *get* and easy to *build on*, without changing
behavior.

- [ ] **Cross-platform release binaries.** v1.0.0 shipped macOS arm64 only.
      Add Linux (x86_64), Windows (x86_64), and macOS x86_64.
- [ ] **CI pipeline** (GitHub Actions): build + `cargo test` + `clippy` + `fmt`
      on every PR; build release tarballs on tag.
- [ ] **Extract the core into a library** (`src/lib.rs`): a stable
      `convert(input, options) -> Report` API so the CLI is a thin front-end.
      Prerequisite for the GUI and for richer integration tests. No behavior
      change.
- [ ] **Integration test corpus**: a small set of real-world EPUB 2 fixtures
      verified end-to-end against the new library API.

---

## 🖥️ Mid term — v1.2: Desktop app (ePubLift GUI)

Goal: reach non-technical readers who will never open a terminal.

- [ ] **`epublift-gui`** — a native, drag-and-drop desktop app built on `egui`,
      consuming the core library directly (stays pure-Rust, single small binary,
      no Electron/runtime).
- [ ] Drop one or many EPUBs → convert → show before/after size and a result log.
- [ ] Quality slider and an "ASCII filenames" toggle mirroring the CLI flags.
- [ ] Packaged builds per platform (`.app` / `.exe` / AppImage).

---

## 🧬 Longer term — v2.0: EPUB 3.4 & next-gen codecs

Goal: support the formats EPUB 3.4 (draft) unlocks, without breaking older
readers.

- [ ] **AVIF and JPEG XL** image conversion via the imazen codec ecosystem
      (keeping the all-pure-Rust, single-vendor codec strategy started with
      `zenwebp`).
- [ ] **Target-version selection**: let users choose EPUB 3.3 (WebP, max
      compatibility) vs. EPUB 3.4 (AVIF/JXL, smaller but newer-reader-only),
      with output named accordingly (`_v3.3` / `_v3.4`).
- [ ] Per-image **codec auto-selection** based on content (photographic vs.
      flat/graphic) for best size at a given quality.

---

## 🔬 Experimental / research

Tracked but not committed to a release.

- [ ] **Zstd-for-EPUB measurement mode.** Following an inquiry to the W3C about
      allowing Zstandard alongside Deflate for EPUB packaging, add an opt-in
      `--zstd` *measurement* mode that reports the size delta Zstd would yield.
      Gated on the spec conversation; output would not be a conformant EPUB
      until/unless the spec permits it.
- [ ] Optional lossless re-optimization pass for images already in a modern
      format.

---

## Guiding principles

1. **Pure Rust, no C.** Every dependency stays C-free.
2. **Backward compatibility first.** Default output must open on the widest
   range of readers; newer-but-narrower formats are opt-in.
3. **Never mutate the input.** Originals are untouched unless the full pipeline
   succeeds.
4. **One core, many front-ends.** CLI and GUI share the same library.
