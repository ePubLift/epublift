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
            "-h" | "--help" => {
                eprintln!(
                    "usage: zstd-bench [--levels 3,9,19] [--dict-sweep|--text-only] <dir-or-epub>...\n\
                     measures Deflate vs Zstandard (pure-Rust, and C under zstd-c-bench)\n\
                     as an EPUB packaging method.\n\
                     --dict-sweep  compares shared-dict heuristics (divisor / min-files).\n\
                     --text-only   reports the text-only win (images/fonts excluded),\n\
                                   bucketed small/medium/large by raw text size."
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
