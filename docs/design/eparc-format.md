# Design — the `.eparc` archive format & the archival mode

**Status:** Design (Phase 1). Codec already decided — see
`eparc-codec-choice.md`. Implementation pending.
**Owner:** Baris Kayadelen

`.eparc` ("ePub Archive") is the output of epublift's **archival mode**
(`epublift archive` / `epublift restore`). It shrinks a personal EPUB library to
save disk, and gives the book back on demand — re-targeted to whatever device the
user is reading on. It is **not** a conformant EPUB and will not open in a reading
system; that is by design (the openable, conformant artifact is what `restore`
produces). Reserve the name **EPUB/A** for the *openable, validated archival
profile* a future `restore --target` may emit — not for this compressed blob.

---

## 1. Goals & non-goals

**Goals**
1. Make a personal EPUB library **smaller on disk**, losslessly — the original is
   always recoverable.
2. **Run on the target hardware:** smallest Raspberry Pi → home PC → NAS → small
   personal server. Slow one-time archiving is tolerable; **restore must be fast**
   and fit Pi RAM.
3. **Re-target on restore:** the archive is the canonical master; the user chooses
   the output EPUB version / device format at restore time (we cannot know in
   advance what device the book will be read on).
4. **Boring & durable / independently re-implementable.** A `.eparc` must be
   openable in 2050 with standard tools, not just our binary.
5. **Pure Rust, no C** (Guiding Principle #1).

**Non-goals (this phase)**
- No enterprise / bulk-server tier; no cross-book deduplication or
  corpus-wide shared dictionary (that is the future *library* phase, on bigger
  hardware).
- No **bit-exact** (byte-identical whole-file) restore yet — Phase 1 is
  **content-exact** (every inner file byte-identical, container re-zipped). Whole
  `.epub` SHA-256 reproduction is a preservation-tier feature, deferred.
- No **v2.0 downgrade** target yet (epublift is an *upgrader*; v3→v2 is separate,
  large, low-demand work).
- No **lossy image transcoding** of the master (would destroy fidelity — see §6).

---

## 2. Container

A `.eparc` is a **ZIP archive, all entries `Stored` (no ZIP compression)** — built
with the `zip` crate's `ZipWriter` (already a dependency, used the same way as the
normal EPUB repackager), so **no new container code**. Compression lives *inside*
the entries (`data.zst`), not in the ZIP layer.

```
book.eparc                 (ZIP, stored)
├── manifest.json          ← inventory + provenance + integrity (§4)
├── data.zst               ← all COMPRESSIBLE entries concatenated, one solid frame (§3)
└── media/                 ← each already-compressed entry stored VERBATIM
    ├── cover.jpg
    ├── audio/clip.mp3
    └── …
```

**Why ZIP (not tar):** the ZIP writer already exists; an EPUB is itself a ZIP, so
we stay in one tool family; and it is transparently inspectable — a 2050 user runs
`unzip book.eparc`, gets `manifest.json` + `data.zst` + `media/`, then
`zstd -d data.zst`, and has everything with **no epublift binary required**. Media
is `Stored`, so it stays byte-verbatim; `data.zst` is already compressed, so
storing it adds no overhead.

---

## 3. The compressed payload (`data.zst`)

Every **compressible** entry — text (XHTML/CSS/OPF/NCX/…) **and fonts** (OTF/TTF)
and any unknown extension — is concatenated **in manifest order** into one buffer
and compressed as a **single solid zstd frame** (pure-Rust `structured-zstd`,
level 19). One window spans every chapter → cross-chapter redundancy captured with
**no stored dictionary**.

Only content that is *already* compressed is kept out of the stream and stored
verbatim (§6): images (JPEG/PNG/WebP/AVIF/JXL…), WOFF/WOFF2 web fonts, audio,
video, archives. Everything else is compressed — crucially **OTF/TTF fonts**,
which deflate well; an early version stored them verbatim and *inflated* small
books. Unknown extensions default to the stream, where zstd never meaningfully
grows even incompressible input (it stores raw blocks).

No inline framing: entry boundaries come from the per-entry `size` fields in the
manifest. On restore, `data.zst` is decompressed once into a single buffer and
split by those sizes, in order.

Rationale for codec/level/solid (measured, 170-book corpus): see
`eparc-codec-choice.md`. Summary: solid zstd L19 ≈ −32% on text vs Deflate; L22
gives nothing in pure-Rust zstd; Brotli/xz are dev-bench ceilings only.

---

## 4. `manifest.json`

Human-readable JSON (durable, inspectable). Small (KB-scale); verbosity is
irrelevant. **Forward-compatible by design** — the per-entry `media_format` /
`source_format` fields are reserved now so a future `format_version: 2` can add
lossless image re-packing / transcoding (§6) without breaking the format's
self-description.

```json
{
  "format": "eparc",
  "format_version": 1,
  "tool": "epublift 1.4.0",
  "created": "2026-06-18T12:00:00Z",
  "source": {
    "filename": "Book (Author).epub",
    "size": 2516582,
    "sha256": "…"
  },
  "epub_version": "3.0",
  "codec": { "name": "zstd", "impl": "structured-zstd", "level": 19, "mode": "solid" },
  "entries": [
    { "path": "mimetype",            "store": "verbatim", "size": 20,    "crc32": "…" },
    { "path": "META-INF/container.xml","store": "stream", "size": 240,   "crc32": "…" },
    { "path": "OEBPS/ch1.xhtml",     "store": "stream",   "size": 12345, "crc32": "…" },
    { "path": "OEBPS/fonts/body.otf","store": "stream",   "size": 40000, "crc32": "…" },
    { "path": "OEBPS/img/cover.jpg", "store": "verbatim", "size": 99999, "crc32": "…",
      "media_format": "original", "source_format": "jpeg" }
  ]
}
```

Field roles:
- **`entries` order** = original ZIP entry order → content-exact re-emit
  (`mimetype` first and `Stored`, per OCF).
- **`store`**: `stream` (→ concatenated into `data.zst`) or `verbatim` (→ stored
  file under `media/`). `mimetype` is `verbatim` (kept first); already-compressed
  media is `verbatim`; everything else (text, OTF/TTF fonts, unknown) is `stream`.
- **`size`**: uncompressed entry size — splits the decompressed `data.zst` buffer
  back into entries; sanity-checks media.
- **`crc32`**: per-entry integrity (cheap corruption detection; `crc32fast`, already
  a dependency). Crypto strength is unnecessary per-entry — the whole-file
  `source.sha256` covers tamper-evidence.
- **`source.sha256`**: whole original `.epub` fixity / provenance (universally
  verifiable with `sha256sum`; see `eparc-codec-choice.md` §hash for why SHA-256
  over BLAKE3 — speed is moot here, longevity/ubiquity wins). Also the natural
  dedup key for the future library phase.
- **`media_format` / `source_format`** *(reserved, Phase 1 always `original`)*:
  the forward-compat hook for §6.

---

## 5. Integrity

- **Whole archive provenance:** `source.sha256` records the original `.epub`'s
  SHA-256. Phase-1 `restore` is content-exact (re-zipped), so the restored file's
  whole-file hash will differ — `restore --verify` reports this honestly:
  *"content-exact restore; per-entry CRC32 verified; original was sha256 = X
  (this is not a bit-exact reproduction)."*
- **Per-entry:** CRC32 on every entry, checked on restore.

---

## 6. Media fidelity — the master must never lose quality

**Principle: the archive master is at least as good as the original.** Any lossy
step happens only once, at restore, for the target device — exactly as in the
original. Storing a *lossy* AVIF/JXL master would compound generation loss
(`JPEG → AVIF → JPEG` = two lossy passes, visibly worse than the original) and
break the faithful-archive promise. **Forbidden as a default.**

- **Phase 1 (now):** already-compressed media stored **verbatim** (text and fonts
  are compressed in the stream). Disk savings come from that compressed payload;
  image-heavy books shrink less (their images are already compressed and cannot be
  shrunk losslessly for free).
- **Phase 2 (with the v3.4 image codecs — see `imazen` roadmap):**
  - *Lossless re-pack* (faithful, default): JPEG → **JXL lossless-JPEG**
    (~20 % smaller, **bit-exact JPEG reconstructable**); PNG → oxipng / JXL-lossless
    (pixel-exact). Smaller **and** faithful — the principled version of
    "shrink the images."
  - *Compact mode* (explicit opt-in, e.g. `--lossy-images Q`): transcode to lossy
    AVIF/JXL for a much smaller archive, at a conscious fidelity cost. Not faithful;
    the user chooses disk over pixels.
- **Re-target on restore (Phase 2):** because the master is high-quality, restore
  can transcode **down** to the device's needs in a single lossy step — `--target
  3.3` → WebP, an older target → JPEG, and (once **EPUB 3.4 is published**) a
  `--target 3.4` that keeps AVIF/JXL (smallest). This is the engine behind the
  EPUB-library-app vision (device syncs → reports what it wants → we serve the
  matching format from one master).

Phase 1 stores verbatim; the manifest's `media_format`/`source_format` fields make
Phase 2 a `format_version` bump, not a redesign.

---

## 7. Restore — the archive is the master, the user picks the output

`restore` always derives from the single stored original; nothing extra is stored
for re-targeting (it is a restore-time `convert()` pass).

```
epublift restore book.eparc
    # default: content-exact — the original, as archived, no transform (fast, faithful)
    --target 3.3           # re-emit via the convert() pipeline at EPUB 3.3
                           # (3.4 added once that spec is published — see §6)
    --keep-images          # original JPEG/PNG instead of WebP (e.g. older Kobo)
    --kepub                # inject Kobo koboSpans (orthogonal; combinable)
    --modernize            # alias for "give me a clean, current EPUB/A" (→ default target)
    --verify               # recompute hashes/CRCs and report
    -o, --output <dir>
```

- **Default = content-exact**: decompress `data.zst`, split by manifest sizes, add
  verbatim `media/`, re-zip in original order (mimetype first/stored) → a valid,
  content-identical `.epub`.
- **`--target 3.3`**: run the existing `convert()` on the restored EPUB (3.3 is the
  only published target today; `EpubVersion` gains 3.4 when that spec ships, and the
  flag follows). Reuses the same options the converter/web UI already expose
  ("Target version" picker). Re-targeting can only derive from **what the original
  contains** (we can't recreate a JPEG the source never had).
- **Deferred:** `--target 2.0` (downgrade — separate large feature) and `--exact`
  (bit-exact whole-file — preservation tier).

The web service mirrors this as a visible dropdown; the CLI is flag-driven with a
content-exact default (no forced interactive prompt — headless Pi/NAS use must stay
scriptable).

---

## 8. CLI surface

```
epublift archive <path...>     # .epub file(s) or a directory (one .eparc per book)
    -o, --output <dir>         # default: alongside each input
    [--level 19]               # future tunable (L19 = pure-Rust practical max)
    [--max]                    # future, demand-gated: Brotli for the smallest text

epublift restore <file.eparc...>   # see §7
```

`book.epub → book.eparc`; `restore` → `book.epub`. A directory input archives each
book **independently** (per-book `.eparc`, individually restorable) — matching the
personal-library use case, not a single library bundle.

---

## 9. Code reuse (≈ no new dependencies)

| Job | Existing piece |
|---|---|
| Read `.epub` entries (uncompressed) | `zip` crate (`ZipArchive`) |
| Classify stream vs verbatim | `eparc::store_verbatim` (extension-based) |
| Solid zstd encode / decode | `structured-zstd` (the shipped, default-on `archival` feature) |
| Stored-ZIP container + restore re-zip | `zip` crate `ZipWriter` (same as `repackage_epub`) |
| Re-target / modernize restore | `convert()` |
| Per-entry CRC32 | `crc32fast` (already a dependency) |
| Whole-file SHA-256 + JSON manifest | `sha2` + `serde`/`serde_json` (pure Rust; the new adds) |

---

## 10. Phasing

1. **Phase 1 (this design):** `archive` + content-exact `restore`; ZIP container;
   solid zstd L19 compressed stream (text + fonts); verbatim already-compressed
   media; manifest with SHA-256 + per-entry CRC32; forward-compatible
   `media_format` hooks. CLI: `epublift archive` / `restore` subcommands.
2. **Phase 1.5:** `restore --target 3.3`, `--keep-images`, `--kepub`,
   `--modernize` (reuses `convert()`). `--target 3.4` is added when EPUB 3.4 ships.
3. **Phase 2 (with v3.4 image codecs):** lossless image re-pack (faithful, default)
   + opt-in lossy `--lossy-images`; transcode-down on restore (the library-app
   engine). `format_version: 2`.
4. **Later / preservation & library tiers:** bit-exact `--exact` restore; v2.0
   downgrade target; cross-book dedup + corpus shared dictionary.
