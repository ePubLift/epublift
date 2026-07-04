# Validate an EPUB (`check`)

This document specifies ePubLift's validation feature: checking an EPUB against
the EPUB specification and reporting problems with epubcheck-compatible message
IDs. It's powered by [**epubveri**](https://github.com/ePubLift/epubveri), our
own pure-Rust, JVM-free alternative to W3C's `epubcheck`.

Status: **shipped** — CLI `check` subcommand, behind the opt-in `validate` build
feature (included in the release binaries).

## Principles

1. **Pure-Rust, no JVM.** `epubcheck` is the reference validator, but it's a
   ~30 MB Java application. `epublift check` embeds epubveri instead: a small,
   fast, pure-Rust engine — no Java, no C, no external process. It reuses
   `roxmltree` (already a dependency) plus a small CSS parser.
2. **Dogfooding.** epublift depends on the **published** `epubveri` crate from
   crates.io, exactly like any other user would — not a local path. If we don't
   consume our own published crate, why would anyone else?
3. **Honest about maturity.** epubveri is **pre-1.0**. On epubcheck's own test
   corpus it reaches **~98.8% exact message-ID recall** with roughly **1% false
   positives** — very good, but **not yet full epubcheck parity**. Treat a clean
   `check` as strong evidence, not a conformance certificate. For an official
   conformance claim, cross-check with `epubcheck` itself.

## Usage

```sh
epublift check book.epub
```

Validate several at once (e.g. a whole shelf):

```sh
epublift check *.epub
```

### Options

| Flag | Meaning |
| --- | --- |
| `--profile <name>` | Validate against an EPUB extension profile: `dict` (dictionaries), `edupub`, `idx` (indexes) or `preview`. Default: base EPUB 3. |
| `--json` | Emit machine-readable JSON instead of the human-readable report. |
| `-q`, `--quiet` | Only print files that have problems; stay silent on clean passes. Handy in scripts/CI over a large library. |

### Exit codes

`check` follows the grep convention: the command itself ran fine, but the exit
code reflects what it found.

| Exit code | Meaning |
| --- | --- |
| `0` | Every input is valid (zero ERROR-severity messages). |
| `1` | At least one input has ERROR-severity problems, or could not be read. |

This makes it a drop-in gate in a build script or CI:

```sh
epublift check dist/*.epub || { echo "validation failed"; exit 1; }
```

## Output

Human-readable (default):

```
FAIL  book.epub  (2 errors, 1 warning)
    ERROR   RSC-005  content.opf  spine references manifest item id 'id43' more than once
    ERROR   RSC-007  text/ch1.html  reference to a resource missing from the publication: '…'
    WARNING OPF-053  content.opf  dc:date value '…' does not follow recommended syntax
```

Each line is `SEVERITY  ID  [location]  message`, where the ID is
epubcheck-compatible (e.g. `RSC-005`, `OPF-053`, `PKG-016`) so existing
epubcheck knowledge and tooling carry over.

JSON (`--json`) — one object per input file:

```json
[
  {
    "file": "book.epub",
    "valid": false,
    "errors": 2,
    "warnings": 1,
    "messages": [
      { "id": "RSC-005", "severity": "ERROR", "text": "…", "location": "content.opf" }
    ]
  }
]
```

A file that could not be read is reported as `{"valid": false, "error": "…"}`.

## In the web app (`epublift-web`)

The hosted web UI has a **Validate** mode that runs the *same* epubveri engine
**entirely client-side, as WebAssembly** — the file is **never uploaded**. The
`wasm-pack --target web` build is vendored under `epublift-web/static/vendor/`
and served same-origin (`/vendor/epubveri.js` + `/vendor/epubveri_bg.wasm`); the
SPA lazy-loads it the first time you open the Validate tab, then validates in the
browser and renders the report inline. The server never sees the book.

Two CSP notes make this work under the site's strict `default-src 'none'` policy:
`script-src` includes `'wasm-unsafe-eval'` (to compile the WASM — this allows
WebAssembly compilation only, not arbitrary `eval`), and the glue/wasm are
same-origin so `'self'` covers them. The UI is honest about maturity: a **beta**
badge, a "never uploaded" privacy line, and a link to epubcheck for an
authoritative result. Available in all 13 UI languages.

## Notes

- **Filename-based checks.** A few checks depend on the file on disk, not just
  its bytes — notably `PKG-016` (the `.epub` extension should be lowercase).
  These only fire when you pass a real path to `check` (which epublift always
  does), not when the library validates raw bytes.
- **Severity and validity.** A book is "valid" (`is_valid`) when it has **zero
  ERROR-severity** messages. WARNING and INFO messages don't fail the check, but
  they're still worth reading — many point at real portability problems.
