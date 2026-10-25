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

Print the version you are running (with the exact commit when the build knew
it, e.g. `epublift 4.4.1+…`):

```bash
epublift --version        # or -V
```

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
Upgrades the book to EPUB 3 and keeps your images exactly as they are. The new
file appears next to the original as `book_v3.3.epub`. Add `--webp` to also
shrink JPEG/PNG images to WebP, for readers that show WebP (Apple Books, Google
Play Books, calibre, most reading apps — not Kobo e-ink; see below).

**📖 I read on a Kobo (Forma, Sage, Clara, Libra, …):**

```bash
epublift -i book.epub --kepub
```
Makes `book_v3.3.kepub.epub`, tuned for Kobo: faster page turns, reading statistics,
and tap-a-word dictionary. Your images stay in their original format,
which matters here: **stock Kobo can't display WebP** (see the next item). Copy
the `.kepub.epub` file onto your Kobo over USB (into the `.kobo` folder) or send
it with Calibre.

> **Advanced — `--kepub --kepub-webp`:** if you've installed the
> [Kobo WebP plugin](../kobo-webp-plugin/README.md) on your device, add
> `--kepub-webp` to emit WebP images in the `.kepub` instead of keeping
> originals. Without the plugin, those images show blank — leave it off.

**🖼️ My converted book opens but the images are blank:**

You converted with `--webp`, and your reader does not draw WebP. Some readers —
most notably **Kobo e-ink devices** — claim to support modern EPUBs but show
WebP images blank. Convert again from the original without `--webp`: the
default keeps your JPEG/PNG pictures while still modernizing the book.

**🔤 My device or computer mangles the Turkish/accented characters in the filename:**

```bash
epublift -i "Işık Doğudan Yükselir.epub" --ascii
```
Renames the output to plain ASCII (`Isik_Dogudan_Yukselir_v3.3.epub`). Handy for
old devices, SD cards, or sync tools. The book's on-screen title isn't affected.

> 💡 You can combine flags, e.g. `epublift -i book.epub --kepub --ascii`.

## Advanced options

```bash
epublift -i book.epub -o optimized_book.epub --quality 85 --report stats_report.txt
```

## Command-line options

| Argument | Long Flag | Description | Default |
| :--- | :--- | :--- | :--- |
| `-i` | `--input` | **[Required]** Path to the original EPUB file | *None* |
| `-o` | `--output` | Path to save the modernized EPUB | `<input>_v3.3.epub` |
| | `--quality`| Image quality (1-100) when images are re-encoded (`--webp`, `--target 3.4`). `-q` still works, with a warning, until 5.0.0 | `80` |
| | `--report` | Path to write the conversion audit report. `-r` still works, with a warning, until 5.0.0 | `<input>_report.txt`, beside the output |
| `-f` | `--force` | Permit replacing existing output files (never the input). Until 5.0.0 an existing file is still replaced without it, with a warning | *off* |
| | `--format` | Report format: `human`, the only one this command has | `human` |
| | `--ascii` | Transliterate the auto-generated output/report names to ASCII | *off* |
| | `--webp` | Re-encode JPEG/PNG images to WebP (size-safe: only when smaller) | *off* |
| | `--keep-images` | Keep original images. The default; kept so existing scripts work, and it overrides `--target 3.4`'s image conversion | *on* |
| | `--kepub` | Produce a Kobo `.kepub.epub` (inject `koboSpan` markup; keeps original images by default) | *off* |
| | `--kepub-webp` | With `--kepub`, emit WebP instead of keeping originals — needs the [Kobo WebP plugin](../kobo-webp-plugin/README.md) | *off* |

(For the `archive` / `restore` subcommands, see the [Archiving guide](archiving.md);
for the `meta` subcommand, see [Metadata](metadata.md) and the summary below;
for the experimental `import` subcommand — PDF → reflowable EPUB — see the
[PDF import guide](pdf-import.md), and Markdown → reflowable EPUB in the
[Markdown import guide](markdown-import.md).)

### Moving to 5.0.0

epublift is moving to the [veripublica CLI conventions](https://github.com/veripublica/conventions/blob/main/CLI.md),
the command line its sibling tools (epubveri, epubsana, kepubverto) already
share. Since 4.5.0 every command takes `-i` and `--format`, `-f/--force`
exists, `-h` and `--help` print the same help, and a flag given twice is
the same as once. Until 5.0.0, these old spellings still work, each with a
`warning:` on stderr:

| Today (warns) | From 5.0.0 |
| :--- | :--- |
| A path without `-i` (`epublift check book.epub`) | `epublift check -i book.epub` (repeat `-i` for more books in `check`) |
| `archive` / `restore` with several paths | one `-i` per run: a file or a folder |
| `-q 85` / `-r report.txt` (default command, `restore`) | `--quality 85` / `--report report.txt` (`-q` is the family's `--quiet`) |
| An existing output file is replaced | refused (exit 2) unless `-f` is given |

An `-o` that names the input has been refused (exit 2) since 4.5.0, with or
without `-f`.

### Read & edit metadata (`meta`)

The `meta` subcommand reads, hand-edits, or auto-fills a book's metadata. It never
modifies the input; edits are written to `<name>_meta.epub` (or `-o <path>`).

```bash
# Print the current metadata (add --format metadata for JSON)
epublift meta show -i book.epub

# Edit by hand (repeat --author / --subject for multiple values)
epublift meta set -i book.epub --title "…" --author "…" --language tr --series "Dune:2"

# Auto-fill missing fields by ISBN (needs the `metadata` feature)
epublift meta enrich -i book.epub --isbn 9780… --dry-run                 # Open Library (default)
epublift meta enrich -i book.epub --isbn 9780… --provider google         # Google Books
```

`meta enrich` is **language-aware**: it fills only fields in the book's own
language (`dc:language`, or `--assume-language`), matching by ISBN-13 — English work-level
subjects/description are skipped on a non-English book unless `--allow-foreign-meta`.
By default it fills only gaps (`--overwrite` replaces existing fields) and
`--dry-run` previews without writing; `--include-description` opts the description
in. Pick the source with `--provider openlibrary` (default) or `--provider google`;
Google Books shares a small anonymous quota, so set `GOOGLE_BOOKS_API_KEY` for your
own. The lookup uses a **pure-Rust** HTTPS client (rustls + RustCrypto, no C), so
`enrich` is compiled only with the opt-in `metadata` feature
(`cargo build --features metadata`); `show` and `set` are always available and
need no network. Full details: [Metadata](metadata.md).

### Validate an EPUB (`check`)

The `check` subcommand validates one or more EPUBs against the spec and reports
problems with epubcheck-compatible message IDs. It uses
[epubveri](https://github.com/veripublica/epubveri) — our own **pure-Rust, JVM-free**
alternative to W3C `epubcheck` — so there's no Java and no external process.

```sh
epublift check -i book.epub                 # human-readable report
epublift check -i a.epub -i b.epub --quiet  # only print files with problems
epublift check -i book.epub --format json   # machine-readable: one envelope per run
epublift check -i dict.epub --profile dict  # validate against an extension profile
```

The exit code follows grep's convention — `0` if every input is valid (no
error- or fatal-severity finding), `1` if any book is invalid, `2` if an input
couldn't be read at all — so it drops straight into a build script: `for f in dist/*.epub; do epublift check -i "$f" || exit 1; done`. It's
compiled only with the opt-in `validate` feature
(`cargo build --features validate`; included in the release binaries). epubveri
is **pre-1.0** (99.7% message-ID recall on epubcheck's own test suite — not yet
full parity), so
treat a clean check as strong evidence, not a conformance certificate. Full
details: [Validation](validate.md).

### Images: kept by default, WebP with `--webp`

Since cli-v4.0.0, epublift **keeps JPEG and PNG images as they are** unless you ask otherwise. Some devices advertise EPUB 3.3 support yet **do not render WebP** — notably **Kobo e-ink readers** (Forma, Sage, …), where a WebP-converted book shows blank images — so keeping images is the choice that works everywhere.

`--webp` re-encodes them to **WebP**, which Apple Books, calibre and most reading apps show fine, for the biggest size win. It is size-safe: an image is replaced only when the WebP is smaller.

```bash
epublift -i book.epub          # images kept
epublift -i book.epub --webp   # images re-encoded to WebP where smaller
```

`--keep-images` (the old way to ask for the default) still works. `--target 3.4` re-encodes images to its new formats (AVIF/JPEG XL), because that is what asking for 3.4 is for; add `--keep-images` to upgrade to 3.4 without touching them.

### Presentational attributes: written as CSS

EPUB 2 books are XHTML 1.1, which still allowed layout attributes that HTML,
and so EPUB 3, does not: an image sized in percent (`<img width="40%">`), or a
table's `cellspacing`, `cellpadding`, `border`, `width` and `height`, and
`width`, `height` and `valign` on its cells, or a line break's `clear`
(`<br clear="all">`). Left as they are, they make a
valid EPUB 2 book an invalid EPUB 3 one. The upgrade writes each one as the CSS
the HTML Standard maps it to (§15 Rendering): a class rule in one stylesheet of
its own, `epublift-presentational.css`, added to the manifest and linked first
in each document that uses it.

```html
<img src="map.jpg" alt="" width="40%"/>                      <!-- before -->
<img class="epublift-width-40pct" src="map.jpg" alt=""/>     <!-- after  -->
```

```css
.epublift-width-40pct { width: 40% }
```

The book looks the same, because the conversion is made only where nothing can
change:

- **The book's CSS doesn't set the property.** A reader already ignores the
  attribute where a rule of the book's own CSS (or a `style` attribute) sets the
  same property. There, and where a rule *may* set it (under `@media`, say), the
  attribute is left as written and counted in the report. The question is
  answered by [styloria](https://github.com/veripublica/styloria)'s cascade.
- **The book's selectors match as before.** A selector such as `img[width]`
  notices the attribute going; when one would match differently, the document
  is left as it is.
- **Every declaration written is valid CSS.**

A class rule, not a `style` attribute, because the attribute was a
low-precedence hint and a class rule keeps it close to one; old readers (Kobo's
engine among them) read it. Values HTML reads the same way in a valid form are
rewritten instead of moved to CSS (`width="60px"` → `width="60"`), and ones that
gave no hint at all are removed (`width="auto"`, `border="0"`). Valid values,
such as `<img width="300">`, are untouched.

The same goes for the elements HTML made obsolete. Each is renamed to its HTML
counterpart, with a class rule for what the old element's rendering added:
`<strike>` becomes `<s>` (readers draw both the same way), `<big>` becomes a
`span` at `font-size: larger`, `<center>` a `div` at `text-align: center`, and
`<font face="…" size="…">` a `span` with `font-family` and the legacy size
keyword (`size="2"` → `small`, `+1` → `large`). The same checks apply, with a
type selector such as `font { … }` or `div { … }` in the book's CSS counting as
a selector that would match differently. Left as written: a `center` around
anything but text, images and inline markup (`<center>` also centres blocks,
which `text-align` does not), `<font color>`, and `size="7"`, whose
`xxx-large` older readers don't know.

### Kobo `.kepub` output (`--kepub`)

[Kobo](https://www.kobo.com/) e-readers unlock their richer reading features — accurate page turns, reading statistics, and dictionary lookup — when a book carries Kobo's `koboSpan` markup. Add `--kepub` to produce a Kobo-optimized file alongside the normal EPUB 3 upgrades:

```bash
epublift -i book.epub --kepub
# → book_v3.3.kepub.epub
```

The result is still a valid EPUB 3 (Kobo simply keys on the `.kepub.epub` extension and the spans), so the same file also opens in other readers. The markup is added by the family's [kepubverto](https://github.com/veripublica/kepubverto), with the same span ids as the open-source [`kepubify`](https://github.com/pgaskin/kepubify) (so reading positions and highlights survive a switch): sentence-level spans, each image in its own paragraph, and Kobo's column scaffolding. kepubverto inserts its markup without re-serializing the document, so every other byte stays as it was. A document it cannot convert (not UTF-8, not well-formed) is left as it was, with a warning. `--kepub` cannot be combined with the experimental Zstandard packaging. Sideload the `.kepub.epub` onto your Kobo (into the `.kobo` folder or via Calibre) to use it.

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
cargo run --release --features gen-sample --bin gen-sample

# 2. Run epublift on it (→ sample_epub2_v3.3.epub + sample_epub2_report.txt):
cargo run --release --bin epublift -- -i sample_epub2.epub

# 3. Inspect the audit report:
cat sample_epub2_report.txt
```
