# Metadata enrichment & editing

This document specifies ePubLift's metadata feature: editing an EPUB's Dublin
Core metadata by hand, or auto-filling missing fields from an online catalogue by
**ISBN**. It is the design contract for the CLI (`meta` subcommand) and the later
web form.

Status: **planned** (cli-v1.6.0 → web-v1.7.0). See the
[roadmap](../ROADMAP.md#-planned--metadata-enrichment--editing--cli-v160--web-v170).

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

Always refreshed: **`dcterms:modified`**. Page count (`number_of_pages`) is *not*
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

## CLI

```
epublift meta show   book.epub                       # print current metadata (table; --json for machine output)

epublift meta set    --title "…" --author "…" \      # manual edit (repeatable flags for multi-valued fields)
                     --subject "…" --series "…:1" book.epub

epublift meta enrich --isbn 9780… [--lang tr] \      # auto-fill missing fields from a provider
                     [--provider openlibrary] \
                     [--dry-run] [--overwrite] [--allow-foreign-meta] \
                     [--include-description] book.epub
```

- `show` and `set` are always available and **offline**. `enrich` needs the
  `metadata` build feature (pulls in the HTTP client).
- `enrich` defaults to **fill-gaps + preview** (`--dry-run` shows the diff without
  writing). `--overwrite` replaces existing fields; `--allow-foreign-meta` keeps
  language-mismatched fields; `--include-description` opts the (often
  publisher-authored) description in.
- Output naming follows the project convention; the input is never mutated.

## Providers

A `MetadataProvider` trait carries a `locale`/`lang` from the start so each
backend can request language-appropriate data:

1. **Open Library** (first) — `GET /isbn/<isbn>.json`, then `works[0].key` for
   description/subjects; `Accept: application/json`, a descriptive `User-Agent`
   per their policy, gentle rate limiting.
2. **Google Books** (next) — `volumes?q=isbn:<isbn>&langRestrict=<lang>`.
3. **Amazon** (next) — by ASIN against the locale's regional endpoint
   (e.g. `.com.tr` for Turkish).

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
