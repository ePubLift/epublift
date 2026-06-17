<!--
DRAFT for a W3C epub-specs *Discussion* (category: Ideas):
  https://github.com/w3c/epub-specs/discussions

Intent (per the project owner): share findings and start a conversation —
NOT to advocate, push, or get a particular outcome adopted. Lead with data and
open questions; concede the known blockers up front; explicitly target future
thinking (3.5+), not the frozen 3.4. Tone: curious, humble, professional.

Post this only after a final human review. The personal-library numbers are
from our own run; the public-domain (Project Gutenberg) numbers are reproducible
with the command included below.
-->

# Has Zstandard (ZIP method 93) ever been explored for the OCF container? — some measurements, and open questions

Hello — and apologies in advance if this has been discussed before and I missed it (pointers very welcome).

I help maintain a small open-source tool that modernizes EPUB 2 files to EPUB 3 and re-encodes their images. While working on it I got curious about the **OCF ZIP container** itself: it mandates *Stored* + *Deflate*, while ZIP has long registered **Zstandard as compression method 93** (PKWARE APPNOTE). I couldn't find prior discussion of Zstd for OCF, so rather than argue for anything, I ran some measurements and would love to hear how the WG and implementers think about it.

**To be clear about scope:** I understand EPUB 3.4 is effectively frozen, and I'm explicitly *not* proposing a change to it. I'm also fully aware of the elephant in the room — the installed base. A second compression method is worthless to a publisher until essentially every reading system supports it, and "the publishing industry is a large tanker." So please read this as *"here's some data, is this interesting to think about for the long term?"*, not as a proposal.

## What I measured

A corpus of **170 real EPUBs** (a personal library; mixed genres and sizes). For each book I re-packed its **already-uncompressed entries** two ways — Deflate (today's conformant packaging) and Zstandard (level 19) — and compared sizes. Because already-compressed **images and fonts dominate most EPUBs and don't benefit from any re-compression**, the whole-archive difference is small (~3%). So the more meaningful number isolates the **text/markup** (XHTML, CSS, OPF, NCX, SVG…), which is what Deflate vs Zstd actually acts on:

### Text-only (images & fonts excluded), bucketed by raw text size — personal library of 170 mixed EPUBs

| Book text size | Books | Zstd, per-entry | Zstd, shared-dictionary* |
|---|---:|---:|---:|
| small (< 200 KB text) | 16 | −4.0% | −7.4% |
| medium (200 KB – 1 MB) | 113 | −6.1% | −7.0% |
| large (> 1 MB text) | 41 | −6.5% | −12.6% |
| **all** | 170 | **−6.3%** | **−9.9%** |

### The same, on a small **public-domain** sample anyone can reproduce

So this isn't just my private files, here are the same measurements on 16 Project Gutenberg titles (spanning a flash-fiction-length piece up to the complete works of Shakespeare). These are predominantly text, so they also show where the shared-dictionary win really lives — large, many-chapter works:

| Book text size | Books | Zstd, per-entry | Zstd, shared-dictionary |
|---|---:|---:|---:|
| small (< 200 KB text) | 3 | −4.1% | −4.1% |
| medium (200 KB – 1 MB) | 6 | −4.9% | −4.9% |
| large (> 1 MB text) | 7 | −4.2% | −15.4% |
| **all** | 16 | **−4.3%** | **−13.5%** |

The small/medium Gutenberg books are mostly a *single* content file, so the shared dictionary has no cross-file redundancy to exploit and correctly falls back to per-entry; the large multi-chapter works (War and Peace, Don Quixote, the Shakespeare collection…) are where it pays off (**−15%**). To replicate (Gutenberg IDs `11 35 84 100 174 345 996 1260 1342 1400 1661 1952 2554 2600 2701 5200`):

```sh
for id in 11 35 84 100 174 345 996 1260 1342 1400 1661 1952 2554 2600 2701 5200; do
  curl -L -o "pg$id.epub" "https://www.gutenberg.org/cache/epub/$id/pg$id.epub"; sleep 2
done
# then, with the open-source tool's research build:
zstd-bench --text-only --levels 19 .
```

\* *Two flavours, because they ask different things of a future spec:*

- **Per-entry** — each file compressed independently, exactly like Deflate today. This is the "standards-plausible, no new container machinery" floor: ~**4–6.5%** smaller text across all book sizes.
- **Shared dictionary** — a dictionary trained from the book's *own* text entries and shared across chapters, capturing the cross-file redundancy Deflate's per-file 32 KB window structurally can't see. This is where the bigger win lives (**up to ~−15% on large, multi-chapter books**), but it needs a place to store the dictionary, which ZIP/OCF has no standard slot for (more on that below). In my tool I store it as a single extra entry and only keep it when it actually wins, so it's never worse than per-entry.

A couple of secondary observations that might matter to implementers:

- **Decode speed.** Zstd typically decompresses several times faster than zlib, which could be a battery/CPU benefit on low-power e-ink readers.
- **A memory-safe reference is now feasible.** I did this with a **pure-Rust** Zstd encoder/decoder (no C/`libzstd`), and on this corpus its **compression ratio is within ~0.5% of reference C `libzstd`** — so "needs the C library" is no longer a hard prerequisite for experimenting.

## Where this honestly lands

- The win is **real but modest on whole books** (~3%), and concentrated in **text-heavy, image-light titles** — e.g. reference works, textbooks, and the kind of **multi-chapter EPUB 2 backlogs** that get converted in bulk. For an image-heavy novel it's nearly nothing.
- None of this changes the adoption reality: until reading systems implement method 93, this can't be used in shipped books. Deflate would obviously have to remain the mandatory baseline.

So I'm not claiming this clears the bar — only that the numbers exist now, and they seem worth a conversation.

## Open questions for the group

1. **Prior art** — has Zstd (or any second compression method) for OCF been raised or studied before? If it was considered and set aside, I'd genuinely like to read the reasoning.
2. **Where would a shared dictionary live?** This is the interesting technical question. ZIP has no standard slot for a per-archive dictionary. As a concrete, testable straw-man I've been storing it as `META-INF/zstd-dict.bin` — but I'd love to hear whether that's sane or there's a better idea.
3. **Is there any appetite** — for the long term, not 3.4 — for an *optional* second method with Deflate kept as the mandatory baseline? Or is the per-reading-system implementation cost simply too high for a single-digit text gain to ever be worth it?

I'm happy to share the measurement methodology and the (open-source, reproducible) tooling, and to run additional numbers if a particular cut would be useful — e.g. a public-domain corpus (Standard Ebooks / Gutenberg) so anyone can replicate, or specific book profiles you'd find more representative.

Thanks for reading, and again — this is meant as "here's what I found, what do you all think?", not a push for any outcome.
