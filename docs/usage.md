# Usage guide

Detailed reference for the `epublift` optimizer (the `-i` / convert mode). For a
quick overview see the [README](../README.md); for archiving a library see the
[Archiving guide](archiving.md).

## Basic command

```bash
epublift -i <path_to_input_epub>
```

This modernizes the input and saves it next to the original as
`<input_name>_v3.3.epub`, plus a performance report in `<input_name>_report.txt`.

During development you can also run it directly with Cargo:

```bash
cargo run --release -- -i book.epub
```

## Which option should I use? (quick guide)

Not sure which flags you need? Find your situation below and copy the command. In
every example, replace `book.epub` with the path to your file (you can also just
drag the file into the terminal to paste its path).

**📚 I just want a smaller, modernized EPUB (most people):**

```bash
epublift -i book.epub
```
Upgrades the book to EPUB 3 and shrinks images to WebP. The new file appears next
to the original as `book_v3.3.epub`. Great for Apple Books, Google Play Books,
Calibre, and most reading apps.

**📖 I read on a Kobo (Forma, Sage, Clara, Libra, …):**

```bash
epublift -i book.epub --kepub
```
Makes `book_v3.3.kepub.epub`, tuned for Kobo: faster page turns, reading statistics,
and tap-a-word dictionary. It **keeps your images in their original format by
default**, because **stock Kobo can't display WebP** (see the next item). Copy
the `.kepub.epub` file onto your Kobo over USB (into the `.kobo` folder) or send
it with Calibre.

> **Advanced — `--kepub --kepub-webp`:** if you've installed the
> [Kobo WebP plugin](../kobo-webp-plugin/README.md) on your device, add
> `--kepub-webp` to emit WebP images in the `.kepub` instead of keeping
> originals. Without the plugin, those images show blank — leave it off.

**🖼️ My converted book opens but the images are blank:**

```bash
epublift -i book.epub --keep-images
```
Some readers — most notably **Kobo e-ink devices** — claim to support modern
EPUBs but don't actually draw WebP images, so they show up blank. `--keep-images`
skips the WebP step and keeps your original JPEG/PNG pictures, while still
modernizing the book. (If you use `--kepub`, this is already done for you.)

**🔤 My device or computer mangles the Turkish/accented characters in the filename:**

```bash
epublift -i "Işık Doğudan Yükselir.epub" --ascii
```
Renames the output to plain ASCII (`Isik_Dogudan_Yukselir_v3.3.epub`). Handy for
old devices, SD cards, or sync tools. The book's on-screen title isn't affected.

> 💡 You can combine flags, e.g. `epublift -i book.epub --kepub --ascii`.

## Advanced options

```bash
epublift -i book.epub -o optimized_book.epub -q 85 -r stats_report.txt
```

## Command-line options

| Argument | Long Flag | Description | Default |
| :--- | :--- | :--- | :--- |
| `-i` | `--input` | **[Required]** Path to the original EPUB file | *None* |
| `-o` | `--output` | Path to save the modernized EPUB | `<input>_v3.3.epub` |
| `-q` | `--quality`| WebP compression quality level (1-100) | `80` |
| `-r` | `--report` | Path to write the conversion audit report | `<input>_report.txt` |
| | `--ascii` | Transliterate the auto-generated output/report names to ASCII | *off* |
| | `--keep-images` | Keep original images (skip JPEG/PNG → WebP) for readers that don't render WebP | *off* |
| | `--kepub` | Produce a Kobo `.kepub.epub` (inject `koboSpan` markup; keeps original images by default) | *off* |
| | `--kepub-webp` | With `--kepub`, emit WebP instead of keeping originals — needs the [Kobo WebP plugin](../kobo-webp-plugin/README.md) | *off* |

(For the `archive` / `restore` subcommands, see the [Archiving guide](archiving.md);
for the `meta` subcommand, see [Metadata](metadata.md) and the summary below;
for the experimental `import` subcommand — PDF → reflowable EPUB — see the
[PDF import guide](pdf-import.md).)

### Read & edit metadata (`meta`)

The `meta` subcommand reads, hand-edits, or auto-fills a book's metadata. It never
modifies the input; edits are written to `<name>_meta.epub` (or `-o <path>`).

```bash
# Print the current metadata (add --json for machine output)
epublift meta show book.epub

# Edit by hand (repeat --author / --subject for multiple values)
epublift meta set book.epub --title "…" --author "…" --language tr --series "Dune:2"

# Auto-fill missing fields by ISBN (needs the `metadata` feature)
epublift meta enrich book.epub --isbn 9780… --dry-run                 # Open Library (default)
epublift meta enrich book.epub --isbn 9780… --provider google         # Google Books
```

`meta enrich` is **language-aware**: it fills only fields in the book's own
language (`dc:language`, or `--lang`), matching by ISBN-13 — English work-level
subjects/description are skipped on a non-English book unless `--allow-foreign-meta`.
By default it fills only gaps (`--overwrite` replaces existing fields) and
`--dry-run` previews without writing; `--include-description` opts the description
in. Pick the source with `--provider openlibrary` (default) or `--provider google`;
Google Books shares a small anonymous quota, so set `GOOGLE_BOOKS_API_KEY` for your
own. The lookup uses a **pure-Rust** HTTPS client (rustls + RustCrypto, no C), so
`enrich` is compiled only with the opt-in `metadata` feature
(`cargo build --features metadata`); `show` and `set` are always available and
need no network. Full details: [Metadata](metadata.md).

### Keep original images (`--keep-images`)

By default epublift converts JPEG/PNG to **WebP**, which most readers (Apple Books, Calibre, and other apps) render fine and which gives the biggest size win. But some devices advertise EPUB 3.3 support yet **do not actually render WebP** — notably **Kobo e-ink readers** (Forma, Sage, …), where a WebP-converted book shows blank images. For those, use `--keep-images` to leave images in their original format while still modernizing the structure:

```bash
epublift -i book.epub --keep-images
```

`--kepub` turns this on automatically, since its target is Kobo.

### Kobo `.kepub` output (`--kepub`)

[Kobo](https://www.kobo.com/) e-readers unlock their richer reading features — accurate page turns, reading statistics, and dictionary lookup — when a book carries Kobo's `koboSpan` markup. Add `--kepub` to produce a Kobo-optimized file alongside the normal EPUB 3 upgrades:

```bash
epublift -i book.epub --kepub
# → book_v3.3.kepub.epub
```

The result is still a valid EPUB 3 (Kobo simply keys on the `.kepub.epub` extension and the spans), so the same file also opens in other readers. The transform follows the approach of the open-source [`kepubify`](https://github.com/pgaskin/kepubify): sentence-level spans, each image in its own paragraph, and Kobo's column scaffolding. Sideload the `.kepub.epub` onto your Kobo (into the `.kobo` folder or via Calibre) to use it.

### ASCII-safe filenames (`--ascii`)

By default epublift **preserves your original filename**, only appending the `_v3.3` suffix — so `Işık Doğudan Yükselir.epub` becomes `Işık Doğudan Yükselir_v3.3.epub`. Modern e-readers and filesystems handle these Unicode names without issue, and the title/author shown on your device come from the EPUB's own metadata, not the filename.

If you prefer a shell-friendly, ASCII-only name (handy for the command line, FAT32 SD cards, or older sync tools), add `--ascii`:

```bash
epublift -i "Işık Doğudan Yükselir.epub" --ascii
# → Isik_Dogudan_Yukselir_v3.3.epub
```

This romanizes Unicode letters (e.g. Turkish `ş→s`, `ğ→g`, `ı→i`, `ö→o`, `ü→u`), turns whitespace into underscores, and drops other punctuation. Transliteration is lossy and not always locale-perfect, which is why it is **off by default**. The flag only affects auto-generated names — an explicit `-o`/`-r` path is always used verbatim.

## Quick sandbox testing

A companion binary (`gen-sample`) builds a valid legacy EPUB 2.0 file containing test images and outdated structures, so you can safely evaluate the tool.

```bash
# 1. Generate a legacy sample (creates sample_epub2.epub in the current folder):
cargo run --release --bin gen-sample

# 2. Run epublift on it (→ sample_epub2_v3.3.epub + sample_epub2_report.txt):
cargo run --release --bin epublift -- -i sample_epub2.epub

# 3. Inspect the audit report:
cat sample_epub2_report.txt
```
