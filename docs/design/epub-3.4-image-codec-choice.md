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

## Quality calibration (`--quality` → per-codec knob)

`convert` takes one `--quality N` (default 80). The codecs' native knobs are *not*
the same perceptual scale, so feeding N raw makes them disagree: AVIF q80
over-delivers quality and comes out **larger** than WebP q80 even though AVIF is
smaller *at equal quality*. We therefore treat **WebP quality as the reference
scale** and map N → each codec's knob to hit the same butteraugli.

Calibration table (`img-calib` over **48 JPEG-source images from 11 books** — the
content AVIF/JXL are actually applied to: real photos *and* JPEG-saved charts):

| webp_q | butteraugli | avif_q (match) | avif Δ | jxl_d (match) | jxl Δ |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 50 | 1.39 | 54 | −3% | 3.36 | +3% |
| 60 | 1.25 | 58 | −4% | 2.92 | +1% |
| 70 | 1.13 | 61 | −5% | 2.61 | −1% |
| 80 | 0.90 | 68 | −11% | 2.08 | −9% |
| 90 | 0.60 | 79 | −19% | 1.24 | −19% |

Least-squares linear fits (in `src/images.rs`, `epub34`):

```
avif_q   ≈ 0.60 · webp_q + 22      (clamped 1..100)
jxl_dist ≈ −0.051 · webp_q + 6.0   (clamped 0.4..15)
```

AVIF reaches WebP's quality at a *lower* knob, and its size advantage grows with
quality. **End-to-end verification** (`--target 3.4` vs `--target 3.3` at default
quality, AVIF at matched quality):

| Book | 3.3 WebP | 3.4 AVIF | Δ |
| :--- | ---: | ---: | ---: |
| Üç Kıtada Osmanlılar (historical photos) | 1952 KB | 1737 KB | **−11%** |
| Sapiens | 1953 KB | 1874 KB | −4% |
| Küçük Prens | 1251 KB | 1205 KB | −4% |
| Senin Kovan | 842 KB | 847 KB | +0.6% |

So AVIF's advantage on JPEG-source content is real but **content-dependent and
modest** (≈0–11%; historical photos benefit most). Before calibration the same
photo book was *larger* under 3.4 (q80-raw over-delivers quality).

> Calibrated on 48 images / 11 books. The earlier single-book fit (0.64·q+17)
> was slightly overfit to one book; the multi-book fit is less aggressive and
> more representative. Could refine further with a non-linear fit and per-format
> speed tuning.

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
2. **Quality mapping is calibrated (done).** `--quality N` is the WebP reference
   scale; AVIF/JXL knobs are derived from N (see "Quality calibration" above, fit
   over 48 images / 11 books) so equal N ≈ equal butteraugli. This realizes the
   photo win — at matched quality, `--target 3.4` is **≈0–11% smaller** than 3.3
   on photo books (historical photos most). Before calibration, raw q80
   over-delivered quality and 3.4 came out *larger* than 3.3.
3. **Per-image "keep smallest" mode (done): `--image-format best`.** Encodes every
   candidate per image and keeps the smallest — valid because `--quality` is
   calibrated, so smallest bytes = smallest *at matched quality*. It corrects the
   heuristic's per-image misroutes: on the photo book it chose **13 WebP + 3 AVIF**
   and came out **839 KB**, beating both the WebP-only (842 KB) and the
   AVIF-everywhere Auto (847 KB). The cost is multiple encodes per image (the slow
   AVIF one included), so it's an opt-in "thorough" mode — `Auto` stays the fast
   default. (Notably, the heuristic over-routes to AVIF here: WebP actually wins on
   most of this book's JPEGs at matched quality.)
4. **Next:** refine the calibration further (more books, non-linear fit, per-format
   speed), and extend `--target 3.4` to `restore` / web. JXL stays available via
   `--image-format jxl`, though it is size-dominated in both regimes (never the
   smallest), so `best` rarely picks it.

## Related

- Spec tracking + plan: [`../epub-3.4.md`](../epub-3.4.md).
- Archive codec choice (the parallel `.eparc` doc): [`eparc-codec-choice.md`](eparc-codec-choice.md).
- The size-safe guarantee (never grow a book) applies to all formats: [`../../README.md`](../../README.md).
