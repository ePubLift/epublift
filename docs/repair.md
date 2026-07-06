# Fix common EPUB issues (`repair`)

This document specifies ePubLift's repair feature: fixing a handful of common
structural defects in an EPUB's OPF package document. It's a companion to
[validate](validate.md) — Validate diagnoses, Repair fixes what it can.

Status: **shipped** — CLI `repair` subcommand (default build, no opt-in feature
needed) and a web **Repair** mode.

## Scope

Repair fixes exactly three classes of defect, all scoped to the **package
document** (`<manifest>`/`<spine>`/`<metadata>`) — content documents (chapter
XHTML) are never touched. This is deliberately narrow: repair closes the
optimizer's OPF-cleaning gap, it isn't a general-purpose EPUB fixer.

1. **Duplicate spine itemrefs** — a `<spine>` that lists the same `idref` more
   than once. The first occurrence is kept, later ones are dropped.
2. **Empty/legacy metadata** — Dublin Core elements (`dc:title`, `dc:date`,
   `dc:publisher`, …) whose text content is empty or whitespace-only. Dropping
   one that carries an `id` also drops any `<meta refines="#id">` that
   describes it, so cleanup never leaves an orphaned `refines`.
   - **Safety guard:** an empty `dc:identifier` that is the package's
     `unique-identifier` is never dropped, and an empty required element
     (`dc:title`/`dc:identifier`/`dc:language`) is only dropped when a
     non-empty sibling of the same type survives — repair never leaves a book
     with zero titles, identifiers, or languages.
3. **Dangling references** — a manifest `<item>` whose `href` doesn't resolve
   to a real file inside the archive, and a spine `<itemref>` whose `idref`
   doesn't match any surviving manifest item (this also catches an itemref
   that pointed at a manifest item repair just removed). Remote (`https://…`)
   and `data:` hrefs are never checked — only local archive paths.

Repair only **removes** content, it never rewrites or invents anything. If
none of the above apply, the output's OPF is byte-for-byte identical to the
input.

*(Later, not built)* Content-document-level link checking — a broken `href`
inside a chapter's XHTML — is out of scope for this pass.

## Usage

```sh
epublift repair book.epub
```

Writes `book_repaired.epub` next to the input (the input is never modified)
and prints a summary of what was fixed:

```
[+] Wrote repaired EPUB to: book_repaired.epub
  - 1 duplicate spine itemref(s) removed
  - 2 empty/legacy metadata element(s) removed
    · spine itemref idref="c1" (duplicate; kept first occurrence)
    · dc:date (empty)
    · dc:publisher (empty)
```

An already-clean book prints `[=] No issues found — this EPUB's package
document is already clean.`

### Options

| Flag | Meaning |
| --- | --- |
| `-o`, `--output <PATH>` | Output path. Default: `<name>_repaired.epub` next to the input. |
| `--dry-run` | Report what would be fixed without writing anything. |

## In the web app (`epublift-web`)

A **Repair** mode sits between Validate and Archive: drop an `.epub`, click
**Fix my EPUB**, get a summary of what was fixed plus a download link.

Repair is a separate tab from Validate — Validate stays read-only/diagnostic
(and runs entirely client-side via WebAssembly), while Repair is a normal
upload → transform → download tool like Optimize/Archive/Restore, so it needs
the file server-side. When Validate finds problems, its result view shows a
**"Fix these issues →"** button that switches to the Repair tab with the
already-selected file carried over (no re-upload) — the user still clicks the
Repair CTA explicitly to run it. Available in all 13 UI languages.

## Notes

- Repair and Validate are independent: after repairing, re-run Validate to
  confirm the fix, since epubveri may still flag issues outside repair's scope
  (missing accessibility metadata, content-document problems, etc.).
- Only the OPF entry in the zip is rewritten; every other entry is copied
  through unchanged, exactly like `meta set`/`meta enrich`.
