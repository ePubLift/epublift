# Reader image-format compatibility

Which e-readers actually **render** which image formats inside an EPUB. This
drives ePubLift's defaults — e.g. why [`--keep-images`](usage.md) is on by
default for Kobo and `--kepub` output (Kobo shows WebP as blank pages).

**Scope: we only list what we have verified ourselves on real hardware.** This
is deliberately a short, honest table rather than a spec sheet copied from
marketing pages — see [Why this list is small](#why-this-list-is-small). It will
grow as we (and contributors) confirm more devices.

## Verified by us

What we have confirmed first-hand on real hardware.

| Device | Model(s) tested | JPG / PNG | WebP | AVIF / JXL | Our recommendation |
|--------|-----------------|:---------:|:----:|:----------:|--------------------|
| **Kobo** (e-ink) | Forma, Sage | ✅ | ❌ | — | `--keep-images` (and `--kepub` for Kobo's page-turn/stats features) |
| **Apple Books** | macOS, iOS | ✅ | ✅ | — | Default — `epublift -i book.epub` (WebP renders fine) |

**Legend** — ✅ renders correctly · ❌ fails (image shown blank) · — not yet
tested by us (no claim either way).

Notes:
- **Kobo / WebP** — confirmed on Forma and Sage: WebP images appear as blank
  pages, even though the files inside the EPUB are valid (the same book renders
  fine in Apple Books). This is a reader-engine limitation, not a packaging bug.
  → Use `--keep-images` (default on for `--kepub`) to keep the original JPEG/PNG.
- **Apple Books** is a reading *app*, not a device; we list it because it was our
  control when verifying the Kobo WebP issue.
- **AVIF / JXL** are grouped because they're the next-generation formats EPUB 3.4
  introduces; we have not yet tested either on any device, so both are left as
  `—`. Expect support to lag for years on cheap e-ink readers (see below).

## Reported (vendor docs / community — not tested by us)

These rows are **not** first-hand results — they summarise each vendor's own
documented image-format list (and well-known engine behaviour). "Not listed"
means the format is absent from the vendor's published support list; that's a
strong signal, but it is **not** the same as us testing it and seeing it fail.
Treat this section as a planning guide, not a guarantee.

| Reader / engine | Basis | JPG / PNG | WebP | AVIF / JXL | Our recommendation |
|-----------------|-------|:---------:|:----:|:----------:|--------------------|
| **Amazon Kindle** | vendor docs | ✅ | ❌ | ❌ | `--keep-images` — Kindle converts EPUB on import and supports JPG/PNG/GIF/BMP only |
| **PocketBook** (e-ink) | vendor docs | ✅ | ❌ | ❌ | `--keep-images` |
| **Onyx Boox** (stock NeoReader) | vendor docs | ✅ | ❌ | ❌ | `--keep-images` for the stock reader; WebP/modern formats may work in a third-party Android reading app (see camps below) |
| **Adobe Digital Editions** & RMSDK-based readers (Nook, Tolino, many others) | engine behaviour | ✅ | ❌ | ❌ | `--keep-images` |

**Legend (reported)** — ✅ documented as supported · ❌ not in the vendor's
documented list (likely unsupported, but untested by us) · — no information.

The pattern is consistent: outside Apple Books (and Android reading apps),
**WebP is generally not supported by dedicated e-ink readers**, and AVIF/JXL
nowhere. When in doubt for an e-ink target, `--keep-images` is the safe choice.

## The two e-ink camps (this part won't go stale)

Individual models and firmware change constantly, but the market splits into two
durable camps, and that split predicts format support better than any spec row:

- **Reading appliances** — Kindle, Kobo, basic e-ink readers. Cheap, low-power,
  older SoCs running a frozen custom Linux with an aging EPUB render engine.
  Software-only image decoding, conservative format support. Weeks of battery.
  **This is where the WebP/AVIF/JXL gap lives**, and where it will persist
  longest — modern image codecs cost die area, licensing, and CPU/battery that a
  grayscale reading appliance has no incentive to spend.
- **Android e-ink slates** — Onyx Boox, Viwoods, some Supernote models. Modern
  mid-range mobile SoCs (Qualcomm Snapdragon / Rockchip class, 64-bit, GPU, ISP)
  running full Android. The hardware/OS *can* decode modern formats, and you can
  install a third-party reading app that does — at the cost of much shorter
  battery life (the newer SoC is paid for in battery). Caveat: the **stock**
  reader still matters — Onyx's own NeoReader documents only JPG/PNG/BMP/TIFF, so
  "Android" doesn't automatically mean the bundled EPUB reader renders WebP. The
  capability is there; whether the app you use exercises it is the open question.

  Note: not every "premium-feeling" device is in this camp. **reMarkable**, for
  instance, uses a deliberately modest SoC (writing-latency focused, custom
  Linux), so for image-codec purposes it behaves more like the appliance camp.

Practical takeaway for now: if you're targeting **any** mainstream e-ink reader,
assume the appliance camp and keep widely-supported formats — JPEG/PNG, or WebP
only when you know the target renders it. The Android-slate camp is where the
"smaller modern formats" future is already real.

## Why this list is small

A full hardware comparison (CPU/RAM/price/screen) would go stale with every new
model and firmware update, and it isn't our domain — we're a conversion tool, not
a review site. So this page only records **format rendering we've confirmed
first-hand**, because that's the part that actually changes what flags you should
pass.

## Contributing a verified result

If you've tested an image format on a real device, we'd like to add it. Please
include:

- exact device + model and firmware version,
- the format(s) tested and whether each **rendered** or showed **blank**,
- how the test EPUB was produced (e.g. `epublift -i book.epub` for WebP, or
  `--keep-images` for the JPEG/PNG control),
- ideally a cross-check in a known-good reader (Apple Books) to rule out a bad
  file.

Open an issue or PR with that and we'll add a row marked as verified.
