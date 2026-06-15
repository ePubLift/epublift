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

- [x] **Cross-platform release binaries.** *(shipped v1.0.1)* v1.0.0 shipped
      macOS arm64 only; added Linux (x86_64), Windows (x86_64), and macOS x86_64.
- [x] **CI pipeline** (GitHub Actions): *(shipped v1.0.2)* build + `cargo test` +
      `clippy` + `fmt` on every PR; build release tarballs on tag.
- [x] **Extract the core into a library** (`src/lib.rs`): a stable
      `convert(input, options) -> Report` API so the CLI is a thin front-end.
      Prerequisite for the GUI and for richer integration tests. No behavior
      change. Shape the `Options` struct so a future `target_version` field
      slots in cleanly, and derive the version-stamped output name (`_v3.3`)
      from that value rather than hardcoding it — so target-version selection
      (v2.0) lands without an API break.
- [x] **Integration test corpus**: a small set of EPUB 2 fixtures verified
      end-to-end against the new library API (`tests/convert.rs`).

---

## 🌐 Near term — v1.2: Hosted web service (`epublift-web`)

Goal: serve the non-technical users asking "how do I use the CLI?" *now*, with
zero install — a browser page that converts EPUBs — while the desktop GUI is
built. A new `epublift-web` workspace member wraps the v1.1 library; the public
instance runs at **epublift.itpax.net** (behind Nginx Proxy Manager, which
terminates TLS).

- [x] **Axum web service** over the `convert()` library API: upload an EPUB →
      convert in memory → return the file plus a result report. Pure-Rust
      (axum/tokio/tower/rustls-free since NPM does TLS); the conversion runs on
      `spawn_blocking` behind a concurrency semaphore. The converted file is held
      in RAM under a one-time token and streamed from `/download/{token}`.
- [x] **Front-end** (static, served by Axum): the editorial drag-and-drop design
      in `epublift-web/static/index.html` — quality **slider** + **ASCII** toggle
      + target-version pills mirroring the CLI options.
- [x] **Result report in the UI**: before/after size and savings up front, with
      an expandable EPUB 3.3 compliance checklist + per-image WebP breakdown
      (data straight from `Report`), plus a "Download report (.txt)" using
      `Report::write_text_report()`.
- [x] **No retention**: each request is processed in a temp dir and deleted
      immediately on success *or* error; no storage, no content logging.
- [x] **Abuse / attack hardening** (the source is public — no security by
      obscurity):
      - HTTP layer: request body-size limit (matched in NPM *and* Axum), request
        timeout, per-IP rate limiting (real IP via `X-Forwarded-For`, trusted
        only from NPM), CORS locked to the page origin, sanitized
        `Content-Disposition` filename, security headers — incl. a strict
        `Content-Security-Policy` (`default-src 'none'`; the front-end script is
        served from its own `/app.js` so `script-src 'self'` needs no inline JS;
        `'unsafe-inline'` granted to styles only; fonts limited to Google Fonts).
      - Input layer (library hardening, benefits the CLI too): caps zip
        extraction (total uncompressed size + entry count) against zip-bombs;
        sets `image` decode limits (max dimensions/allocation) against
        decode-bombs. Zip-slip is already guarded via `enclosed_name`.
      - Container: non-root, read-only root FS, `tmpfs` for temp, plus
        `mem_limit` / `pids_limit`. *(Egress-blocking left to the operator.)*
- [x] **Docker distribution**: a `Dockerfile` (static musl binary on a minimal
      base) + `docker-compose.yml` in the repo; CI builds and pushes a multi-arch
      image to **GHCR** (`ghcr.io/epublift/epublift-web`, `:latest` + `:vX.Y.Z`)
      on tag, so anyone can self-host with one command. AGPL-3.0: a visible
      "Source" link on the page satisfies the §13 network-use obligation.

---

## 🖥️ Mid term — v1.3: Desktop app (ePubLift GUI)

Goal: reach non-technical readers who will never open a terminal (the native,
offline counterpart to the v1.2 web service).

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
- [ ] **Target-version selection** (`--target-version`, short `-t`): default to
      the newest supported EPUB version, and let users opt into an older one for
      maximum reader compatibility — e.g. EPUB 3.3 (WebP) vs. EPUB 3.4 (AVIF/JXL,
      smaller but newer-reader-only), with output named accordingly
      (`_v3.3` / `_v3.4`). The selected version governs the per-version
      feature/codec set. Use `--target-version`, **not** `-v`/`--version`, which
      conventionally prints the program's own version.
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
