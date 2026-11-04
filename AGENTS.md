# ePubLift: detail for code agents and maintainers

`README.md` is the short introduction. This file holds the detail and is kept
true at every release. Options are in `docs/usage.md` and the other guides
linked below; this file does not repeat them. Past changes: `CHANGELOG.md`.

## Scope

ePubLift is a pure-Rust EPUB toolbox for readers and publishers. One Cargo
workspace holds two components, each with its own version and tag:

- the CLI `epublift` and its library (root crate), tags `cli-vX.Y.Z`;
- the web service `epublift-web` (Axum, `epublift-web/`), tags `web-vX.Y.Z`.

A hosted instance runs at <https://epublift.itpax.net>. The image is
`ghcr.io/epublift/epublift-web` (amd64, arm64).

It declines:

- **DRM removal.** Point users to DeDRM or calibre.
- **Amazon metadata,** which would mean scraping. The providers are Open Library
  and Google Books.
- **Any C toolchain.** Dependencies are pure Rust, including TLS (rustls,
  RustCrypto, webpki-roots over the small client in `src/http.rs`) and images
  (zenwebp, zenavif, zenjxl). Benchmark-only features may link C; they never
  ship.
- **Deconvolution, vectorising and ML upscaling of images.** They were tested
  and rejected.

Rules the code follows:

- **Never make a book worse.** The input is never modified. An image is replaced
  only when the re-encode is smaller. WebP quality is capped at the source JPEG's
  estimated quality. Grayscale stays gray.
- **Kobo does not render WebP** (its Qt 5.2.1 has no plugin for it), so images
  are kept by default and `--kepub` keeps originals. `--kepub-webp` is for
  devices with the plugin in `kobo-webp-plugin/`.
- **The family's tools do the family's work.** Validation, repair, KEPUB markup
  and PDF reading live in the crates below and are not reimplemented here.

## Family crates

| Crate | Used for |
| --- | --- |
| [`epubveri`](https://github.com/veripublica/epubveri) | `check`; `xmlguard::check` before every roxmltree parse (`util::parse_xml`) |
| [`epubsana`](https://github.com/veripublica/epubsana) | `repair`; the last step of the upgrade (`src/upgrade_fixes.rs`) |
| [`kepubverto`](https://github.com/veripublica/kepubverto) | `--kepub` (`src/kepub.rs`); refused with zstd packaging |
| [`paginaverto`](https://github.com/veripublica/paginaverto) | PDF import, pinned exactly; `src/pdf/` keeps the title, language, reflow and model download |
| [`styloria`](https://github.com/veripublica/styloria) | The cascade behind presentational attributes (`src/presentational.rs`) |

The web service runs epubveri and epubsana in the browser from WASM vendored in
`epublift-web/static/vendor/`. The crate and its WASM move in one change, or the
CLI and the web grade the same book differently. `VENDOR.md` there has the steps
and digests.

## CLI

| Command | What it does | Guide |
| --- | --- | --- |
| `epublift -i book.epub` | Upgrade to `book_v3.3.epub` (`_v3.4` with `--target 3.4`; `.kepub.epub` with `--kepub`) | `docs/usage.md`, `docs/epub-3.4.md` |
| `check` | Validate, epubcheck-compatible message ids | `docs/validate.md` |
| `repair` | Apply fixes, asking about those that need a decision; up to 3 passes while a fix reveals more | `docs/repair.md` |
| `meta show`, `set`, `enrich` | Read, edit, or fill metadata by ISBN | `docs/metadata.md` |
| `archive`, `restore` | Lossless `.eparc`, and back | `docs/archiving.md` |
| `import` | PDF, or a Markdown file, folder or zip, into a reflowable EPUB (experimental) | `docs/pdf-import.md`, `docs/markdown-import.md` |

The command line conforms to [veripublica conventions](https://github.com/veripublica/conventions/blob/main/CLI.md)
v0.7: input only through `-i`, one per run except for `check`, and no existing
file replaced without `-f`.

Exit codes:

| Code | Upgrade, `import`, re-targeting `restore` | `meta`, `archive`, plain `restore` | `check` | `repair` |
| --- | --- | --- | --- | --- |
| 0 | written, and the book is valid | done | every input valid | goal met |
| 1 | written, but errors remain | | at least one invalid | defects remain |
| 2 | usage error, refusal or failure | the same | an input unreadable | unreadable, or output = input |

`check --format json` and `repair --format json` emit the veripublica envelope.
`meta show --format metadata` prints one JSON object. Changing exit codes, JSON
output or flags is a CLI major.

`--help` on every command lists its options and is the reference for names.

## The upgrade

`convert()` in `src/lib.rs`, in order:

1. Extract into a system temporary directory, under the limits below.
2. Find and parse the OPF.
3. With `--webp` or `--target 3.4`, re-encode images and rewrite references in
   XHTML, CSS, SVG and the manifest.
4. Raise the package to 3.0, translate EPUB 2 metadata to `<meta refines>`, add
   `dcterms:modified`.
5. Build `nav.xhtml` from `toc.ncx`, which stays for old readers; turn `<guide>`
   into landmarks.
6. Set HTML5 DOCTYPEs.
7. Write presentational attributes and obsolete elements as CSS, where the book's
   own CSS does not set the property (`docs/usage.md`).
8. Package with `mimetype` first and stored.
9. Run epubsana's EPUB 3 fixes.
10. Write the size report.

## Library

The root crate is usable as a library: build `Options`, call `convert()`, read
`Report`. `read_metadata`, `write_metadata`, `write_metadata_and_cover` and
`read_cover` serve the metadata editor. Modules `meta`, `cover`, `enrich`,
`eparc`, `markdown`, `pdf` and `smart_import` are public, some behind features.
It is not published to crates.io.

## Web service

Modes, in workflow order: Validate, Repair, Metadata, Optimize, Archive, Restore,
Import, Smart Import. Validate and Repair run in the browser and the book is not
uploaded. The others are synchronous server endpoints.

- `GET /version` gives `{version, engine, epubveri, commit}`, `GET /config` gives
  Smart Import's state and providers, never a key, `GET /healthz` gives `ok`.
- Static files are `include_str!`/`include_bytes!`: rebuild after any change in
  `static/`.
- The UI ships in 13 languages (`static/i18n.js`: en es tr de fr pt it nl pl ru ja
  ko zh). A new key goes in every block; there is no English fallback.
- Port 8080 is fixed. Environment variables and the hardening profile are in
  `docs/web-service.md`.
- OCR is not on the web: it would need asynchronous jobs.
- Smart Import uses Mistral with the operator's own key. The hosted instance has
  none, on purpose (`docs/smart-import.md`).

## Limits

| Limit | Value | Where |
| --- | --- | --- |
| Archive entries | 50,000 | `src/lib.rs` |
| Total uncompressed | 1 GiB | `src/lib.rs` |
| Image dimension | 16,384 px | `src/images.rs` |
| Image allocation | 512 MiB | `src/images.rs` |
| Markdown zip | 5,000 entries, 200 MiB | `src/markdown.rs` |
| Cover | 10 MB | `src/cover.rs` |
| Web upload | 50 MiB | `epublift-web/src/main.rs` |

Request time, result lifetime and the rate limit are in `SECURITY.md`. The CSP is
`script-src 'self' 'wasm-unsafe-eval'`. The rate limit trusts `X-Forwarded-For`
only from a private or loopback peer. Measurements behind the design are in the
guides and in each `CHANGELOG.md` entry, with the versions they ran on.

## Build features

Default: `archival`. Opt-in:

| Feature | Adds |
| --- | --- |
| `metadata` | `meta enrich` |
| `epub34` | AVIF and JPEG XL under `--target 3.4` |
| `validate` | `check` |
| `pdf` | PDF import from a text layer |
| `pdf-ocr` | `pdf` plus OCR; models download on first use and are checked by SHA-256 |
| `markdown` | Markdown import |
| `smart-import` | Mistral |
| `zstd-experimental` | Research packaging, not conformant (`docs/zstandard-research.md`) |

Release binaries ship `epub34,metadata,pdf-ocr,markdown,validate`. The web crate
uses `epub34,metadata,pdf,markdown,smart-import`. Others (`*-bench`, `img-calib`,
`gen-sample`) are for development.

## Layout

```
src/lib.rs             convert(), Options, Report, limits
src/main.rs            the CLI
src/opf.rs nav.rs      package document, navigation
src/images.rs          image conversion and its size and quality rules
src/presentational.rs  attributes to CSS, guarded by the cascade
src/upgrade_fixes.rs   the upgrade's epubsana pass
src/kepub.rs           kepubverto wrapper
src/meta.rs cover.rs enrich.rs  metadata
src/unique_id.rs       the package's unique identifier
src/eparc.rs           .eparc format
src/epub_writer.rs     EPUB writer the importers share
src/pdf/ markdown.rs smart_import/  importers
src/http.rs            pure-Rust HTTPS client
src/util.rs            parse_xml, shared helpers
src/bin/               development tools
tests/                 integration tests, including the family's CLI conventions
epublift-web/          web service; static/vendor/ is the vendored WASM
kobo-webp-plugin/      WebP plugin for Kobo devices
docs/                  user guides; docs/design/ holds design notes
```

## Build and test

MSRV is the latest stable minus two, equal in both crates (`rust-version`).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --all-targets --locked --features zstd-experimental,pdf-ocr,markdown,smart-import,validate
cargo build --release --locked --features epub34,metadata,pdf-ocr,markdown,validate --bin epublift
docker build -t epublift-web:pre .
```

XML goes through `util::parse_xml`. A change to a flag or behaviour updates its
`docs/*.md` in the same commit.

## Releases

Released from `main` by annotated tag. `cli-vX.Y.Z` fires `release.yml`: a GitHub
Release with five targets, `.deb`, `.rpm` and `.sha256` files. `web-vX.Y.Z` fires
`docker.yml`: the image, and a GitHub Release not marked latest. Both take their
notes from the `CHANGELOG.md` section whose heading contains the exact tag.

## Licence

AGPL-3.0-or-later. The imazen `zen*` codecs are AGPL-or-commercial, so a permissive
relicense is not possible while they are linked.
