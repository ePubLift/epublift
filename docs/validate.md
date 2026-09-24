# Validate an EPUB (`check`)

This document specifies ePubLift's validation feature: checking an EPUB against
the EPUB specification and reporting problems with epubcheck-compatible message
IDs. It's powered by [**epubveri**](https://github.com/veripublica/epubveri), our
own pure-Rust, JVM-free alternative to W3C's `epubcheck`.

Status: **shipped** — CLI `check` subcommand, behind the opt-in `validate` build
feature (included in the release binaries).

## Principles

1. **Pure-Rust, no JVM.** `epubcheck` is the reference validator, but it's a
   ~30 MB Java application. `epublift check` embeds epubveri instead: a small,
   fast, pure-Rust engine — no Java, no C, no external process. It shares the
   `zip` and `roxmltree` versions epublift already uses rather than pulling
   second copies of them, plus a small CSS parser.
2. **Dogfooding.** epublift depends on the **published** `epubveri` crate from
   crates.io, exactly like any other user would — not a local path. If we don't
   consume our own published crate, why would anyone else?
3. **Honest about maturity.** epubveri is **pre-1.0**. Against epubcheck 5.4.0's
   own test suite (measured by epubveri on 0.17.4) it catches **99.7%** of the
   cases that should be flagged with epubcheck's exact message ID (685 of 687)
   and raises **no false alarm** on any of the 368 valid ones — very good, but
   **not yet full epubcheck parity**, and it departs from epubcheck on purpose
   in a few documented places (see epubveri's
   [coverage notes](https://github.com/veripublica/epubveri/blob/main/docs/COVERAGE.md)). Treat a clean
   `check` as strong evidence, not a conformance certificate. For an official
   conformance claim, cross-check with `epubcheck` itself.
4. **Machine output is a shared contract.** `--json` emits the
   [veripublica machine envelope](https://github.com/veripublica/conventions/blob/main/FORMATS.md),
   the same shape epubveri and epubsana emit — so one report can be consumed
   without bespoke per-tool parsing. epublift is not a veripublica tool (it's a
   separate project with its own CLI surface), but its validation output speaks
   that family's format, because the findings come from that family's engine.

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
| `--json` | Emit the machine-readable [envelope](#json) instead of the human-readable report. |
| `-q`, `--quiet` | Only print files that have problems; stay silent on clean passes. Handy in scripts/CI over a large library. |

### Exit codes

`check` follows the grep convention: the command itself ran fine, but the exit
code reflects what it found. **A verdict is not a failure** — a book so broken
that validation stops dead is still a book epubveri graded, and it exits `1`.
Exit `2` is reserved for inputs it could not report on *at all*.

| Exit code | Meaning |
| --- | --- |
| `0` | Every input was validated and is valid (no error- or fatal-severity findings). |
| `1` | Every input was validated; at least one is invalid. |
| `2` | At least one input could **not** be processed — a missing file, an unreadable one, an I/O failure. The other inputs are still validated and reported. |

The `1`/`2` split is what lets a script tell *"this book is bad"* apart from
*"I couldn't read that file"* — a typo in a path is not a validation result. Both
are non-zero, so an ordinary gate still works unchanged:

```sh
epublift check dist/*.epub || { echo "validation failed"; exit 1; }
```

Multiple inputs are **all** processed even when an earlier one fails, so one
broken path never hides the reports for the rest of a shelf.

## Output

Human-readable (default):

```
FAIL  book.epub  (3 errors, 1 warning)
    ERROR   OPF-034  content.opf:103:5  spine references manifest item id 'id43' more than once
    ERROR   RSC-007  text/ch1.html:13:24  reference to a resource missing from the publication: '…'
    WARNING HTM-025  text/ch1.html:13:24  URL '…' uses an unregistered scheme
```

Each line is `SEVERITY  ID  [location]  message`, where the ID is
epubcheck-compatible (e.g. `RSC-005`, `OPF-053`, `PKG-016`) so existing
epubcheck knowledge and tooling carry over. When the engine can pin the exact
spot, the location reads like a compiler diagnostic — `file:line:column` (e.g.
`content.opf:103:5`) — so a producer can jump straight to it; checks that have
no single line to point at (whole-container/ZIP-structure problems) show just
the file, or nothing.

The summary line counts errors and warnings. **Fatals are counted separately**
and only named when there are any, because a fatal stops processing and would
otherwise leave a failure reading `(0 errors, 0 warnings)`:

```
FAIL  broken.epub  (1 fatal, 0 errors, 0 warnings)
    FATAL   PKG-008  could not open zip file: invalid Zip archive: Could not find EOCD
```

<a id="json"></a>

### JSON

`--json` emits **one** [veripublica envelope](https://github.com/veripublica/conventions/blob/main/FORMATS.md)
object: the tool that produced it, the convention it conforms to, an aggregate
`status` mirroring the exit code, and one self-contained object per input in
command-line order.

```json
{
  "tool": "epublift",
  "tool_version": "2.0.0",
  "convention": "0.6",
  "status": "problems",
  "inputs": [
    {
      "path": "book.epub",
      "status": "problems",
      "summary": { "fatal": 0, "error": 3, "warning": 1, "info": 0, "usage": 37 },
      "items": [
        {
          "type": "finding",
          "code": "OPF-034",
          "rule": "opf.spine.duplicate_itemref",
          "severity": "error",
          "location": "content.opf",
          "position": { "line": 103, "column": 5 },
          "message": "spine references manifest item id 'id43' more than once",
          "data": {
            "params": ["id43"],
            "element_path": "/opf:package[1]/opf:spine[1]/opf:itemref[2]",
            "namespaces": { "opf": "http://www.idpf.org/2007/opf" }
          }
        }
      ]
    }
  ]
}
```

Worth knowing when consuming it:

- **`status`** is `"ok"` / `"problems"` / `"error"`, mirroring exit `0` / `1` /
  `2`. An input with `status: "error"` carries an `error` string saying why it
  could not be read, and no verdict — it is never a grade.
- **`code`** is the epubcheck-compatible ID; **`rule`** is epubveri's own finer,
  stable sub-code (e.g. `opf.spine.duplicate_itemref`), which distinguishes the
  many unrelated violations one shared ID like `RSC-005` can mean. `rule` is
  being rolled out incrementally, so it is absent on checks not yet retrofitted.
- **`data.element_path`** is an XPath-style path to the offending node, resolvable
  with the prefixes in `data.namespaces` — so a fixer can jump to the node
  instead of re-deriving it from a line and column.
- **`summary`** always carries all five counters, zeros included, so a missing
  key never stands in for "none". `usage` findings (epubcheck's `-u` level) are
  counted and listed like the others; `epublift check` filters nothing, so the
  envelope never carries a `suppressed` marker.
- **`convention`** is epublift's own claim about this output: the envelope meets
  [FORMATS.md](https://github.com/veripublica/conventions/blob/main/FORMATS.md)
  of veripublica conventions 0.6. It is a claim about the JSON, not the command
  line — `check` takes positional paths and `--json` rather than the `-i` and
  `--format json` that conventions' CLI.md asks for.
- Fields that don't apply are **omitted**, and unknown fields **must be ignored**:
  the shape gains optional fields over time without breaking consumers.

> **Breaking change.** `--json` used to emit a bespoke array of
> `{file, valid, errors, warnings, messages}` objects. It now emits the shared
> envelope above. Migrating: the array is `inputs`; `file` is `path`; `valid:
> true` is `status == "ok"` (the old `valid: false` covered both a bad book and
> an unreadable file — the two are now `"problems"` and `"error"`);
> `messages[].id` is `items[].code`; `messages[].text` is `items[].message`; and
> `severity` values are now lowercase (`"error"`, not `"ERROR"`).

## In the web app (`epublift-web`)

The hosted web UI has a **Validate** mode that runs the epubveri engine
**entirely client-side, as WebAssembly** — the file is **never uploaded**. The
engine is epubveri's own published build, `@veripublica/epubveri-wasm` from npm
(built by epubveri's CI from its release tag, with a provenance attestation),
vendored byte-for-byte under `epublift-web/static/vendor/` beside a small loader
of ours; [`VENDOR.md`](../epublift-web/static/vendor/VENDOR.md) there records the
version, digests and update steps. It is served same-origin (`/vendor/epubveri.js`,
`/vendor/epubveri_bg.js`, `/vendor/epubveri_bg.wasm`); the SPA lazy-loads it the
first time you open the Validate tab, then validates in the browser and renders
the report inline. It is always the same epubveri version as the CLI's `check`:
on the 474-book test shelf the two report identical findings for every book. The server never sees the book. The
report table's location column shows the same `file:line:column` spot as the
CLI when the engine pinned one.

Two CSP notes make this work under the site's strict `default-src 'none'` policy:
`script-src` includes `'wasm-unsafe-eval'` (to compile the WASM — this allows
WebAssembly compilation only, not arbitrary `eval`), and the glue/wasm are
same-origin so `'self'` covers them. The UI is honest about maturity: a **beta**
badge, a "never uploaded" privacy line, and a link to epubcheck for an
authoritative result. Available in all 13 UI languages.

The WASM binding returns the **same `inputs[i]` object** the CLI's `--json`
emits per input (minus `path`/`error`, which a browser caller has neither of), so
both surfaces read one shape from one engine and cannot drift into reporting the
same book differently.

## Notes

- **Filename-based checks.** A few checks depend on the file on disk, not just
  its bytes — notably `PKG-016` (the `.epub` extension should be lowercase).
  These only fire when you pass a real path to `check` (which epublift always
  does), not when the library validates raw bytes.
- **Severity and validity.** Findings carry epubcheck's five severity levels:
  `fatal`, `error`, `warning`, `info`, `usage`. Only **`error` and `fatal`**
  cross the valid/invalid line — a book is valid when it has none. `warning`,
  `info` and `usage` findings are reported and never fail the check, but they're
  still worth reading: many point at real portability problems. A script that
  wants to fail on warnings is asking a different question than `check`'s
  default.
- **`fatal` vs. exit `2`.** They are not the same thing. `fatal` means epubveri
  could not keep going *and still named the defect* — a verdict, exit `1`. Exit
  `2` means there was nothing to grade at all (no such file, unreadable).
