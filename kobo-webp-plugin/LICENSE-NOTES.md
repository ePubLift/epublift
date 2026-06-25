# Licensing notes

`libqwebp.so` is built from third-party open-source components, each under its
own license. ePubLift itself is AGPL-3.0, but **this plugin is not an ePubLift
work** — it is upstream Qt's WebP image-format plugin, recompiled. The relevant
licenses are:

## Qt (the image-format plugin + the Qt 5.2.1 it links)

- **License:** GNU **LGPL** v2.1 (Qt 5.2.1's available option), © The Qt Company /
  Digia and contributors.
- The `webp` image-format plugin source is from
  [`qt/qtimageformats`](https://github.com/qt/qtimageformats) tag `v5.3.0`.
- It dynamically links Qt 5.2.1 (`libQt5Gui`, `libQt5Core`), built from
  [`kobolabs/qtbase`](https://github.com/kobolabs/qtbase) (Kobo's published Qt
  source) — itself LGPL.

**LGPL compliance / your rights:** because the plugin links Qt under the LGPL,
you are entitled to **relink it against a modified Qt** and to the corresponding
source. We satisfy this by:
- using only **publicly available, unmodified** upstream sources (links above),
- providing a **complete, reproducible build recipe** in [`BUILD.md`](BUILD.md)
  that regenerates the plugin from those sources,
- shipping the plugin as a separate, dynamically-linked `.so` you can rebuild and
  replace.

## libwebp (bundled inside the plugin)

- **License:** **BSD 3-Clause**, © Google Inc.
- The Qt 5.3.0 webp plugin **bundles** libwebp (`qtimageformats/src/3rdparty/libwebp`)
  and compiles it into `libqwebp.so`. The BSD license permits redistribution in
  binary form with attribution; the copyright/license text travels with the
  upstream source used in [`BUILD.md`](BUILD.md).

## Summary

| Component | License | Source |
|-----------|---------|--------|
| WebP image-format plugin | LGPL v2.1 | `qt/qtimageformats` @ v5.3.0 |
| Qt 5.2.1 (linked) | LGPL v2.1 | `kobolabs/qtbase` |
| libwebp (bundled) | BSD 3-Clause | bundled in qtimageformats v5.3.0 |

No proprietary code is included. Trademarks (Qt, Kobo, WebP) belong to their
respective owners; this project is independent and unaffiliated. See
[`README.md`](README.md#disclaimer) for the warranty disclaimer.
