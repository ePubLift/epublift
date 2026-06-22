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

## ✅ Shipped — v1.1: Distribution & foundation

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

## ✅ Shipped — v1.2: Hosted web service (`epublift-web`)

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
        `mem_limit` / `pids_limit`. The converter makes no outbound
        connections, so `docker-compose.yml` documents a ready-to-use
        egress-blocking opt-in (an `internal` Docker network the reverse
        proxy joins); the simple default keeps a published port.
- [x] **Docker distribution**: a `Dockerfile` (static musl binary on a minimal
      base) + `docker-compose.yml` in the repo; CI builds and pushes a multi-arch
      image to **GHCR** (`ghcr.io/epublift/epublift-web`, `:latest` + `:vX.Y.Z`)
      on tag, so anyone can self-host with one command. AGPL-3.0: a visible
      "Source" link on the page satisfies the §13 network-use obligation.

---

## ✅ Shipped — v1.3.0 (2026-06-16): Kobo (`.kepub`) support

Goal: produce Kobo-optimised `.kepub.epub` output, so books render faster and
gain Kobo's reading features (statistics, page turns, dictionary) on Kobo
devices. This is pure **core** work — it benefits the CLI *and* the already-live
web app in one change, with **no per-platform burden** — which is why it was
prioritised ahead of the desktop GUI.

- [x] **kepub conversion** — inject Kobo's `koboSpan` markup into the content
      HTML (reusing the existing XHTML processing pipeline) and emit a
      `.kepub.epub`. Reference: the open-source `kepubify` transformation.
      *(Implemented in `src/kepub.rs`: sentence-level spans, image paragraphs,
      `book-columns`/`book-inner` wrapper, `kobostylehacks` style.)*
- [x] A **target/output selector** — a `--kepub` flag (kept orthogonal to the
      future `--target-version` so "3.4 + kepub" stays expressible), with a
      matching **"Kobo (`.kepub`)"** toggle in the web UI.
- [x] Validated on a **real Kobo device** (Forma + Sage): images render, page
      turns are fast, and the font/size controls work.
- [x] **`--keep-images`** (unplanned, found during device testing): Kobo e-ink
      does **not** render WebP despite advertising EPUB 3.3, so a WebP book shows
      blank images. `--keep-images` keeps the original JPEG/PNG (still upgrading
      structure); `--kepub` forces it. The same WebP output renders fine in Apple
      Books, so the files are correct — it's a Kobo decoder gap.
- [ ] *(Optional, later)* Compare against Calibre's KePub output for parity.

---

## 🖥️ Demand-gated — Desktop app (ePubLift GUI)

Deferred from v1.3. The hosted web service already covers the no-install case,
and a desktop GUI carries a **recurring per-platform cost** (three OSes,
installers, code signing). Rather than build it speculatively, we prioritise it
on real demand signals — GitHub stars, feature requests/issues, community
feedback on the web launch.

> **Demand so far points elsewhere.** Early requests cluster around
> **library-scale batch** work and **pipeline pre-processing** (bulk
> optimize/archive, cover/image stripping, metadata repair, clean text/metadata
> extraction for downstream tools) — not a prettier single-file converter, which
> the web app already is. So the GUI stays demand-gated; the more likely next
> front-end work is batch/library-oriented.

- [ ] **`epublift-gui`** — a native, drag-and-drop desktop app built on `egui`,
      consuming the core library directly (stays pure-Rust, single small binary,
      no Electron/runtime).
- [ ] Drop one or many EPUBs → convert → show before/after size and a result log.
- [ ] Quality slider and an "ASCII filenames" toggle mirroring the CLI flags.
- [ ] Packaged builds per platform (`.app` / `.exe` / AppImage).

---

## ✅ Shipped — Archival mode (`.eparc`) — cli-v1.4.0 → 1.4.1, web-v1.5.0

Goal: shrink a personal EPUB library to save disk, and get any book back on
demand — **losslessly**. Not originally on this roadmap; it emerged from the
Zstandard research + a W3C-philosophy discussion (the archival case *removes* the
container-conformance constraint, since we control both ends).

- [x] **`archive` / `restore` subcommands** → a compact `.eparc` (stored ZIP of
      `manifest.json` + one solid Zstandard stream of the text + fonts, with
      already-compressed media stored verbatim, so an archive **never grows a
      book**). Whole-file SHA-256 + per-entry CRC32 for integrity.
- [x] **Content-exact restore by default** (the original book, byte-for-byte per
      entry); the archive is a canonical master you can **re-target** on the way
      out (`restore --target 3.3`/`3.4`, `--keep-images`, `--kepub`).
- [x] **Distro reach** *(cli-v1.4.1)*: an ARM64 Linux (Raspberry Pi / NAS) static
      binary plus `.deb` (amd64/arm64) and `.rpm` (x86_64/aarch64) packages.
- [x] **In the browser** *(web-v1.5.0)*: Archive / Restore modes in `epublift-web`
      (stateless, in-memory), with a content-exact or Modernize restore.
- [ ] *(Later)* Library-level dedup + a corpus-trained shared dictionary;
      bit-exact `--exact` (byte-identical original `.epub`); P2 image re-pack
      (JPEG → JXL-lossless, see below).

---

## ✅ Shipped — EPUB 3.4 (experimental) — cli-v1.5.0 → 1.5.1, web-v1.6.0 → 1.6.1

Goal: support the formats EPUB 3.4 (W3C Working Draft) unlocks, without breaking
older readers. Behind the opt-in `epub34` build feature; tracked in
[`docs/epub-3.4.md`](docs/epub-3.4.md) for the 2027 spec release.

- [x] **AVIF and JPEG XL** image conversion via the imazen codec ecosystem
      (`zenavif`, `zenjxl` alongside `zenwebp` — all pure-Rust, single-vendor).
- [x] **Target-version selection** (`--target 3.3|3.4`, output named
      `_v3.3` / `_v3.4`); `restore --target 3.4` and a web **Target version**
      selector (3.3 / 3.4 experimental) too.
- [x] **Per-image content-adaptive codec selection** — the source format is a free
      content-type signal: **JPEG → AVIF, PNG → WebP** (measured at equal
      perceptual quality; AVIF wins on photos, WebP on line-art). Plus
      `--image-format best` (encode all candidates, keep the smallest) and a
      **butteraugli-calibrated** quality scale so `--quality N` means the same
      across codecs. Full data: [`docs/design/epub-3.4-image-codec-choice.md`](docs/design/epub-3.4-image-codec-choice.md).
- [ ] *(Later)* More calibration books / per-format speed tuning; `best` mode in
      the web; re-check the W3C change log as 3.4 firms up toward its 2027 release.

---

## 🚧 Planned — Metadata enrichment & editing — cli-v1.6.0 → web-v1.7.0

Goal: let anyone fix a book's metadata — fill what's missing from an online
catalogue by **ISBN**, or edit fields by hand — and write it back into the EPUB's
OPF correctly. This serves the repeated "metadata repair" demand signal (see the
GUI note above) and stays true to the product's character: a **stateless,
single-file tool**, not a library/catalogue. CLI first, then the web form.

Design & full field map: [`docs/metadata.md`](docs/metadata.md).

- [x] **Core OPF metadata writer** *(`src/meta.rs`)* — a from-scratch, pure-Rust
      writer that applies Dublin Core fields (title/subtitle, creators +
      `role`/`file-as`, publisher, date, description, subjects, ISBN via
      `dc:source`, series via `belongs-to-collection`) into the package document,
      **filling gaps by default** (no overwrite without `--overwrite`), always
      refreshing `dcterms:modified`. quick-xml streaming with a drop-plan: only the
      replaced elements (and their `refines`) change; everything else (unique-id,
      cover meta, unknown vocab) is preserved verbatim. UTF-8, no transliteration.
- [x] **CLI: `meta` subcommand** — `meta show` (read; `--json`), `meta set` (manual
      edit), `meta enrich --isbn` (auto-fill). Offline-first: `show`/`set` need no
      network and ship in the default build; `enrich` is behind the opt-in
      **`metadata`** feature. The input is never mutated (writes `<name>_meta.epub`).
- [x] **Provider abstraction** (`Http` trait, network-agnostic + unit-tested with
      fixtures) — **Open Library** done (`jscmd=data` + `/isbn/<isbn>.json` for
      language/work-link + `/works/<id>.json` for description). Google Books
      (`langRestrict`) and Amazon (regional TLD) slot in next.
- [x] **Language-aware (critical)** — resolves the book's language
      (`dc:language`/`--lang`), matches by ISBN-13, and **skips fields whose
      language ≠ the book's** (edition mismatch warns; English work-level
      subjects/description skipped on a non-English book) unless
      `--allow-foreign-meta`. ISO 639-2/3 → BCP-47 mapping.
- [x] **Pure-Rust TLS (no C)** *(`src/http.rs`)* — the HTTPS client is `rustls`
      with the **RustCrypto** crypto provider (no `ring`/`aws-lc`, no C toolchain)
      + `webpki-roots`, over a small hand-rolled HTTP/1.1 GET (redirects + chunked).
      Default build stays offline and C-free; only `--features metadata` pulls it.
- [x] **Web form** *(web-v1.7.0)* — a **Metadata** mode in `epublift-web`: drop an
      `.epub` → `/meta/read` populates an editable form → optional **Fetch from
      Open Library** by ISBN (`/meta/enrich`, language-aware suggestions) → **Save
      & download** (`/meta/write`). Stateless/in-memory like the other modes;
      reuses the core writer; the network call runs server-side on the pure-Rust
      TLS client. (i18n: English strings for now; full translations are a follow-up.)
- [ ] *(Later)* **Phase 2 fields** — classification (Dewey / LCC) and **EPUB
      Accessibility 1.1** metadata (`schema:accessMode`, `accessibilityFeature`,
      `accessibilityHazard`, `accessibilitySummary`, `dcterms:conformsTo`).
- [ ] *(Long-term, optional)* **Upstream contribution** — feed the skip/coverage
      reports back to **Open Library** to improve Turkish/Korean records over time
      (a team effort, parallel to the tool — not a prerequisite for shipping).

---

## 🔬 Experimental / research

Tracked separately from the shipping product.

- [x] **Experimental Zstandard OCF packaging** *(merged; research track).* An
      opt-in `--zstd` mode (ZIP method 93) that produces a reversible
      `_zstd-experimental.epub` and measures the size delta vs Deflate — both
      *per-entry* and with a *shared dictionary* (the cross-chapter win). Pure-Rust
      by default; a dev-only benchmark compares it against reference C `libzstd`.
      The measurements seeded a **live W3C discussion** ([epub-specs#3025]) and the
      `.eparc` archival format above. Design:
      [`docs/design/zstd-ocf-experimental.md`](docs/design/zstd-ocf-experimental.md).
      Conformance axis stays `non-conformant` until the spec registers Zstd **and**
      readers implement it.
- [ ] **P2 archival image re-pack** — for `.eparc`, losslessly re-pack media for
      density without generation loss: JPEG → **JXL-lossless-JPEG** (~20% smaller,
      bit-exact JPEG reconstructable), PNG → pixel-exact oxipng/JXL. Faithful
      default; opt-in `--lossy-images` compact mode. (Needs the v3.4 codecs, now
      shipped.)

[epub-specs#3025]: https://github.com/w3c/epub-specs/discussions/3025

---

## 📋 Backlog — small, unprioritized

The shipping product is in a complete place. What remains is a pool of
**nice-to-haves with no urgency and no priority** — none is scheduled. They get
pulled in **opportunistically**: when a larger piece of work needs one, we do
whichever of these we want. (The desktop GUI above stays demand-gated under the
same logic — the web app already serves the no-install case.)

- **`.eparc` P2 image re-pack** — lossless JPEG → JXL-lossless, PNG → pixel-exact
  (codecs now shipped).
- **`.eparc` deepening** — library-level dedup + corpus shared-dictionary;
  bit-exact `--exact` (byte-identical original).
- **EPUB 3.4 refinement** — more calibration books / per-format speed tuning;
  `--image-format best` in the web UI; re-check the W3C change log toward the 2027
  spec release.
- **Kobo** — optional parity comparison vs Calibre's KePub output.
- **Web build-info footer** — version + commit link (built, on a branch; ship it
  bundled with the next web change).

---

## Guiding principles

1. **Pure Rust, no C.** Every dependency stays C-free.
2. **Backward compatibility first.** Default output must open on the widest
   range of readers; newer-but-narrower formats are opt-in.
3. **Never mutate the input.** Originals are untouched unless the full pipeline
   succeeds.
4. **One core, many front-ends.** CLI and GUI share the same library.
