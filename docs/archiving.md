# Archiving guide — `archive` / `restore` (`.eparc`)

Shrink a personal EPUB collection to save disk space, and get any book back on
demand. `epublift archive` packs a book into a compact **`.eparc`** archive;
`epublift restore` brings it back. It's **lossless** — your originals are always
recoverable — and runs anywhere from a Raspberry Pi to a NAS or desktop.

For the optimizer (`-i` / convert mode), see the [Usage guide](usage.md).

## Archive

```bash
# Archive one book, or a whole folder (one .eparc per book):
epublift archive -i book.epub
epublift archive -i ~/Books -o ~/Archive      # recurses the folder
```

`archive` accepts EPUB files or directories (scanned recursively); it writes one
`.eparc` per book and prints a per-book and library-wide size summary. Use `-o`
to choose an output directory (default: next to each input).

## Restore

By default you get back a **content-exact** `.epub` — the original book,
byte-for-byte per entry:

```bash
epublift restore -i book.eparc
```

`restore` also accepts directories of `.eparc` files, and `-o` for the output
directory.

### Restore for a specific device

The archive is your **canonical master** — on restore you pick the output for the
reader you're using (nothing extra is stored; it re-runs the optimizer):

```bash
epublift restore -i book.eparc                 # content-exact original (default)
epublift restore -i book.eparc --target 3.3    # re-emit as EPUB 3.3 (original images)
epublift restore -i book.eparc --webp          # re-emit as EPUB 3.3, images re-encoded to WebP
epublift restore -i book.eparc --kepub         # → book_v3.3.kepub.epub for Kobo
```

| Flag | Effect |
| :--- | :--- |
| *(none)* | Content-exact original `.epub` |
| `--target 3.3` | Re-emit at EPUB 3.3 (original JPEG/PNG kept) |
| `--modernize` | Alias for `--target 3.3` |
| `--webp` | Re-target, and re-encode JPEG/PNG to WebP where smaller |
| `--keep-images` | Re-target, keeping original images (the default; kept for existing scripts) |
| `--kepub` | Produce a Kobo `.kepub.epub` |
| `--quality` | WebP quality (1-100) when re-targeting (`-q` still works, with a warning, until 5.0.0) |
| `-f`, `--force` | Permit replacing existing `.epub` files (until 5.0.0 they are still replaced without it, with a warning) |

> EPUB **3.4** (AVIF/JXL) isn't published yet, so `--target` accepts **3.3**
> today; it'll gain 3.4 when that spec ships.

## How it works

The compressible parts of a book (XHTML/CSS/OPF and **fonts**) are packed into a
single solid [Zstandard](https://facebook.github.io/zstd/) stream, while
already-compressed media (images, audio, WOFF) is stored verbatim — so the archive
**never grows a book**. A `manifest.json` records the original's SHA-256 and a
CRC32 per entry for integrity. The archive is just a ZIP, so even without epublift
a future you can `unzip book.eparc` and `zstd -d data.zst`. Text-heavy books
typically shrink ~30%; image-heavy ones less (their images are already
compressed).

The design and the measured codec rationale live in
[`design/eparc-format.md`](design/eparc-format.md) and
[`design/eparc-codec-choice.md`](design/eparc-codec-choice.md).
