# Fix EPUB issues (`repair`)

`epublift repair` fixes the defects that [`check`](validate.md) finds, where
there is a fix that is certain to be right. It is a companion to `check`:
Validate diagnoses, Repair fixes what it can.

Status: **shipped**, in the default build. The CLI runs
[epubsana](https://github.com/veripublica/epubsana), the veripublica family's
repairer, since cli-v3.0.0. The web **Repair** mode still runs
the older, narrower repair described [at the end](#in-the-web-app-epublift-web)
until it moves to epubsana too.

## What it does

Repair reads the book, validates it with epubveri, and plans a fix for each
finding that has one. It applies the fixes that are approved, validates the
result again, and writes a new file. The input is never modified.

The repair itself is epubsana's: `epublift repair` calls the same function the
`epubsana` command and its browser build call, so the same book with the same
fixes approved comes back identical from all three. epubsana fixes undeclared
HTML entities, invalid ids and the links that point at them, repeated or
dangling spine and manifest entries, empty metadata, wrong package versions,
font rules that point at missing fonts, content that needs wrapping in a block
element, and more. Its README lists every fix.

Every fix falls into one of two kinds:

- **Safe**: there is exactly one correct fix, and it keeps the content. These
  are applied without asking.
- **Needs a decision**: a good fix exists, but it involves a choice. Dropping a
  repeated spine entry, for example, assumes the first occurrence is where the
  chapter belongs. Repair asks about each of these.

Two rules protect the book:

- **A fix that makes the book worse is undone.** After the approved fixes are
  applied, the book is validated again. If any finding has become more frequent,
  the fix that caused it is undone and reported as `undone`, and the rest are
  kept.
- **Repair never guesses.** A finding with no fix that is certain to be right is
  left for a person to decide, and `check` still reports it.

### A second pass

Some fixes let epubveri check part of the book it could not check before, for
example correcting a package `version="1.0"` that no validator recognises, or
clearing a fatal error that stopped a chapter from being read. The findings that
then appear were always in the book; they were just not visible when the fixes
were planned. Repair therefore plans again on the result, up to three passes in
all, and reports each pass. On a book with an unrecognised version, one error
before repair can be 29 after the first pass and none after the second.

## Usage

```sh
epublift repair book.epub
```

Writes `book_repaired.epub` next to the input. In a terminal, it applies the
safe fixes and asks about each fix that needs a decision:

```
[?] Drop 1 repeat spine entry for "c1" — keep the first
    why: The spine lists this manifest item more than once, …
    - spine: drop 1 repeat itemref(s) with idref="c1"
    Apply this fix? [y/N] y
  [1] fixed: Map 1 undeclared HTML entity (1×) to characters in c1.xhtml (nbsp)
      - replace &nbsp; → ' ' (1×)
  [2] fixed (needs a decision): Drop 1 repeat spine entry for "c1" — keep the first
      - spine: drop 1 repeat itemref(s) with idref="c1"

  Before: 1 fatal error, 1 error, 0 warnings
  After:  0 fatal errors, 0 errors, 0 warnings

[+] Wrote repaired EPUB to: book_repaired.epub
[+] Goal 'valid' met.
```

Without a terminal (in a script, or with input redirected), there is no one to
ask: safe fixes are applied and the fixes that need a decision are skipped. The
report says how many were skipped. Pass `--yes` to apply them too.

A run that applies no fix writes no file. A book with nothing to fix prints
`[=] Nothing to repair: this EPUB already meets the goal 'valid'.`

### Options

| Flag | Meaning |
| --- | --- |
| `-o`, `--output <PATH>` | Output path. Default: `<name>_repaired.epub` next to the input. It may not be the input. |
| `--dry-run` | Report what would be fixed without writing anything. |
| `-y`, `--yes` | Apply every fix, including those that need a decision, without asking. |
| `--goal <valid\|openable>` | How far to repair. `valid` (the default): no fatal errors and no errors remain. `openable`: no fatal errors remain, so the book opens; errors may still be reported. |
| `--format <human\|json>` | Report format. `json` is the veripublica machine envelope, the same shape `epubsana --format json` prints, with `epublift` as the tool. |

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The goal was met. |
| `1` | The goal was not met: fixes were skipped, or what remains needs a person to decide. `epublift check` lists it. |
| `2` | The book could not be repaired at all: it could not be read, or the output path is the input. |

## In the web app (`epublift-web`)

> The web Repair mode does **not** run epubsana yet. Until it does, it runs the
> older repair below, which fixes only three kinds of package-document defect.
> Its move to epubsana, running in the browser like Validate, is the next step.

A **Repair** mode sits between Validate and Archive: drop an `.epub`, click
**Fix my EPUB**, get a summary of what was fixed plus a download link. It fixes:

1. **Duplicate spine itemrefs**, keeping the first occurrence.
2. **Empty or legacy metadata**: Dublin Core elements with no text, and any
   `<meta refines>` that describes them. An element that holds markup, such as
   a description written as `<p>` paragraphs, is never treated as empty. It
   never leaves a book with no title, identifier or language.
3. **Dangling references**: manifest items whose file is not in the archive,
   and spine entries that point at no manifest item.

It only removes content, and only in the package document. When Validate finds
problems, its result view shows a **"Fix these issues →"** button that switches
to the Repair tab with the file carried over. Available in all 13 UI languages.

## Notes

- After repairing, run `epublift check` on the output: epubveri may still report
  findings that no automatic fix can settle, such as missing accessibility
  metadata or a broken link whose intended target only the author knows.
- Every entry the fixes do not touch is copied through unchanged.
