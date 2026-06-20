# EPUB 3.4 image codec — WebP vs AVIF vs JPEG XL (calibrated)

**Status:** research / experimental (the `epub34` feature). This is the
measurement basis for *which* image codec to emit under `--target 3.4`. It is the
3.4 analogue of [`eparc-codec-choice.md`](eparc-codec-choice.md): a data-first
defense, not a vibe.

## TL;DR

At **equal perceptual quality**, the best image codec **depends on the content**:

| Content (one book) | source | WebP | AVIF | JPEG XL | winner |
| :--- | :--- | ---: | ---: | ---: | :--- |
| Diagrams / screenshots / line-art | PNG | **baseline** | **+93.0%** | +33.4% | **WebP** |
| Photographs | JPEG | baseline | **−13.8%** | −5.2% | **AVIF** |

So a blanket *"EPUB 3.4 ⇒ AVIF"* default is wrong: it helps photo-heavy books and
**badly hurts** diagram-heavy ones (AVIF was nearly **2×** WebP on the line-art
book). Codec choice must be content-aware.

**Free, strong heuristic:** the *source* format already encodes the content type —
publishers use **PNG for line-art/screenshots** and **JPEG for photographs**. So a
zero-cost rule matches the data:

> `--target 3.4`: **JPEG source → AVIF/JXL**, **PNG source → WebP**.

(The most robust option is per-image: encode candidates and keep the smallest at
matched quality — but the source-format heuristic is far cheaper and agrees with
every measurement here.)

## Why content matters

AVIF (AV1 intra) and JPEG XL (VarDCT) are tuned for **photographic** content. On
sharp-edged **text / line-art / flat-colour** diagrams they spend bits poorly,
while WebP's lossy mode (VP8 intra) handles that content better. JPEG and PNG
sources are themselves a revealed-preference signal of which regime a book is in.

## Methodology

The comparison is only meaningful at **equal perceptual quality** — comparing
"WebP q80 vs AVIF q80 vs JXL distance(80)" is meaningless because those knobs mean
different things. We therefore:

1. Decode each book's source images to RGB (the source is the reference — what the
   book actually contains).
2. Encode each with all three codecs over a quality grid (all pure-Rust **imazen**
   codecs: `zenwebp`, `zenavif`, `zenjxl`).
3. **Decode back** and measure **butteraugli** (pure-Rust `butteraugli` crate, the
   metric JPEG XL's "distance" targets; <1.0 ≈ visually identical) vs the source,
   plus the encoded byte size.
4. Anchor on WebP q80's mean butteraugli, then **interpolate** each other codec's
   grid to that score → size at equal quality.

Tool: [`src/bin/img_calib.rs`](../../src/bin/img_calib.rs), built with the
`img-calib` feature.

```bash
# extract a book's images, then:
cargo run --release --features img-calib --bin img-calib -- <IMAGE_DIR> [SAMPLE] [AVIF_SPEED]
```

### Raw curves

Line-art book (20 PNG images, anchor butteraugli **0.5141**, AVIF speed 6):

```
webp  q80   0.5141   451.6 KB   (anchor)
avif  q85   0.6035   711.9 KB        jxl  d2.0  0.6004   553.2 KB
avif  q92   0.4933   908.6 KB        jxl  d1.5  0.4600   632.9 KB
→ avif @0.5141 = 871.6 KB (+93.0%)   → jxl @0.5141 = 602.2 KB (+33.4%)
```

Photo book (16 JPEG images, anchor butteraugli **1.0593**, AVIF speed 6):

```
webp  q80   1.0593  1030.7 KB   (anchor)
avif  q70   0.9622   950.5 KB        jxl  d3.0  1.2008   881.0 KB
avif  q85   0.5886  1405.3 KB        jxl  d2.0  0.8775  1099.6 KB
→ avif @1.0593 = 888.4 KB (−13.8%)   → jxl @1.0593 = 976.7 KB (−5.2%)
```

## Tooling caveats (important)

- **`zenavif`'s pure-Rust decoder (rav1d-safe 0.5.7) is unreliable on Apple
  Silicon**: it *panics* on some AVIFs (a loop-restoration SIMD `DisjointMut` /
  bounds bug) and *silently corrupts* others (the first AVIF runs showed
  non-monotonic butteraugli — the tell). The bench therefore decodes AVIF via
  macOS **`sips`** *for measurement only*. The `unsafe-asm` C decoder did not build
  (rav1d 1.1.0 ARM asm header missing).
  - **This does not affect the shipped product:** epublift only ever *encodes*
    AVIF (pure Rust), and reading systems do the decoding. We decode nothing.
- The butteraugli "quality" WebP q80 reaches differs by content (line-art 0.51 vs
  photo 1.06); each book is calibrated to its own WebP-q80 anchor.

## Caveats / scope

- One book per content type; 16–20 image samples; AVIF at speed 6. The effect
  sizes (+93% / −14%) are large enough to be real for these content types, but the
  exact percentages are not the point — the **sign flip** is.
- These are the imazen encoders at their **current tuning**. A slower AVIF speed
  or future encoder tuning could narrow (not erase) the line-art gap.
- Quality mapping is naïve (`--quality` → each codec's knob via fixed formulas);
  calibrated per-format knobs are a follow-up.

## Decision / direction

1. **Source-format heuristic is wired into `--target 3.4`** (the default): per
   image, **JPEG → AVIF, PNG → WebP** (`FormatPolicy::Auto`). An explicit
   `--image-format avif|jxl` forces one format for every image. This already
   delivers the unambiguous win — a diagram/line-art book no longer gets AVIF
   (which was +93% size *and* ~15× slower); it stays WebP, fast and small.
2. **Quality mapping is the remaining half (not done).** `convert` uses a fixed
   `--quality` (e.g. 80) mapped naïvely per codec, so AVIF q80 and WebP q80 are
   *different* perceptual qualities — AVIF q80 over-delivers quality and can come
   out **larger** than WebP q80 on a photo book (measured 0.96 MB vs 0.82 MB),
   even though AVIF is −14% **at equal quality**. The photo size win is only
   realized once `--quality N` means the same butteraugli across codecs. Until
   then, 3.4 on a photo book is *higher quality* but not smaller than 3.3.
3. **Next:** calibrate the per-format quality knobs (map `--quality` → each
   codec's knob so equal N = equal perceptual quality), re-measure on more books
   per content type, and consider a per-image "keep smallest at matched quality"
   mode. JXL stays available via `--image-format jxl`.

## Related

- Spec tracking + plan: [`../epub-3.4.md`](../epub-3.4.md).
- Archive codec choice (the parallel `.eparc` doc): [`eparc-codec-choice.md`](eparc-codec-choice.md).
- The size-safe guarantee (never grow a book) applies to all formats: [`../../README.md`](../../README.md).
