# Experimental: Zstandard packaging (research only)

> ⚠️ **Not in release builds, and not a conformant EPUB.** This is a
> measurement-only research track, compiled in only when you build from source
> with a feature flag. A file packed this way **will not open in any current
> reading system.**

The EPUB container (OCF) restricts ZIP compression to *Stored* + *Deflate*. ZIP
also registers **Zstandard as method 93**, so we built an opt-in mode to *measure*
what Zstd would save over Deflate for EPUB packaging — to bring real data to a
future [W3C `epub-specs`](https://github.com/w3c/epub-specs/discussions)
discussion. It's **pure Rust** (no C in any shipped artifact) and fully lossless
(round-trip verified):

```bash
# Build with the feature, then:
cargo run --features zstd-experimental -- -i book.epub --zstd
# → book_zstd-experimental.epub   (NON-CONFORMANT; for measurement)

# Cross-chapter "shared dictionary" mode (trains a dict from the book's text):
cargo run --features zstd-experimental -- -i book.epub --zstd --zstd-mode shared-dict

cargo run --features zstd-experimental -- -i book_zstd-experimental.epub --zstd-decode
# → reconstructs a normal, conformant EPUB (proves no data loss)
```

Two modes: **per-entry** (each entry compressed independently — the conservative
floor) and **shared-dict** (one dictionary trained from the book's own text
entries, stored as `META-INF/zstd-dict.bin`, then shared across chapters — the
bigger win on text-heavy, multi-chapter books, and explicitly non-standard). A
dev-only `zstd-bench` compares the pure-Rust encoder against reference C `libzstd`
for ratio and speed across a corpus, reporting both modes. Full rationale and the
two-axis (*maturity* vs *conformance*) framing live in
[`design/zstd-ocf-experimental.md`](design/zstd-ocf-experimental.md).

> Note: the shipped, conformant archival mode (`archive` / `restore` →
> [`.eparc`](archiving.md)) also uses pure-Rust Zstandard, but as a *separate,
> openable* archive format — not inside the EPUB container. See the
> [Archiving guide](archiving.md).
