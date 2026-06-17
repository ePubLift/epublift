# Design — Experimental Zstandard OCF packaging (research track)

**Status:** Draft / not started. Begins **after v1.3 (Kobo `.kepub`) ships.**
**Owner:** Baris Kayadelen

This work is described on **two independent axes** — do not collapse them into one
word ("experimental"), because they graduate on different conditions:

| Axis | States | What moves it forward |
|---|---|---|
| **measurement-maturity** | `preliminary` → `validated` | *Our* test data + a solid, tested implementation. Within our control. |
| **conformance** | `non-conformant` → `conformant` | A future EPUB spec registering Zstd (method 93) **and** reading systems implementing it. **Not** in our control; **not** affected by how good our numbers are. |

Today we are `preliminary` **and** `non-conformant`. Good measurements move us to
`validated`; they do **not** touch `non-conformant`. A Zstd `.epub` will **not**
open in today's reading systems regardless of how strong our data is — that
warning stays until the conformance axis actually moves. This work exists to
*measure* the potential of Zstandard over Deflate for EPUB packaging, and to back
a W3C `epub-specs` **Ideas** discussion aimed at a future spec (EPUB 3.5 horizon
— 3.4 is frozen at Candidate Recommendation).

---

## 1. Goals & non-goals

**Goals**
1. Produce **credible, reproducible measurements** of Zstd vs Deflate over a
   corpus of real EPUBs — reported two ways:
   - *per-entry* (each file compressed independently — standards-plausible,
     conservative floor),
   - *shared-dictionary* (all text entries compressed against one trained
     dictionary — the real "big win" ceiling; storage is non-standard).
2. Let users **see it with their own eyes**: emit an experimental
   `<name>_zstd-experimental.epub` and report bytes saved vs the conformant
   `_v3.3` output *and* vs the original EPUB 2 input.
3. **Lossless & reversible**: a decoder reconstructs a byte-faithful conformant
   EPUB from the experimental file — this is what proves "no data loss" and earns
   credibility in the discussion.
4. **Benchmark pure-Rust *and* C zstd** (ratio + speed). The wider ecosystem
   (publishers, other tools) will keep using the C `libzstd`, so we report both
   the pure-Rust *floor* and the C *ceiling*, plus the throughput gap.

**Non-goals**
- Conformance. We say "won't open in current readers" loudly, everywhere.
- Changing the default pipeline. Deflate `_v3.3` stays the shipping default;
  Zstd is strictly opt-in and feature-gated.
- A new reading system. Verification is round-trip, not rendering.

---

## 2. Why Zstd, mechanically (the one-paragraph rationale)

OCF restricts the container to *Stored* + *Deflate*. ZIP already registers
Zstandard as **compression method 93** (PKWARE APPNOTE), so a `.epub` *can*
technically carry zstd entries. Deflate uses a **32 KB window that resets per
entry**, so it is blind to redundancy *across* files — and an EPUB is exactly
dozens of small, near-identical XHTML files (shared boilerplate, repeated markup,
one CSS vocabulary) that never even fill that window. Zstd brings a larger window
and, crucially, **trained dictionaries**, which is precisely the cross-chapter
case Deflate can't see. Secondary win: zstd **decompresses several× faster** than
zlib — a battery/CPU benefit on low-power readers.

---

## 3. Where it plugs in

Single integration point today: **`repackage_epub()` — `src/lib.rs:353`**, which
writes `stored` mimetype first, then `deflated` everything else.

Generalise to a packaging strategy:

```rust
enum Packaging {
    Deflate,                                  // default, conformant (_v3.3)
    Zstd { mode: ZstdMode, level: i32 },      // experimental
}
enum ZstdMode { PerEntry, SharedDict }
```

- Add `packaging: Packaging` to the core `Options` struct — same "slot a new
  field in cleanly" pattern the roadmap already used for `target_version`.
- CLI: `--zstd` (+ `--zstd-level`, `--zstd-mode per-entry|shared-dict`). The
  conformant path is untouched when the flag is absent.
- Web (`epublift-web`): **off by default.** Optionally a clearly-labelled
  "Experimental" toggle later — decide after we have numbers.
- Output name: **`<name>_zstd-experimental.epub`** — deliberately *not* `_v3.x`,
  which would read like a spec-version claim and could irritate the WG mid-CR.
  Conformant outputs keep `_v3.3`. **The `-experimental` in the name is tied to
  the *conformance* axis, not the *maturity* axis** — it stays even after our
  measurements are `validated`, and is only dropped when the output actually
  becomes conformant (spec + reader support). Reaching `validated` maturity does
  *not* rename the file.

---

## 4. The ZIP-writing problem (the real subtlety)

The `zip` crate (v8, our current dep) writes Deflate/Stored fine, and has a
`CompressionMethod::Zstd` **behind its `zstd` feature — which pulls the C
`zstd` crate**. So:

- **C path (benchmark/ceiling only):** use the `zip` crate's built-in Zstd
  method. Easiest way to emit a valid method-93 archive. C dependency — **never
  shipped**, only compiled under a bench feature.
- **Pure-Rust path (shipped experimental output):** the `zip` crate has *no*
  pure-Rust zstd backend, and won't let us hand it pre-compressed bytes as method
  93. So we compress entries ourselves with **`structured-zstd`/`ruzstd`** and
  write the archive with a **small self-contained ZIP writer** (local file
  headers + central directory + CRC32 + method id 93). This is acceptable because
  the shared-dictionary mode needs a **non-standard archive layout anyway** (see
  §5) — we're off the conformant path regardless, so owning the writer is the
  clean choice.
  - New module: **`src/zstd_ocf.rs`** (writer + reader).
  - CRC32 of *uncompressed* data via **`crc32fast`** (pure Rust).
  - Validate the writer against our existing `zip::ZipArchive` reader and
    external `zipdetails`/`unzip -l` during development.

---

## 5. Shared-dictionary design (the "big win" — explicitly non-standard)

- **Train** a dictionary from the book's own text entries (XHTML + CSS) using
  `structured-zstd`'s pure-Rust dictionary support; optionally compare against a
  static "EPUB boilerplate" dictionary.
- **Storage:** ZIP has no standard slot for a shared dictionary, so we store it
  as a dedicated archive entry — proposal: **`META-INF/zstd-dict.bin`** — that an
  experimental reader loads before decompressing text entries. *This is our
  concrete answer to the W3C open question "where would the dictionary live?"* —
  we get to test a real proposal instead of hand-waving.
- Produces the two measurement modes from §1 (per-entry vs shared-dict).

---

## 6. Decoder / round-trip verification

- `epublift --zstd-decode in_zstd-experimental.epub` → reconstruct a conformant
  Deflate EPUB. Pure-Rust decode via `ruzstd`/`structured-zstd` + the dictionary
  entry.
- **Round-trip test (the credibility anchor):** conformant EPUB → encode
  zstd-experimental → decode back → assert the **file tree is byte-identical**
  (per-entry CRC match). Lives alongside `tests/convert.rs`.

---

## 7. Benchmark harness — Rust **and** C (the comparison we want)

A **dev-only** binary, `src/bin/zstd_bench.rs` (excluded from release artifacts),
runs over a corpus directory and prints a per-book + aggregate table:

| column | meaning |
|---|---|
| original | input EPUB 2 size |
| deflate `_v3.3` | our conformant output size |
| zstd per-entry | bytes, % vs deflate, % vs original |
| zstd shared-dict | bytes, % vs deflate, % vs original |
| encode / decode time | per backend |
| throughput (MB/s) | per backend |

- **Two backends, side by side:** pure-Rust (`structured-zstd`/`ruzstd`) **and**
  C (`zstd` crate / `zip`'s zstd feature). Report ratio *and* speed for both, so
  we can state honestly: "a young pure-Rust encoder already saves X%; reference
  C libzstd saves Y% and is N× faster." Rationale: publishers and other tools
  will use the C library, so both numbers matter to the audience.
- The C backend sits behind a **`zstd-c-bench` cargo feature** (and dev-only
  dep), so **Guiding Principle #1 — "Pure Rust, no C" — holds for every shipped
  artifact**; C appears only in our local measurements.
- **Level sweep:** a few zstd levels (e.g. 3 / 9 / 19) to find the knee.
- **Corpus:** reuse `tests/` fixtures + curated public-domain books
  (Gutenberg / Standard Ebooks), deliberately mixing **image-light and
  image-heavy** titles — images are already compressed, so zstd barely helps
  there; the win is text-/markup-heavy, many-chapter books. We report this
  split honestly rather than cherry-picking.

---

## 8. Dependencies

| crate | role | purity |
|---|---|---|
| `structured-zstd` (or `ruzstd`) | pure-Rust zstd encode/decode + dictionary | ✅ pure Rust, no FFI |
| `crc32fast` | ZIP entry CRC32 (self-contained writer) | ✅ pure Rust |
| `zstd` (C) | **bench-only** reference backend | ❌ C — gated behind `zstd-c-bench`, never shipped |

Verify crate maturity/API at implementation time — pure-Rust zstd encoders are
young and may trail C `libzstd` on ratio/speed/configurability (that gap is
exactly what the §7 comparison surfaces).

---

## 9. Honest framing (output, report, web copy, W3C post)

State the two axes separately on every surface; never let "validated" numbers
imply the output is usable.

- **Conformance (permanent until the spec + readers move):** "Not a conformant
  EPUB — will **not** open in current reading systems. Provided to measure
  potential." This line stays no matter how good the data gets.
- **Maturity (moves with our data):** `preliminary` while numbers are speculative
  → `validated` once we have real, reproducible corpus measurements and a tested
  implementation. Only this word changes; "non-conformant" does not.

Dropping the `preliminary`/`validated` distinction is *not* a green light to drop
the non-conformance warning — and claiming "no longer experimental" while the
file still won't open would **damage credibility** with the WG, so we don't.

The eventual W3C `Ideas` post reports per-entry (conservative) and shared-dict
(ceiling) numbers *separately*, includes pure-Rust-vs-C figures, concedes the
installed-base blocker up front, and is framed for **EPUB 3.5 / future**,
explicitly not touching frozen 3.4.

---

## 10. Risks & open questions

- **Pure-Rust encoder maturity** (ratio/speed) could undersell zstd → mitigated
  by also reporting C numbers.
- **Self-contained ZIP writer correctness** → mitigated by byte-equality
  round-trip tests + cross-checking against `ZipArchive` and `zipdetails`.
- **Images dominate many EPUBs** and won't shrink → set expectations; the win is
  text-heavy books. Report the split.
- **Dictionary storage is non-standard** → that's the point (a concrete proposal
  to test), but it must be labelled non-conformant.

---

## 11. Sequencing

1. **v1.3 Kobo `.kepub` ships first** (committed milestone; real user value,
   testable on a real Kobo). This work does **not** start until then.
2. **Phase 1:** per-entry encode + round-trip decode + benchmark harness
   (pure-Rust **and** C).
3. **Phase 2:** shared-dictionary mode + `META-INF/zstd-dict.bin` layout.
4. **Phase 3:** open the W3C `Ideas` discussion and drop real numbers in while
   attention is fresh.
</content>
</invoke>
