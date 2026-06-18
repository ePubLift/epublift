# Decision — `.eparc` archive codec: pure-Rust Zstandard (L19), solid stream

**Status:** Decided (codec + level). Format implementation pending.
**Decision:** the `.eparc` archive **solid-compresses the text with pure-Rust
Zstandard** (`structured-zstd`, level 19) and **stores media (images/fonts)
verbatim**. Brotli and xz/LZMA are kept only as *dev-bench ceilings*; an opt-in
`--max=brotli` is deferred until there is real demand.
**Owner:** Baris Kayadelen

This document records *why*, with reproducible measurements, so that a future
"why not Brotli / xz / zstd-ultra / C — isn't C better?" question has a
data-backed answer instead of an opinion.

Context — what `.eparc` is *for*: a tool people **download and run on their own
device** (smallest Raspberry Pi → home PC → NAS → small personal server) to shrink
their personal EPUB library and save disk. **No enterprise / bulk-server tier.**
We control both ends, the archive is opened *whole* on restore (no random access),
and the first-run experience must not make a new user wait hours. Slow one-time
compression is tolerable only up to a point; restore must be fast and fit Pi RAM.

Because there is no random-access requirement, the archive is a single **solid**
stream: all *text* entries concatenated and compressed together, so one window
spans every chapter and cross-chapter redundancy is captured with **no stored
dictionary**. Already-compressed media is **not** run through the codec (that is
wasted CPU and is where pure-Rust zstd is weakest — see §2.2); it is stored
verbatim. This is why the solid archive is far smaller on text than the per-entry
OCF-experimental ZIP (`zstd-ocf-experimental.md`, which stays per-entry because it
must emulate a random-access EPUB container).

---

## 1. The measurement

Corpus: **170 real EPUBs** (mixed languages, mostly Turkish; fiction +
technical), plus a focused **40-book subset in the 1–4 MB range** (the typical
personal-library book) used for the per-book / time projections. Each book's
entries are concatenated into one length-prefixed solid buffer, compressed, and
round-trip verified. Figures are **% smaller than today's conformant Deflate**
packaging (negative = smaller), reported on the **text payload** (XHTML/CSS/OPF/
NCX) and on the **whole archive** (everything, codec-over-images included — the
naive case, *not* what we ship).

Peak RSS is measured with `/usr/bin/time -l` around a single-codec **encode** (the
memory-hungry phase and the real Pi constraint) on a typical ~8 MB book and a
73 MB image-heavy worst case.

| codec (solid) | pure-Rust | text % | whole % | enc MB/s | restore MB/s | RSS 8 MB | RSS 73 MB |
|---|:---:|---:|---:|---:|---:|---:|---:|
| **zstd-rust L19** ← shipped | ✅ | −31.7 | −9.4 | 3.5 | 1227 | 131 MB | 411 MB |
| brotli-11 | ✅ | −33.1 | −14.6 | 1.0 | 177 | 217 MB | 512 MB |
| zstd-C L19 | ❌ C | −31.7 | −13.5 | 5.0 | 1077 | 121 MB | — |
| zstd-C **L22 (--ultra)** | ❌ C | −31.7 | −14.0 | 4.0 | 1101 | **745 MB** | **1015 MB** |
| xz −9 | ❌ C | −32.5 | −14.9 | 4.0 | 46 | 170 MB | 886 MB |
| xz −9e | ❌ C | −32.7 | −15.0 | 3.6 | 45 | 171 MB | 886 MB |

Solid mode is the headline architectural result: per-entry zstd (the OCF floor) is
only −6.3% on text, **solid is ~5× that** (−31.7%).

---

## 2. Why pure-Rust Zstandard

### 2.1 First-run UX on the target hardware wins, and the size cost is noise
Measured on the **40-book 1–4 MB subset**, scaled to a 1000-book library:

| | zstd-rust L19 | brotli-11 |
|---|---|---|
| 1000-book size (text-heavy, from ~2.5 GB) | ~1.64 GB | ~1.61 GB |
| encode speed | 3.2 MB/s | 1.1 MB/s (**~3× slower**) |
| time, 1000 books (desktop) | **~36 min** | ~1.8 h |
| time, 1000 books (Pi, ~6× est.) | **~3.5 h** | **~11 h** |
| restore (decode) | 951 MB/s | 213 MB/s |

Brotli buys **~1.2 pp** on text (≈ 30 MB across 1000 books, ~2 %) for **~3× the
time** — on a Pi that is **~11 h vs ~3.5 h**. For someone who just downloaded the
tool, hours of waiting is the difference between "this is great" and uninstalling.
Zstd is also the faster *decoder* (951 vs 213 MB/s), so restore is snappier — and
restore is the frequent operation.

### 2.2 The media-verbatim architecture nullifies Brotli's only real edge
Brotli's whole-archive lead (−14.6 % vs −9.4 %) comes almost entirely from the
**image bytes** (text-only gap is just 0.7–1.2 pp). Pure-Rust zstd handles
already-compressed media poorly in a solid stream; pure-Rust Brotli handles it
better. **But we store media verbatim and only compress text** — so the codec
difference collapses to the **~1.2 pp text gap, even for image-heavy books.** Zstd
loses essentially nothing in the shipped design while being far faster. (Storing
images verbatim is correct anyway: re-compressing JPEG/PNG through a text codec is
wasted CPU. Shrinking images is the *converter's* job — see §3.)

### 2.3 Going C buys ≤ 0.4 pp — and the "compress harder" knob is a RAM trap
On text, pure-Rust Brotli already **beats** the best C (xz −9e). On the whole
archive the best C beats it by **0.4 pp** (~861 KB / 170 books, ~5 KB/book). The
knob people reach for, zstd `--ultra -22`, gives **zero** on text / +0.5 pp on the
whole archive while peak RSS jumps to **745 MB (8 MB book) / 1015 MB (73 MB book)**
— it **OOMs a 512 MB Pi Zero 2 W and strains a 1 GB Pi** for no benefit. xz's
marginal edge also costs ~4× slower restore (~45 MB/s) and a C/liblzma toolchain
that breaks pure-Rust portability and complicates ARM cross-compilation. None of it
is worth it for this product.

### 2.4 L19 is the practical pure-Rust ceiling — L22 is pure loss
`structured-zstd`'s encoder effectively caps at ~L19. Measured L19 → L22 on the
40-book subset: **2 KB smaller total** (≈ 0.05 KB/book, noise) while **~10 % slower**
(text 3.32 → 3.06 MB/s; whole 5.39 → 4.81 MB/s). So **L19** — higher levels only
burn time.

### 2.5 One compression library
Zstd (`structured-zstd`) is already the project's compression backend for the
OCF/W3C research track. Using it for `.eparc` too keeps the project on **a single
pure-Rust compression dependency** — Brotli never enters the shipped binary (it
stays in the dev bench as a measured ceiling, alongside xz).

**Net:** pure-Rust zstd L19 gives up ~1.2 pp of text ratio (noise at library scale)
for ~3× faster archiving, faster restore, bounded Pi memory, no C, and one
compression lib. The C/Brotli/ultra numbers are kept as honest ceilings, not as
the shipped path. `--max=brotli` can be added later (the bench's codec abstraction
already supports it) **if** real "smallest-possible, I'll wait" demand appears.

---

## 3. Composition with image modernization (the real "tiny" path)

Archiving shrinks **text**; it cannot shrink already-compressed images — that is
the **converter's** job. The two compose into a power pipeline:

```
original .epub ──(epublift convert: modernize + re-encode images)──▶ smaller .epub
              ──(epublift archive: solid-zstd the text, media verbatim)──▶ .eparc
```

Today the converter re-encodes images to WebP. With **EPUB v3.4 (AVIF / JXL)** the
image bytes themselves get materially smaller (AVIF/JXL beat WebP/JPEG/PNG). So a
user who **first upgrades to v3.4 (small images) and then archives to `.eparc`
(small text)** ends up *dramatically* smaller than either step alone — the image
win and the text win compound, because they act on disjoint parts of the book.
Archiving and the v3.4 image roadmap are complementary, not competing.

---

## 4. Pi-fit notes

- **Restore is cheap regardless of compression effort** (codec asymmetry): zstd
  decodes ~950–1200 MB/s here → a typical archive restores in milliseconds, even at
  a several-× slowdown on a Pi.
- **Encode memory is bounded by single-book scope.** zstd-rust L19 peaked at
  131 MB (8 MB book) / 411 MB (73 MB worst case) — fits the whole target range. The
  RAM-hungry giant-window/dictionary case is deliberately out of scope (no bulk /
  enterprise tier).
- For very long single books a future `--level` could trade ratio for speed; L19 is
  the default and the practical pure-Rust max.

---

## 5. Reproduce

Dev-only benchmark, never shipped (gated behind bench features; the C ceilings
behind `ceiling-bench`):

```sh
# pure-Rust candidates, ratio + enc/dec speed (text-only + whole archive):
cargo run --release --bin zstd-bench --features archival-bench -- --solid <dir-of-epubs>

# add the C ceilings (libzstd incl. --ultra 22, and xz/LZMA):
cargo run --release --bin zstd-bench --features ceiling-bench -- --solid <dir-of-epubs>

# level sweep (e.g. confirm L19 == L22 for pure-Rust zstd):
... --solid --levels 19,22 <dir-of-epubs>

# peak RSS for one codec on one book (wrap with the OS timer):
/usr/bin/time -l \
  cargo run --release --bin zstd-bench --features ceiling-bench -- \
  --mem-probe "zstd-rust L19" big.epub   # macOS: -l ; GNU: /usr/bin/time -v
```

Codec names for `--mem-probe`: `zstd-rust L19`, `zstd-rust L22`, `brotli-11`,
`zstd-C L19`, `zstd-C L22`, `xz -9`, `xz -9e`.
