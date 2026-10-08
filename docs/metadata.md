# Metadata enrichment & editing

This document specifies ePubLift's metadata feature: editing an EPUB's Dublin
Core metadata by hand, or auto-filling missing fields from an online catalogue by
**ISBN**. It covers both the CLI (`meta` subcommand) and the web form.

Status: **shipped** — CLI in **cli-v1.6.0**, web Metadata mode in **web-v1.7.0**.
**Open Library** (default) and **Google Books** are supported. (Amazon was
evaluated and dropped — see [Providers](#providers).)

## Principles

1. **Stateless, single-file.** This is a tool that operates on one EPUB at a
   time — not a library/catalogue. No database, no accounts.
2. **Offline-first / pure-Rust.** The core writer and `meta show`/`meta set`
   need no network and ship in the default build. Only `meta enrich` reaches out,
   behind the opt-in `metadata` build feature. The HTTPS client is **100% pure
   Rust** — `rustls` with the **RustCrypto** crypto provider (no `ring`/`aws-lc`,
   no C toolchain) + `webpki-roots`, over a small hand-rolled HTTP/1.1 client
   (`src/http.rs`). **No translation services, ever.**
3. **Never destroy author intent.** Fill *gaps* by default; never overwrite an
   existing field without `--overwrite`. The input file is never mutated unless
   the whole operation succeeds (existing project rule).
4. **Language-aware (critical).** Metadata is written in the **book's own
   language** (`dc:language`). See [Language policy](#language-policy).
5. **Provenance.** Every auto-filled field is stamped with its source and date,
   so enrichment is auditable — consistent with the `.eparc` archival ethos.

## Language policy

A Turkish book gets Turkish metadata; a Korean book gets Korean metadata. We do
**not** translate. Instead:

- **Match by ISBN-13 exactly — never a fuzzy title search.** A book's ISBN
  resolves to *that* edition, whose `title`/`subtitle`/`publisher`/`by_statement`
  are already in the edition's language. Title search would risk matching an
  English edition.
- Each fetched field is tagged with its (inferred) language. **Edition-level**
  fields (title, subtitle, publisher, contributors, series) inherit the edition
  language; **work-level** fields (`description`, `subjects`, classifications) are
  language-agnostic and usually English.
- **Fields whose language ≠ the book's `dc:language` are skipped by default.** So
  English subjects/description never land in a Turkish book. Override with
  `--allow-foreign-meta`.
- **Subjects are offered, never written unasked.** "Usually English" does not
  hold for subjects: Open Library merges the headings of every library that
  catalogued the book, in that library's language, with no language tag — for
  an English programming book (ISBN 9780137909100) it returns German
  (`Datenverarbeitung`, `Einführung`) and Dutch (`Coderingstheorie`) headings
  next to the Library of Congress ones. Short headings cannot be told apart by
  language (`Hardware` and `Computers` are German too), so the CLI lists them
  and writes them only with `--include-subjects`, and the web form shows them
  as tick boxes, none ticked.
- **Catalogue text is cleaned:** HTML entities decoded (`Computers &amp; the
  internet`), Unicode composed (NFC), and duplicate subjects dropped — compared
  without case, accents and spaces, since the same heading also arrives broken
  (`Einfu hrung` next to `Einführung`).
- If the matched edition's language disagrees with the book's `dc:language`, the
  tool **warns** (the ISBN may point at a different-language edition).
- If the book has no `dc:language`, `enrich` requires `--lang <BCP-47>` or aborts
  with a clear message.
- The list of skipped, language-mismatched fields is reported — it doubles as a
  "what to enrich upstream at Open Library" list for a future contribution effort.

Encoding: UTF-8 throughout, **no transliteration**. For non-Latin scripts
(e.g. Korean) the creator `file-as` (sort) value defaults to the display value
rather than being ASCII-folded.

## Field map (EPUB OPF 3.3 / 3.4 ↔ Open Library)

`description`/`subjects` usually live on the Open Library **work** record, so the
auto path fetches `/isbn/<isbn>.json` (edition) and then follows `works[0].key`.

| Group | Field | EPUB OPF target | Open Library source |
| --- | --- | --- | --- |
| **A. Core (required)** | Title | `dc:title` + `meta property="title-type">main` | `title` |
| | Subtitle | `dc:title` + `title-type=subtitle` | `subtitle` |
| | Author(s) | `dc:creator` + `role=aut` + `file-as` + `display-seq` | `authors[].name` |
| | Language | `dc:language` (BCP-47) | `languages[]` (`/languages/eng` → `en`) |
| | Identifier (unique) | `dc:identifier` (UUID/URN, the `unique-identifier`) | preserved / generated |
| **B. Publication** | Publisher | `dc:publisher` | `publishers[].name` |
| | Publication date | `dc:date` (normalised to ISO 8601) | `publish_date` |
| | Description | `dc:description` | **work**.`description` |
| | Subjects / tags | `dc:subject` (+ `authority`/`term`) | `subjects`, `subject_{people,places,times}` |
| | Cover image | manifest `properties="cover-image"` | `cover.large` / `covers[]` |
| | Series + position | `belongs-to-collection` + `group-position` | `series` |
| **C. Contributors** | Translator / editor / illustrator / narrator | `dc:contributor` + `role` (`trl`/`edt`/`ill`/`nrt`) | `contributions`, `by_statement` |
| **D. Identifiers** | ISBN-13 / ISBN-10 | `dc:identifier` (`urn:isbn:…`, id `epublift-isbn`) — recognized as the ISBN by Calibre / Apple Books | `isbn_13` / `isbn_10` |
| | ASIN / Google / Goodreads / LibraryThing / OCLC / LCCN / DOI / OLID | extra `dc:identifier` + `identifier-type` | `identifiers{}` |
| **E. Classification** *(phase 2)* | Dewey / LCC | `dc:subject` (with `authority`) | `classifications.{dewey_decimal_class,lc_classifications}` |
| **F. Accessibility** *(phase 2)* | accessMode / accessibilityFeature / Hazard / Summary / conformsTo | `meta property="schema:…"` | not in OL → manual / smart default |
| **G. Rights** | Rights / licence | `dc:rights` | manual |

Refreshed on every edit of an **EPUB 3** book: **`dcterms:modified`** (EPUB 2
has no such field). Page count (`number_of_pages`) is *not*
a standard reflowable-EPUB field — shown for information, never written.

First release scope: **A + B + C + D**. Groups **E + F** are phase 2.

### EPUB 3.3 vs 3.4: no difference (writer is version-agnostic)

The bibliographic vocabulary we read and write is **identical** in EPUB 3.3 and
3.4 — same Dublin Core terms and the same refinement meta properties
(`title-type`, `file-as`, `role`, `display-seq`, `identifier-type`, `authority`,
`term`, `belongs-to-collection`, `collection-type`, `group-position`). So the
metadata writer needs **no version branch**. The only 3.4 metadata-area deltas
don't touch our fields: `pageBreakSource` replaces `source-of` (a pagination
refinement, already handled in `opf.rs`), and the package-level `<collection>`
element became "obsolete but conforming" (unrelated to the `belongs-to-collection`
meta property we use for series). See [`docs/epub-3.4.md`](epub-3.4.md).

### EPUB 2: written in EPUB 2's own syntax

EPUB 2 has no `refines`, no `property` metas and no `dcterms:modified`; writing
them is an error there (RSC-005). An EPUB 2 book is written the OPF 2 way:

| Field | EPUB 3 | EPUB 2 |
| --- | --- | --- |
| Title | `dc:title` + `title-type` main | `dc:title` |
| Subtitle | `dc:title` + `title-type` subtitle | **refused** — EPUB 2 has no subtitle field; put it in the title, or upgrade the book first |
| Author | `dc:creator` + `role` / `file-as` / `display-seq` refinements | `dc:creator opf:role opf:file-as` |
| ISBN | `dc:identifier` `urn:isbn:…` | `dc:identifier opf:scheme="ISBN"` |
| Series | `belongs-to-collection` + `group-position` | `calibre:series` / `calibre:series_index` metas |
| Modified | `dcterms:modified` refreshed | — |

Two rules hold for both versions:

- **A field whose value the book already has is not touched.** The web form
  sends every field it shows; re-writing an unchanged one would lose what the
  form does not carry (a second `dc:language`, the markup of an existing ISBN).
- **An author who stays keeps their role and sort name** (`file-as`), matched by
  name in order, so a translator listed as a creator is not turned into an
  author.

Prefixes are taken from the book: Calibre 0.7 wrote the OPF namespace as `ns0:`
and declared Dublin Core on each element, and the writer follows that instead of
assuming `dc:` and `opf:`.

Measured on our 544-book shelf (before → after this rule set, cli-v3.2.0 →
next): re-writing a book's title made **457 of 457 EPUB 2 books** invalid (2,285
RSC-005) and failed outright on 6; now re-saving every field with its own value
changes nothing on any book, a real edit (title, authors, publisher, series,
ISBN) adds no error on any book, and every author keeps role and sort name.

## CLI

```
epublift meta show   book.epub                       # print current metadata and the cover (table; --format metadata for JSON)

epublift meta set    --title "…" --author "…" \      # manual edit (repeatable flags for multi-valued fields)
                     --subject "…" --series "…:1" \
                     [--cover new.jpg] book.epub      # --cover: make this JPEG/PNG the cover

epublift meta enrich --isbn 9780… [--lang tr] \      # auto-fill missing fields from a provider
                     [--provider openlibrary] \
                     [--dry-run] [--overwrite] [--allow-foreign-meta] \
                     [--include-description] [--include-subjects] \
                     [--cover] book.epub
```

- `show` and `set` are always available and **offline**. `enrich` needs the
  `metadata` build feature (pulls in the HTTP client).
- `enrich` defaults to **fill-gaps + preview** (`--dry-run` shows the diff without
  writing). `--overwrite` replaces existing fields; `--allow-foreign-meta` keeps
  language-mismatched fields; `--include-description` opts the (often
  publisher-authored) description in; `--include-subjects` opts the subjects
  in (they are listed either way).
- `enrich` shows the catalogue's cover next to the book's own (sizes, and a
  note when it is smaller); `--cover` makes it the book's cover.
- `--series` takes `Name` or `Name:position`; the position starts with a digit
  (`Dune:2`, `Dune:1-6`), so a colon inside a name stays in the name.
- Output naming follows the project convention; the input is never mutated.

## Cover

`meta set --cover`, `meta enrich --cover` and the web form's Cover section
replace the book's cover, or add one to a book that has none.

- **JPEG or PNG, at most 10 MB, put in as given** — no re-encode, no resize
  (Optimize shrinks it afterwards if wanted). Kobo shows no WebP cover in a
  plain `.epub`, which is why the cover is not converted. The format is read
  from the file's bytes, and the image must decode fully.
- **What changes around it.** The declared cover's file is replaced (renamed
  when the format changes, e.g. `cover.jpeg` → `cover.png`), and every
  reference that *resolves* to it follows — pages, stylesheets, a `<guide>`
  entry; a file with a similar name (`backcover.jpg`) is never touched. A cover
  page that wraps the image in `<svg viewBox="0 0 W H">` (Calibre's) gets the
  new image's size, or the new cover would be stretched to the old one's shape.
- **A book with no declared cover** gets one: the image (`cover-image` in EPUB
  3, `<meta name="cover">` in both), and a cover page first in the reading
  order (with a viewport in a fixed-layout book). Existing pages are not
  touched, even when the first one shows a picture.
- **Checked:** if the result has more fatal errors and errors than the input,
  nothing is written.
- **Catalogue covers** are fetched by the server, from `covers.openlibrary.org`
  and the Internet Archive only (the URL comes from the catalogue's JSON, so the
  hosts are fixed), and shown to the browser as a `data:` URL — the browser
  never contacts a catalogue. They are only offered: on our shelf Open Library
  had a cover for 82 of 120 ISBNs, and it was smaller than the book's own in 62
  of 80. Google Books covers are not offered yet.
- **Kobo:** the library caches cover thumbnails; delete the old copy from the
  device before copying the new one.

Measured on the 544-book shelf with a 1200×1800 JPEG and a 1000×1500 PNG:
every book got the new cover (512 replaced, 32 added), none has more errors
than before, and all 405 SVG cover pages took the new size.

## Providers

Each provider returns a normalized `Fetched` (carrying the edition `language` for
the policy); `enrich::fetch_isbn(provider, isbn, …)` dispatches by name. Selectable
with `meta enrich --provider <name>` (CLI) or the provider dropdown (web).

1. **Open Library** (`openlibrary`, default) — `GET /isbn/<isbn>.json` for the
   language + work link, the flattened `jscmd=data` view for names/subjects, and
   `works[0]` for the description (only with `--include-description`).
2. **Google Books** (`google`) — a single `GET /books/v1/volumes?q=isbn:<isbn>`
   returns title/authors/publisher/date/categories, the ISBNs, and the edition
   `language` (BCP-47) — no follow-up request. Anonymous calls share a small daily
   quota (HTTP 429 when exhausted); set the `GOOGLE_BOOKS_API_KEY` environment
   variable to use your own key and raise it.
**Amazon — evaluated and dropped.** Amazon has no usable metadata API (PA-API
requires an affiliate account with qualifying sales), so it would require
**scraping**: against Amazon's ToS, blocked from datacenter IPs (would need a
headless browser, breaking the pure-Rust/C-free guarantee), brittle on every page
change, and a liability for the public hosted instance. The two open-API providers
cover the need. A future third provider, if added, should be a clean API / open
data — e.g. **Wikidata** (open SPARQL, multilingual) or a **national-library SRU**
endpoint for better local-language coverage.

> We match by ISBN-13 exactly and apply the language gate ourselves (using the
> edition's `language`), rather than relying on a provider's `langRestrict` — so a
> mismatched edition is reported, not silently dropped.

## Open Library response shape (reference)

Observed fields (`jscmd=data` and `/isbn/<isbn>.json`), for the mapping above:

- Edition (`/isbn/<isbn>.json`): `title`, `subtitle`, `authors[]`,
  `contributions[]`, `publishers[]`, `publish_date`, `covers[]`, `languages[]`,
  `isbn_10`, `isbn_13`, `identifiers{}`, `series`, `works[]`, `classifications{}`,
  `number_of_pages`, `first_sentence`.
- Work (`/works/<id>.json`): `description`, `subjects`, `subject_people`,
  `subject_places`, `subject_times`, `covers[]`, `first_publish_date`.

Sources: [Open Library Books API](https://openlibrary.org/dev/docs/api/books),
[Open Library Developers / APIs](https://openlibrary.org/developers/api).
