// Dev-only benchmark for the experimental Zstandard-OCF research track.
//
// This binary is built ONLY under the `zstd-experimental` feature (see the
// `required-features` in Cargo.toml), so it never ships in a release artifact.
// It measures what Zstandard would save over Deflate as an EPUB *packaging*
// method, isolated from image conversion: it reads the already-uncompressed
// entries of real `.epub` files and re-packs them, comparing
//
//   * Deflate (our conformant shipping packaging), vs
//   * Zstandard per-entry — pure-Rust `structured-zstd` (the SHIPPED floor), vs
//   * Zstandard per-entry — reference C `libzstd` (the ceiling; ratio + speed).
//
// The C backend is itself behind the `zstd-c-bench` feature, so the default
// `zstd-experimental` build stays pure Rust (Guiding Principle #1). Run:
//
//   cargo run --bin zstd-bench --features zstd-experimental -- <dir-or-epubs...>
//   cargo run --bin zstd-bench --features zstd-c-bench       -- <dir-or-epubs...>
//
// Options: `--levels 3,9,19` selects the zstd level sweep (default `19`).
//
// Honest framing: per-entry is the conservative floor. The big win — a shared
// dictionary across a book's many near-identical XHTML chapters — is Phase 2 and
// is NOT measured here. Image-heavy books barely move (images are already
// compressed); the win is in text-/markup-heavy titles. We report both.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use epublift::zstd_ocf::{
    DictParams, OcfEntry, is_dict_eligible, pack_zstd, pack_zstd_best, pack_zstd_shared_dict,
    pack_zstd_shared_dict_with, unpack_zstd,
};
use structured_zstd::decoding::FrameDecoder;
use structured_zstd::encoding::{CompressionLevel, compress_to_vec};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// One backend's result on one payload at one level.
struct Measure {
    /// Total compressed bytes (sum of per-entry payloads).
    bytes: usize,
    /// Seconds spent encoding all entries.
    encode_s: f64,
    /// Seconds spent decoding all entries (round-trip verified).
    decode_s: f64,
}

/// Aggregate per-book figures.
struct BookResult {
    name: String,
    raw: usize,
    deflate_archive: usize,
    /// Per-entry Zstd archive size at the headline level.
    zstd_archive: usize,
    /// Shared-dictionary Zstd archive size at the headline level.
    zstd_shared_archive: usize,
    rust: Vec<(i32, Measure)>,
    #[cfg(feature = "zstd-c-bench")]
    c: Vec<(i32, Measure)>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut levels = vec![19];
    let mut inputs: Vec<String> = Vec::new();
    let mut dict_sweep = false;
    let mut text_only = false;
    let mut solid = false;
    let mut mem_probe: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--levels" => {
                let v = it.next().context("--levels needs a value, e.g. 3,9,19")?;
                levels = v
                    .split(',')
                    .map(|s| s.trim().parse::<i32>())
                    .collect::<Result<_, _>>()
                    .context("invalid --levels value")?;
            }
            "--dict-sweep" => dict_sweep = true,
            "--text-only" => text_only = true,
            "--solid" => solid = true,
            "--mem-probe" => {
                mem_probe = Some(
                    it.next()
                        .context("--mem-probe needs a codec name, e.g. 'brotli-11'")?
                        .clone(),
                );
            }
            "-h" | "--help" => {
                eprintln!(
                    "usage: zstd-bench [--levels 3,9,19] [--dict-sweep|--text-only|--solid] <dir-or-epub>...\n\
                     measures Deflate vs Zstandard (pure-Rust, and C under zstd-c-bench)\n\
                     as an EPUB packaging method.\n\
                     --dict-sweep  compares shared-dict heuristics (divisor / min-files).\n\
                     --text-only   reports the text-only win (images/fonts excluded),\n\
                                   bucketed small/medium/large by raw text size.\n\
                     --solid       ARCHIVAL track (.eparc): solid-stream zstd-ultra vs\n\
                                   brotli-11 (needs --features archival-bench)."
                );
                return Ok(());
            }
            _ => inputs.push(a.clone()),
        }
    }
    if inputs.is_empty() {
        bail!(
            "no input given. usage: zstd-bench [--levels 3,9,19] [--dict-sweep] <dir-or-epub>..."
        );
    }

    let epubs = collect_epubs(&inputs)?;
    if epubs.is_empty() {
        bail!("no .epub files found in the given paths");
    }
    if dict_sweep {
        return run_dict_sweep(&epubs, *levels.last().unwrap());
    }
    if text_only {
        return run_text_only(&epubs, *levels.last().unwrap());
    }
    if solid {
        #[cfg(feature = "archival-bench")]
        return run_solid(&epubs, &levels);
        #[cfg(not(feature = "archival-bench"))]
        bail!("--solid needs the Brotli backend: rebuild with --features archival-bench");
    }
    if let Some(codec) = mem_probe {
        #[cfg(feature = "archival-bench")]
        return run_mem_probe(&codec, &epubs[0], &levels);
        #[cfg(not(feature = "archival-bench"))]
        {
            let _ = codec;
            bail!(
                "--mem-probe needs the archival backends: rebuild with --features archival-bench"
            );
        }
    }
    #[cfg(feature = "zstd-c-bench")]
    eprintln!("[backends] pure-Rust structured-zstd + reference C libzstd");
    #[cfg(not(feature = "zstd-c-bench"))]
    eprintln!(
        "[backends] pure-Rust structured-zstd only \
         (build --features zstd-c-bench to add the C libzstd ceiling)"
    );
    eprintln!("[levels] {levels:?}\n");

    let mut results = Vec::new();
    for path in &epubs {
        match bench_one(path, &levels) {
            Ok(r) => {
                print_book(&r, &levels);
                results.push(r);
            }
            Err(e) => eprintln!("[skip] {}: {e:#}", path.display()),
        }
    }

    print_aggregate(&results, &levels);
    Ok(())
}

/// The question that actually matters for a "replace Deflate with Zstd" pitch:
/// what does Zstd save on the **text** of a book (XHTML/CSS/OPF/…), with images
/// and fonts EXCLUDED (they're already compressed and dilute the headline)?
/// Books are bucketed by raw uncompressed text size (small / medium / large) so
/// the answer is honest about where the win lives.
fn run_text_only(epubs: &[PathBuf], level: i32) -> Result<()> {
    struct Bucket {
        label: &'static str,
        // raw-text range [lo, hi) in bytes
        lo: usize,
        hi: usize,
        books: usize,
        raw: usize,
        deflate: usize,
        per_entry: usize,
        safe: usize,
    }
    let mut buckets = [
        Bucket {
            label: "small  (<200 KB text)",
            lo: 0,
            hi: 200 * 1024,
            books: 0,
            raw: 0,
            deflate: 0,
            per_entry: 0,
            safe: 0,
        },
        Bucket {
            label: "medium (200KB–1MB)   ",
            lo: 200 * 1024,
            hi: 1024 * 1024,
            books: 0,
            raw: 0,
            deflate: 0,
            per_entry: 0,
            safe: 0,
        },
        Bucket {
            label: "large  (>1 MB text)  ",
            lo: 1024 * 1024,
            hi: usize::MAX,
            books: 0,
            raw: 0,
            deflate: 0,
            per_entry: 0,
            safe: 0,
        },
    ];

    for path in epubs {
        let entries = match read_entries(path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[skip] {}: {e:#}", path.display());
                continue;
            }
        };
        // Keep only the compressible text entries — drop images, fonts, mimetype.
        let text: Vec<OcfEntry> = entries
            .into_iter()
            .filter(|e| is_dict_eligible(&e.name))
            .collect();
        if text.is_empty() {
            continue;
        }
        let raw: usize = text.iter().map(|e| e.data.len()).sum();
        let deflate = deflate_archive_size(&text)?;
        let per_entry = pack_zstd(&text, level)?.len();
        let safe = pack_zstd_best(&text, level)?.len();

        let b = buckets
            .iter_mut()
            .find(|b| raw >= b.lo && raw < b.hi)
            .unwrap();
        b.books += 1;
        b.raw += raw;
        b.deflate += deflate;
        b.per_entry += per_entry;
        b.safe += safe;
    }

    println!("══ TEXT-ONLY (images/fonts excluded, level {level}) ══");
    println!(
        "{:<24} {:>6} {:>12} {:>14} {:>16}",
        "bucket (by raw text)", "books", "deflate KB", "zstd/entry", "zstd/size-safe"
    );
    let mut tot = (0usize, 0usize, 0usize, 0usize); // books, deflate, per_entry, safe
    for b in &buckets {
        if b.books == 0 {
            continue;
        }
        println!(
            "{:<24} {:>6} {:>12.0} {:>12.1}% {:>14.1}%",
            b.label,
            b.books,
            kb(b.deflate),
            pct(b.per_entry, b.deflate) - 100.0,
            pct(b.safe, b.deflate) - 100.0,
        );
        tot.0 += b.books;
        tot.1 += b.deflate;
        tot.2 += b.per_entry;
        tot.3 += b.safe;
    }
    println!(
        "{:<24} {:>6} {:>12.0} {:>12.1}% {:>14.1}%",
        "ALL",
        tot.0,
        kb(tot.1),
        pct(tot.2, tot.1) - 100.0,
        pct(tot.3, tot.1) - 100.0,
    );
    println!("\n(zstd/entry = per-entry; zstd/size-safe = shared-dict kept only when it wins.)");
    Ok(())
}

/// encode(raw) -> compressed.
#[cfg(feature = "archival-bench")]
type EncFn = Box<dyn Fn(&[u8]) -> Vec<u8>>;
/// decode(compressed, expected_len) -> raw.
#[cfg(feature = "archival-bench")]
type DecFn = Box<dyn Fn(&[u8], usize) -> Vec<u8>>;

/// A solid-stream codec under test: a name + encode/decode closures. The
/// pure-Rust candidates (zstd-rust, brotli) are always present; the C reference
/// *ceilings* (zstd-C incl. `--ultra 22`, xz/LZMA) are added under `ceiling-bench`
/// so we can state — with numbers — exactly how much ratio the pure-Rust choice
/// leaves on the table, and (via `--mem-probe`) at what memory cost C buys it.
#[cfg(feature = "archival-bench")]
struct Codec {
    name: String,
    enc: EncFn,
    /// decode(compressed, expected_len) — `expected_len` pre-sizes the output
    /// (structured-zstd's `decode_all_to_vec` needs the capacity up front).
    dec: DecFn,
}

/// Running totals for one codec over the corpus, for one payload class.
#[cfg(feature = "archival-bench")]
#[derive(Clone, Default)]
struct Tally {
    bytes: usize,
    enc_s: f64,
    dec_s: f64,
}

/// Build the solid-codec line-up: pure-Rust always, C ceilings under `ceiling-bench`.
#[cfg(feature = "archival-bench")]
fn solid_codecs(levels: &[i32]) -> Vec<Codec> {
    let mut v: Vec<Codec> = Vec::new();
    for &lvl in levels {
        v.push(Codec {
            name: format!("zstd-rust L{lvl}"),
            enc: Box::new(move |d| compress_to_vec(d, CompressionLevel::Level(lvl))),
            dec: Box::new(|c, n| {
                let mut dec = FrameDecoder::new();
                let mut out = Vec::with_capacity(n);
                dec.decode_all_to_vec(c, &mut out)
                    .expect("zstd-rust decode");
                out
            }),
        });
    }
    v.push(Codec {
        name: "brotli-11".into(),
        enc: Box::new(|d| {
            let params = brotli::enc::BrotliEncoderParams {
                quality: 11,
                lgwin: 24,
                ..Default::default()
            };
            let mut out = Vec::new();
            brotli::BrotliCompress(&mut &d[..], &mut out, &params).expect("brotli encode");
            out
        }),
        dec: Box::new(|c, _n| {
            let mut out = Vec::new();
            brotli::BrotliDecompress(&mut &c[..], &mut out).expect("brotli decode");
            out
        }),
    });
    // --- C reference ceilings (dev-only; NEVER shipped) ---------------------
    #[cfg(feature = "ceiling-bench")]
    {
        for lvl in [19i32, 22] {
            v.push(Codec {
                name: format!("zstd-C L{lvl}"),
                enc: Box::new(move |d| zstd::encode_all(d, lvl).expect("zstd-C encode")),
                dec: Box::new(|c, _n| zstd::decode_all(c).expect("zstd-C decode")),
            });
        }
        v.push(Codec {
            name: "xz -9".into(),
            enc: Box::new(|d| {
                let mut e = xz2::write::XzEncoder::new(Vec::new(), 9);
                std::io::Write::write_all(&mut e, d).expect("xz encode");
                e.finish().expect("xz finish")
            }),
            dec: Box::new(|c, _n| {
                let mut out = Vec::new();
                xz2::read::XzDecoder::new(c)
                    .read_to_end(&mut out)
                    .expect("xz decode");
                out
            }),
        });
        v.push(Codec {
            name: "xz -9e".into(),
            enc: Box::new(|d| {
                // 9 | LZMA_PRESET_EXTREME (the extreme bit is 1 << 31, which xz2
                // does not re-export as a named constant).
                let stream =
                    xz2::stream::Stream::new_easy_encoder(9 | (1 << 31), xz2::stream::Check::Crc64)
                        .expect("xz -9e stream");
                let mut e = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
                std::io::Write::write_all(&mut e, d).expect("xz -9e encode");
                e.finish().expect("xz -9e finish")
            }),
            dec: Box::new(|c, _n| {
                let mut out = Vec::new();
                xz2::read::XzDecoder::new(c)
                    .read_to_end(&mut out)
                    .expect("xz -9e decode");
                out
            }),
        });
    }
    v
}

/// ARCHIVAL research track (`.eparc`). Unlike the OCF-experimental ZIP (per-entry,
/// random-access), an archive is opened whole on restore, so it can be a single
/// **solid** stream: all entries concatenated and compressed together. That lets
/// one window span every chapter — capturing cross-chapter redundancy with NO
/// stored dictionary (the Phase-2 shared-dict's overhead disappears). Baselines
/// are Deflate (today's packaging) and zstd per-entry (the OCF floor); the codec
/// line-up is built by `solid_codecs`. Reported on both the text payload and the
/// whole archive. Encode is one-time + slow-OK on a Pi; decode MB/s is shown to
/// prove restore stays fast regardless of compression effort.
#[cfg(feature = "archival-bench")]
fn run_solid(epubs: &[PathBuf], levels: &[i32]) -> Result<()> {
    let codecs = solid_codecs(levels);
    let mut text = vec![Tally::default(); codecs.len()];
    let mut whole = vec![Tally::default(); codecs.len()];
    let (mut tb, mut wb) = (0usize, 0usize);
    let (mut traw, mut wraw) = (0usize, 0usize);
    let (mut tdef, mut wdef) = (0usize, 0usize);
    let (mut tpe, mut wpe) = (0usize, 0usize);
    let last = *levels.last().unwrap();

    for path in epubs {
        let entries = match read_entries(path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[skip] {}: {e:#}", path.display());
                continue;
            }
        };
        // text-only = the compressible markup; whole = everything but mimetype.
        let text_entries: Vec<&OcfEntry> = entries
            .iter()
            .filter(|e| is_dict_eligible(&e.name))
            .collect();
        let whole_entries: Vec<&OcfEntry> =
            entries.iter().filter(|e| e.name != "mimetype").collect();

        if !text_entries.is_empty() {
            let buf = entries_text(&text_entries);
            tb += 1;
            traw += buf.len();
            run_codecs(&codecs, &buf, &mut text)?;
            let owned: Vec<OcfEntry> = text_entries.iter().map(|e| (*e).clone()).collect();
            tdef += deflate_archive_size(&owned)?;
            tpe += pack_zstd(&owned, last)?.len();
        }
        let wbuf = entries_text(&whole_entries);
        wb += 1;
        wraw += wbuf.len();
        run_codecs(&codecs, &wbuf, &mut whole)?;
        wdef += deflate_archive_size(&entries)?; // includes the stored mimetype
        wpe += pack_zstd(&entries, last)?.len();
    }

    print_solid(
        "TEXT-ONLY (images/fonts excluded)",
        tb,
        traw,
        tdef,
        tpe,
        &codecs,
        &text,
    );
    println!();
    print_solid(
        "WHOLE ARCHIVE (all entries)",
        wb,
        wraw,
        wdef,
        wpe,
        &codecs,
        &whole,
    );
    println!("\nSolid = all entries concatenated into ONE stream (length-prefixed).");
    println!("Pure-Rust (shippable): zstd-rust (structured-zstd), brotli-11 (q11/lgwin24).");
    #[cfg(feature = "ceiling-bench")]
    println!(
        "C ceilings (dev-only, never shipped): zstd-C (libzstd, incl. --ultra 22), xz (liblzma)."
    );
    println!("Encode is one-time (slow-OK on a Pi); decode MB/s stays high regardless of level.");
    Ok(())
}

/// Concatenate entries into one length-prefixed solid buffer (the archive payload).
#[cfg(feature = "archival-bench")]
fn entries_text(entries: &[&OcfEntry]) -> Vec<u8> {
    let total: usize = entries.iter().map(|e| 8 + e.data.len()).sum();
    let mut buf = Vec::with_capacity(total);
    for e in entries {
        buf.extend_from_slice(&(e.data.len() as u64).to_le_bytes());
        buf.extend_from_slice(&e.data);
    }
    buf
}

/// Run every codec on one solid buffer (round-trip verified) into the tallies.
#[cfg(feature = "archival-bench")]
fn run_codecs(codecs: &[Codec], buf: &[u8], tally: &mut [Tally]) -> Result<()> {
    for (i, c) in codecs.iter().enumerate() {
        let t = Instant::now();
        let comp = (c.enc)(buf);
        let enc_s = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let out = (c.dec)(&comp, buf.len());
        let dec_s = t.elapsed().as_secs_f64();
        if out != buf {
            bail!("solid round-trip mismatch for {}", c.name);
        }
        tally[i].bytes += comp.len();
        tally[i].enc_s += enc_s;
        tally[i].dec_s += dec_s;
    }
    Ok(())
}

/// Print one payload-class block: deflate / per-entry baselines + one row per codec.
#[cfg(feature = "archival-bench")]
#[allow(clippy::too_many_arguments)]
fn print_solid(
    title: &str,
    books: usize,
    raw: usize,
    deflate: usize,
    per_entry: usize,
    codecs: &[Codec],
    tally: &[Tally],
) {
    println!("══ SOLID — {title} ({books} books) ══");
    println!(
        "raw {:.0} KB | deflate {:.0} KB | zstd per-entry {:+.1}% vs deflate",
        kb(raw),
        kb(deflate),
        pct(per_entry, deflate) - 100.0,
    );
    println!(
        "{:<16} {:>12} {:>14} {:>12} {:>12}",
        "method", "size KB", "vs deflate", "enc MB/s", "dec MB/s"
    );
    for (c, t) in codecs.iter().zip(tally) {
        println!(
            "{:<16} {:>12.0} {:>13.1}% {:>12.2} {:>12.0}",
            c.name,
            kb(t.bytes),
            pct(t.bytes, deflate) - 100.0,
            mb_per_s(raw, t.enc_s),
            mb_per_s(raw, t.dec_s),
        );
    }
}

/// Single-codec ENCODE of one book's whole-archive solid buffer, so an external
/// `/usr/bin/time -l` (macOS) / `-v` (GNU) wrapper can capture this process's peak
/// RSS for that one codec. Encode is the memory-hungry phase and the real Pi
/// constraint: it decides whether the device can even *make* the archive. Run:
///   for c in "brotli-11" "zstd-rust L19" "zstd-C L22" "xz -9e"; do \
///     /usr/bin/time -l zstd-bench --mem-probe "$c" big.epub; done
#[cfg(feature = "archival-bench")]
fn run_mem_probe(codec: &str, book: &Path, levels: &[i32]) -> Result<()> {
    let entries = read_entries(book)?;
    let whole: Vec<&OcfEntry> = entries.iter().filter(|e| e.name != "mimetype").collect();
    let buf = entries_text(&whole);
    let codecs = solid_codecs(levels);
    let c = codecs.iter().find(|c| c.name == codec).ok_or_else(|| {
        let names: Vec<&str> = codecs.iter().map(|c| c.name.as_str()).collect();
        anyhow::anyhow!("unknown codec '{codec}'. available: {}", names.join(", "))
    })?;
    let comp = (c.enc)(&buf);
    // Print (and thus keep) the result so the encode can't be optimised away.
    println!(
        "{codec}: {} KB -> {} KB ({:+.1}% of raw)",
        buf.len() / 1024,
        comp.len() / 1024,
        pct(comp.len(), buf.len()) - 100.0,
    );
    Ok(())
}

/// Compare shared-dictionary heuristics against the per-entry baseline at one
/// level, over the corpus. For each config we report the aggregate archive size
/// (vs deflate and vs per-entry) and how many books it beats / loses to
/// per-entry — plus a `keep-smaller` row that takes min(per-entry, shared) per
/// book (the size-safe option that can never lose).
fn run_dict_sweep(epubs: &[PathBuf], level: i32) -> Result<()> {
    struct Cfg {
        label: &'static str,
        params: DictParams,
    }
    let base = DictParams::default();
    let cfgs = [
        Cfg {
            label: "div4  min2 ",
            params: DictParams {
                size_divisor: 4,
                ..base
            },
        },
        Cfg {
            label: "div8  min2 ",
            params: DictParams {
                size_divisor: 8,
                ..base
            },
        },
        Cfg {
            label: "div16 min2 ",
            params: DictParams {
                size_divisor: 16,
                ..base
            },
        },
        Cfg {
            label: "div32 min2 ",
            params: DictParams {
                size_divisor: 32,
                ..base
            },
        },
        Cfg {
            label: "div8  min4 ",
            params: DictParams {
                size_divisor: 8,
                min_files: 4,
                ..base
            },
        },
        Cfg {
            label: "div8  min8 ",
            params: DictParams {
                size_divisor: 8,
                min_files: 8,
                ..base
            },
        },
    ];

    // Per book: deflate, per-entry, and each config's shared size.
    let mut deflate_tot = 0usize;
    let mut per_entry_tot = 0usize;
    let mut cfg_tot = vec![0usize; cfgs.len()];
    let mut cfg_win = vec![0usize; cfgs.len()];
    let mut cfg_loss = vec![0usize; cfgs.len()];
    // keep-smaller using the best (smallest) shared per book across configs.
    let mut keepsmaller_tot = 0usize;
    let mut books = 0usize;

    for path in epubs {
        let entries = match read_entries(path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[skip] {}: {e:#}", path.display());
                continue;
            }
        };
        let deflate = deflate_archive_size(&entries)?;
        let per_entry = pack_zstd(&entries, level)?.len();
        deflate_tot += deflate;
        per_entry_tot += per_entry;

        let mut best_shared = per_entry; // keep-smaller starts at per-entry
        for (i, cfg) in cfgs.iter().enumerate() {
            let s = pack_zstd_shared_dict_with(&entries, level, cfg.params)?.len();
            cfg_tot[i] += s;
            if s < per_entry {
                cfg_win[i] += 1;
            } else if s > per_entry {
                cfg_loss[i] += 1;
            }
            best_shared = best_shared.min(s);
        }
        keepsmaller_tot += best_shared;
        books += 1;
    }
    if books == 0 {
        bail!("no books measured");
    }

    println!("══ DICT SWEEP ({books} books, level {level}) ══");
    println!(
        "baseline: deflate {:.0} KB | per-entry {:.0} KB ({:+.1}% vs deflate)",
        kb(deflate_tot),
        kb(per_entry_tot),
        pct(per_entry_tot, deflate_tot) - 100.0,
    );
    println!(
        "{:<12} {:>12} {:>14} {:>14} {:>8} {:>8}",
        "config", "shared KB", "vs deflate", "vs per-entry", "won", "lost"
    );
    for (i, cfg) in cfgs.iter().enumerate() {
        println!(
            "{:<12} {:>12.0} {:>13.1}% {:>13.1}% {:>8} {:>8}",
            cfg.label,
            kb(cfg_tot[i]),
            pct(cfg_tot[i], deflate_tot) - 100.0,
            pct(cfg_tot[i], per_entry_tot) - 100.0,
            cfg_win[i],
            cfg_loss[i],
        );
    }
    println!(
        "{:<12} {:>12.0} {:>13.1}% {:>13.1}%   (size-safe: min(per-entry, shared) per book)",
        "keep-smaller",
        kb(keepsmaller_tot),
        pct(keepsmaller_tot, deflate_tot) - 100.0,
        pct(keepsmaller_tot, per_entry_tot) - 100.0,
    );
    Ok(())
}

/// Read every entry of an EPUB into memory, *uncompressed*. Skips the `mimetype`
/// from the compression comparison (it is always stored), but keeps a record so
/// archive sizes include it identically across backends.
fn read_entries(path: &Path) -> Result<Vec<OcfEntry>> {
    let file = std::fs::File::open(path)?;
    let mut zip = ZipArchive::new(file)?;
    let mut entries = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        if !f.is_file() {
            continue;
        }
        let name = f.name().to_string();
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data)?;
        entries.push(OcfEntry { name, data });
    }
    Ok(entries)
}

/// Build a conformant Deflate archive (mimetype stored first) and return its
/// total size — exactly the packaging we ship today.
fn deflate_archive_size(entries: &[OcfEntry]) -> Result<usize> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for e in entries {
        let opts = if e.name == "mimetype" {
            stored
        } else {
            deflated
        };
        zip.start_file(&e.name, opts)?;
        std::io::Write::write_all(&mut zip, &e.data)?;
    }
    Ok(zip.finish()?.into_inner().len())
}

fn bench_one(path: &Path, levels: &[i32]) -> Result<BookResult> {
    let entries = read_entries(path)?;
    let raw: usize = entries.iter().map(|e| e.data.len()).sum();

    // Compress everything except the (always-stored) mimetype.
    let payloads: Vec<&[u8]> = entries
        .iter()
        .filter(|e| e.name != "mimetype")
        .map(|e| e.data.as_slice())
        .collect();

    let deflate_archive = deflate_archive_size(&entries)?;
    // Archive-level zstd sizes use the headline level (last in the sweep).
    let headline = *levels.last().unwrap();
    let per_entry_archive = pack_zstd(&entries, headline)?;
    let zstd_archive = per_entry_archive.len();
    let shared_archive = pack_zstd_shared_dict(&entries, headline)?;
    let zstd_shared_archive = shared_archive.len();
    // Verify BOTH archives round-trip (decode == original entries).
    if unpack_zstd(&per_entry_archive)? != entries || unpack_zstd(&shared_archive)? != entries {
        bail!("archive round-trip mismatch — refusing to report numbers");
    }

    let mut rust = Vec::new();
    #[cfg(feature = "zstd-c-bench")]
    let mut c = Vec::new();
    for &lvl in levels {
        rust.push((lvl, measure_rust(&payloads, lvl)?));
        #[cfg(feature = "zstd-c-bench")]
        c.push((lvl, measure_c(&payloads, lvl)?));
    }

    Ok(BookResult {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        raw,
        deflate_archive,
        zstd_archive,
        zstd_shared_archive,
        rust,
        #[cfg(feature = "zstd-c-bench")]
        c,
    })
}

/// Pure-Rust per-entry encode + decode (round-trip verified).
fn measure_rust(payloads: &[&[u8]], level: i32) -> Result<Measure> {
    let t = Instant::now();
    let compressed: Vec<Vec<u8>> = payloads
        .iter()
        .map(|p| compress_to_vec(*p, CompressionLevel::Level(level)))
        .collect();
    let encode_s = t.elapsed().as_secs_f64();
    let bytes: usize = compressed.iter().map(|c| c.len()).sum();

    let t = Instant::now();
    for (orig, comp) in payloads.iter().zip(&compressed) {
        let mut dec = FrameDecoder::new();
        let mut out = Vec::with_capacity(orig.len());
        dec.decode_all_to_vec(comp, &mut out)
            .map_err(|e| anyhow::anyhow!("rust decode failed: {e:?}"))?;
        if out != *orig {
            bail!("rust round-trip mismatch");
        }
    }
    let decode_s = t.elapsed().as_secs_f64();
    Ok(Measure {
        bytes,
        encode_s,
        decode_s,
    })
}

/// Reference C libzstd per-entry encode + decode (round-trip verified).
#[cfg(feature = "zstd-c-bench")]
fn measure_c(payloads: &[&[u8]], level: i32) -> Result<Measure> {
    let t = Instant::now();
    let compressed: Vec<Vec<u8>> = payloads
        .iter()
        .map(|p| zstd::encode_all(*p, level))
        .collect::<Result<_, _>>()?;
    let encode_s = t.elapsed().as_secs_f64();
    let bytes: usize = compressed.iter().map(|c| c.len()).sum();

    let t = Instant::now();
    for (orig, comp) in payloads.iter().zip(&compressed) {
        let out = zstd::decode_all(comp.as_slice())?;
        if out != *orig {
            bail!("C round-trip mismatch");
        }
    }
    let decode_s = t.elapsed().as_secs_f64();
    Ok(Measure {
        bytes,
        encode_s,
        decode_s,
    })
}

fn collect_epubs(inputs: &[String]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for inp in inputs {
        let p = PathBuf::from(inp);
        if p.is_dir() {
            for e in walkdir_epubs(&p) {
                out.push(e);
            }
        } else if p.extension().map(|x| x == "epub").unwrap_or(false) {
            out.push(p);
        } else {
            eprintln!("[skip] not an .epub or directory: {}", p.display());
        }
    }
    out.sort();
    Ok(out)
}

fn walkdir_epubs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir_epubs(&p));
            } else if p.extension().map(|x| x == "epub").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}

// ---- reporting ------------------------------------------------------------

fn pct(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64) * 100.0
    }
}

fn mb_per_s(bytes: usize, secs: f64) -> f64 {
    if secs <= 0.0 {
        0.0
    } else {
        (bytes as f64 / (1024.0 * 1024.0)) / secs
    }
}

fn kb(n: usize) -> f64 {
    n as f64 / 1024.0
}

fn print_book(r: &BookResult, _levels: &[i32]) {
    println!("📖 {}", r.name);
    let headline = r.rust.last().map(|(l, _)| *l).unwrap_or(0);
    println!(
        "   raw {:>9.1} KB | deflate {:>9.1} KB ({:>5.1}% of raw)",
        kb(r.raw),
        kb(r.deflate_archive),
        pct(r.deflate_archive, r.raw),
    );
    let safe = r.zstd_archive.min(r.zstd_shared_archive);
    println!(
        "   archive(L{}) per-entry {:>9.1} KB ({:+5.1}% vs deflate) | shared-dict {:>9.1} KB ({:+5.1}% vs deflate, {:+5.1}% vs per-entry) | size-safe {:>9.1} KB ({:+5.1}% vs deflate)",
        headline,
        kb(r.zstd_archive),
        pct(r.zstd_archive, r.deflate_archive) - 100.0,
        kb(r.zstd_shared_archive),
        pct(r.zstd_shared_archive, r.deflate_archive) - 100.0,
        pct(r.zstd_shared_archive, r.zstd_archive) - 100.0,
        kb(safe),
        pct(safe, r.deflate_archive) - 100.0,
    );
    for (lvl, m) in &r.rust {
        println!(
            "   zstd L{:<2} rust  payload {:>9.1} KB ({:+5.1}% vs deflate) | enc {:>6.1} MB/s | dec {:>6.1} MB/s",
            lvl,
            kb(m.bytes),
            pct(m.bytes, r.deflate_archive) - 100.0,
            mb_per_s(r.raw, m.encode_s),
            mb_per_s(r.raw, m.decode_s),
        );
    }
    #[cfg(feature = "zstd-c-bench")]
    for (lvl, m) in &r.c {
        println!(
            "   zstd L{:<2} C     payload {:>9.1} KB ({:+5.1}% vs deflate) | enc {:>6.1} MB/s | dec {:>6.1} MB/s",
            lvl,
            kb(m.bytes),
            pct(m.bytes, r.deflate_archive) - 100.0,
            mb_per_s(r.raw, m.encode_s),
            mb_per_s(r.raw, m.decode_s),
        );
    }
    println!();
}

fn print_aggregate(results: &[BookResult], levels: &[i32]) {
    if results.is_empty() {
        return;
    }
    let raw: usize = results.iter().map(|r| r.raw).sum();
    let deflate: usize = results.iter().map(|r| r.deflate_archive).sum();
    let zstd_arch: usize = results.iter().map(|r| r.zstd_archive).sum();
    let shared_arch: usize = results.iter().map(|r| r.zstd_shared_archive).sum();

    println!(
        "══════════════════════════ AGGREGATE ({} books) ══════════════════════════",
        results.len()
    );
    println!("raw {:.1} KB | deflate {:.1} KB", kb(raw), kb(deflate));
    println!(
        "archive per-entry   {:.1} KB → {:+.1}% vs deflate",
        kb(zstd_arch),
        pct(zstd_arch, deflate) - 100.0,
    );
    println!(
        "archive shared-dict {:.1} KB → {:+.1}% vs deflate ({:+.1}% vs per-entry)",
        kb(shared_arch),
        pct(shared_arch, deflate) - 100.0,
        pct(shared_arch, zstd_arch) - 100.0,
    );
    // Size-safe = min(per-entry, shared-dict) per book — never worse than
    // per-entry, captures the dictionary wins where they exist.
    let safe_arch: usize = results
        .iter()
        .map(|r| r.zstd_archive.min(r.zstd_shared_archive))
        .sum();
    println!(
        "archive size-safe   {:.1} KB → {:+.1}% vs deflate ({:+.1}% vs per-entry)",
        kb(safe_arch),
        pct(safe_arch, deflate) - 100.0,
        pct(safe_arch, zstd_arch) - 100.0,
    );
    for (i, &lvl) in levels.iter().enumerate() {
        let rb: usize = results.iter().map(|r| r.rust[i].1.bytes).sum();
        println!(
            "  L{lvl:<2} rust payload total {:.1} KB ({:+.1}% vs deflate)",
            kb(rb),
            pct(rb, deflate) - 100.0
        );
        #[cfg(feature = "zstd-c-bench")]
        {
            let cb: usize = results.iter().map(|r| r.c[i].1.bytes).sum();
            let gap = if cb == 0 {
                0.0
            } else {
                (rb as f64 / cb as f64 - 1.0) * 100.0
            };
            println!(
                "  L{lvl:<2} C    payload total {:.1} KB ({:+.1}% vs deflate) | rust is {:+.1}% larger than C",
                kb(cb),
                pct(cb, deflate) - 100.0,
                gap,
            );
        }
    }
    println!("\nNote: per-entry is the conservative floor. The shared-dictionary win (Phase 2)");
    println!("is not measured here. Image-heavy books barely move; text-heavy books gain most.");
}
