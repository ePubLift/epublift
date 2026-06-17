// epublift - Experimental Zstandard-OCF packaging (research track).
//
// This module is part of the experimental measurement track described in
// docs/design/zstd-ocf-experimental.md. It is compiled only under the
// `zstd-experimental` feature and produces a DELIBERATELY NON-CONFORMANT
// container: a ZIP that carries its non-mimetype entries with compression
// **method 93 (Zstandard)**. Today's reading systems only implement Stored +
// Deflate, so a `_zstd-experimental.epub` will NOT open in them. The point is
// to *measure* what Zstd would save over Deflate for EPUB packaging, and to
// back a future W3C `epub-specs` discussion with real numbers.
//
// Why a hand-rolled ZIP writer? The `zip` crate (our normal dependency) only
// emits method 93 via its `zstd` feature, which is the **C** libzstd — that
// would put C into a shipped artifact and break Guiding Principle #1. So we
// compress entries ourselves with the pure-Rust `structured-zstd` encoder and
// lay out the archive here. The layout is intentionally minimal (no zip64, no
// data descriptors): every entry's CRC32 and sizes are known up front because
// we hold the whole payload in memory, so each local file header is written
// with final values. Books are far below the 4 GiB / 65k-entry ZIP limits.
//
// Two packing modes (see [`Packaging`](crate::Packaging)):
//   * per-entry — each entry compressed independently (the conservative floor);
//   * shared-dictionary — a dictionary is trained from the book's own text
//     entries (XHTML/CSS/…) and stored as `META-INF/zstd-dict.bin`; text entries
//     are then compressed against it. This is the cross-chapter "big win", and
//     it is explicitly non-standard (ZIP has no slot for a shared dictionary —
//     storing it as a named entry is our concrete proposal for the W3C thread).

use anyhow::{Result, bail};
use structured_zstd::decoding::FrameDecoder;
use structured_zstd::dictionary::{
    FastCoverOptions, FinalizeOptions, create_fastcover_dict_from_source,
};
use structured_zstd::encoding::{CompressionLevel, FrameCompressor, compress_to_vec};

/// One archive member: a name (ZIP path) and its *uncompressed* bytes.
///
/// Working in terms of uncompressed entries (rather than files on disk) keeps
/// the packer/unpacker pure and unit-testable, and makes the round-trip
/// property — "decode reproduces the exact input bytes" — trivial to assert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcfEntry {
    pub name: String,
    pub data: Vec<u8>,
}

/// ZIP path of the trained shared dictionary in shared-dict archives. Its
/// presence is the signal that an archive is shared-dict (vs per-entry); the
/// decoder loads it before decompressing the dictionary-eligible text entries.
pub const DICT_ENTRY: &str = "META-INF/zstd-dict.bin";

/// ZIP compression method ids (PKWARE APPNOTE 4.4.5).
const METHOD_STORED: u16 = 0;
const METHOD_ZSTD: u16 = 93;

// ZIP record signatures.
const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;

// A minimal valid MS-DOS timestamp: 1980-01-01 00:00:00. We deliberately do not
// embed real mtimes — this is research output and a fixed stamp keeps packing
// deterministic (helpful for reproducible measurements).
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021;

/// Below this much training text a shared dictionary is not worth training —
/// fall back to per-entry packing.
const MIN_DICT_CORPUS: usize = 4 * 1024;

/// Tunable knobs for shared-dictionary training. Exposed so the benchmark can
/// sweep them; [`DictParams::default`] holds the shipping heuristic.
#[derive(Debug, Clone, Copy)]
pub struct DictParams {
    /// Train a dictionary of about `corpus_len / size_divisor` bytes.
    pub size_divisor: usize,
    /// Cap the trained dictionary at this many bytes.
    pub size_cap: usize,
    /// Require at least this many dictionary-eligible text entries; below it a
    /// dictionary can only be pure overhead (no cross-file redundancy to share),
    /// so fall back to per-entry.
    pub min_files: usize,
}

impl Default for DictParams {
    fn default() -> Self {
        Self {
            size_divisor: 8,
            size_cap: 110 * 1024,
            min_files: 2,
        }
    }
}

/// True for entries whose contents are text and benefit from a shared
/// dictionary (XHTML/CSS/OPF/…). Classification is purely by extension so the
/// encoder and decoder agree without storing any side manifest.
pub fn is_dict_eligible(name: &str) -> bool {
    if name == "mimetype" || name == DICT_ENTRY {
        return false;
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "xhtml" | "html" | "htm" | "css" | "opf" | "ncx" | "xml" | "svg" | "txt" | "json"
    )
}

/// A fully prepared entry ready to be written: payload is already compressed
/// (or stored), with its CRC32 and uncompressed length computed.
struct Prepared {
    name: String,
    method: u16,
    crc: u32,
    uncomp_size: u32,
    payload: Vec<u8>,
}

impl Prepared {
    fn stored(name: &str, data: &[u8]) -> Result<Self> {
        Ok(Self {
            name: name.to_string(),
            method: METHOD_STORED,
            crc: crc32fast::hash(data),
            uncomp_size: u32_len(data.len(), name)?,
            payload: data.to_vec(),
        })
    }

    fn zstd(name: &str, data: &[u8], payload: Vec<u8>) -> Result<Self> {
        Ok(Self {
            name: name.to_string(),
            method: METHOD_ZSTD,
            crc: crc32fast::hash(data),
            uncomp_size: u32_len(data.len(), name)?,
            payload,
        })
    }
}

/// Pack `entries` into an experimental Zstd-OCF archive, **per-entry**: the
/// `mimetype` is stored uncompressed (as OCF requires) and every other entry is
/// compressed independently with Zstandard (method 93) at `level` (C-zstd
/// numbering, 1–22).
pub fn pack_zstd(entries: &[OcfEntry], level: i32) -> Result<Vec<u8>> {
    let mut prepared = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.name == "mimetype" {
            prepared.push(Prepared::stored(&entry.name, &entry.data)?);
        } else {
            let payload = compress_to_vec(entry.data.as_slice(), CompressionLevel::Level(level));
            prepared.push(Prepared::zstd(&entry.name, &entry.data, payload)?);
        }
    }
    write_archive(&prepared)
}

/// Pack `entries` into an experimental Zstd-OCF archive using a **shared
/// dictionary** trained from the book's own text entries. Text entries are
/// compressed against the dictionary (the cross-chapter win); binary entries
/// (images/fonts) are compressed plainly; the dictionary is stored as
/// [`DICT_ENTRY`]. Falls back to [`pack_zstd`] when there is too little text to
/// train on, so callers always get a valid archive.
pub fn pack_zstd_shared_dict(entries: &[OcfEntry], level: i32) -> Result<Vec<u8>> {
    pack_zstd_shared_dict_with(entries, level, DictParams::default())
}

/// As [`pack_zstd_shared_dict`], but with explicit [`DictParams`] (used by the
/// benchmark to sweep the dictionary heuristic).
pub fn pack_zstd_shared_dict_with(
    entries: &[OcfEntry],
    level: i32,
    params: DictParams,
) -> Result<Vec<u8>> {
    let corpus: Vec<u8> = entries
        .iter()
        .filter(|e| is_dict_eligible(&e.name))
        .flat_map(|e| e.data.iter().copied())
        .collect();
    let eligible = entries.iter().filter(|e| is_dict_eligible(&e.name)).count();
    if corpus.len() < MIN_DICT_CORPUS || eligible < params.min_files {
        return pack_zstd(entries, level);
    }

    // Train a dictionary sized relative to the corpus, capped to a sane range.
    let dict_size = (corpus.len() / params.size_divisor).clamp(MIN_DICT_CORPUS, params.size_cap);
    let mut dict = Vec::new();
    create_fastcover_dict_from_source(
        corpus.as_slice(),
        &mut dict,
        dict_size,
        &FastCoverOptions::default(),
        FinalizeOptions::default(),
    )
    .map_err(|e| anyhow::anyhow!("dictionary training failed: {e}"))?;

    // One dictionary-primed compressor, reused across all text entries.
    let mut dict_c: FrameCompressor = FrameCompressor::new(CompressionLevel::Level(level));
    dict_c
        .set_dictionary_from_bytes(&dict)
        .map_err(|e| anyhow::anyhow!("attaching dictionary failed: {e:?}"))?;

    let mut prepared = Vec::with_capacity(entries.len() + 1);
    let mut dict_written = false;
    for entry in entries {
        if entry.name == "mimetype" {
            prepared.push(Prepared::stored(&entry.name, &entry.data)?);
            // Place the dictionary right after the mimetype.
            if !dict_written {
                prepared.push(Prepared::stored(DICT_ENTRY, &dict)?);
                dict_written = true;
            }
        } else if is_dict_eligible(&entry.name) {
            let mut payload = Vec::new();
            dict_c.compress_independent_frame_into(&entry.data, &mut payload);
            prepared.push(Prepared::zstd(&entry.name, &entry.data, payload)?);
        } else {
            let payload = compress_to_vec(entry.data.as_slice(), CompressionLevel::Level(level));
            prepared.push(Prepared::zstd(&entry.name, &entry.data, payload)?);
        }
    }
    // No mimetype entry present (unusual): still emit the dictionary first.
    if !dict_written {
        prepared.insert(0, Prepared::stored(DICT_ENTRY, &dict)?);
    }
    write_archive(&prepared)
}

/// Size-safe shared-dictionary packing: produce **both** the per-entry and the
/// shared-dictionary archive and return whichever is smaller. This mirrors the
/// project's existing "never grow a book" image principle — the trained
/// dictionary is kept only when it actually pays for its stored bytes (it wins
/// on large multi-chapter text books, loses on small/single-file/image-heavy
/// ones), so the result is **never worse than per-entry**.
///
/// (The naive [`pack_zstd_shared_dict`] — which always keeps the dictionary — is
/// retained for honest measurement of the dictionary's raw cost/benefit.)
pub fn pack_zstd_best(entries: &[OcfEntry], level: i32) -> Result<Vec<u8>> {
    let per_entry = pack_zstd(entries, level)?;
    let shared = pack_zstd_shared_dict(entries, level)?;
    Ok(if shared.len() < per_entry.len() {
        shared
    } else {
        per_entry
    })
}

/// Lay prepared entries out as a complete ZIP: local file headers + payloads,
/// then the central directory, then the end-of-central-directory record.
fn write_archive(prepared: &[Prepared]) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut offsets: Vec<u32> = Vec::with_capacity(prepared.len());

    for e in prepared {
        if e.name.len() > u16::MAX as usize {
            bail!("entry name too long for ZIP: {}", e.name);
        }
        offsets.push(u32_len(buf.len(), "archive offset")?);
        let comp_size = u32_len(e.payload.len(), &e.name)?;

        push_u32(&mut buf, SIG_LOCAL);
        push_u16(&mut buf, version_needed(e.method));
        push_u16(&mut buf, 0); // general purpose bit flag
        push_u16(&mut buf, e.method);
        push_u16(&mut buf, DOS_TIME);
        push_u16(&mut buf, DOS_DATE);
        push_u32(&mut buf, e.crc);
        push_u32(&mut buf, comp_size);
        push_u32(&mut buf, e.uncomp_size);
        push_u16(&mut buf, e.name.len() as u16);
        push_u16(&mut buf, 0); // extra field length
        buf.extend_from_slice(e.name.as_bytes());
        buf.extend_from_slice(&e.payload);
    }

    let cd_offset = u32_len(buf.len(), "central directory offset")?;
    for (e, &offset) in prepared.iter().zip(&offsets) {
        let comp_size = u32_len(e.payload.len(), &e.name)?;
        push_u32(&mut buf, SIG_CENTRAL);
        push_u16(&mut buf, 20); // version made by (2.0, MS-DOS)
        push_u16(&mut buf, version_needed(e.method));
        push_u16(&mut buf, 0); // general purpose bit flag
        push_u16(&mut buf, e.method);
        push_u16(&mut buf, DOS_TIME);
        push_u16(&mut buf, DOS_DATE);
        push_u32(&mut buf, e.crc);
        push_u32(&mut buf, comp_size);
        push_u32(&mut buf, e.uncomp_size);
        push_u16(&mut buf, e.name.len() as u16);
        push_u16(&mut buf, 0); // extra field length
        push_u16(&mut buf, 0); // file comment length
        push_u16(&mut buf, 0); // disk number start
        push_u16(&mut buf, 0); // internal file attributes
        push_u32(&mut buf, 0); // external file attributes
        push_u32(&mut buf, offset);
        buf.extend_from_slice(e.name.as_bytes());
    }
    let cd_size = u32_len(buf.len() - cd_offset as usize, "central directory size")?;

    let count = prepared.len() as u16;
    push_u32(&mut buf, SIG_EOCD);
    push_u16(&mut buf, 0); // number of this disk
    push_u16(&mut buf, 0); // disk where central directory starts
    push_u16(&mut buf, count); // CD records on this disk
    push_u16(&mut buf, count); // total CD records
    push_u32(&mut buf, cd_size);
    push_u32(&mut buf, cd_offset);
    push_u16(&mut buf, 0); // .ZIP file comment length

    Ok(buf)
}

/// A raw, still-compressed entry located from the central directory.
struct RawRecord {
    name: String,
    method: u16,
    crc: u32,
    uncomp_size: usize,
    payload_range: (usize, usize),
}

/// Parse the central directory, returning each entry's metadata and the byte
/// range of its (still-compressed) payload.
fn parse_records(archive: &[u8]) -> Result<Vec<RawRecord>> {
    let eocd = find_eocd(archive)?;
    let cd_count = read_u16(archive, eocd + 10)? as usize;
    let mut cd_pos = read_u32(archive, eocd + 16)? as usize;

    let mut records = Vec::with_capacity(cd_count);
    for _ in 0..cd_count {
        if read_u32(archive, cd_pos)? != SIG_CENTRAL {
            bail!("corrupt archive: bad central directory signature");
        }
        let method = read_u16(archive, cd_pos + 10)?;
        let crc = read_u32(archive, cd_pos + 16)?;
        let comp_size = read_u32(archive, cd_pos + 20)? as usize;
        let uncomp_size = read_u32(archive, cd_pos + 24)? as usize;
        let name_len = read_u16(archive, cd_pos + 28)? as usize;
        let extra_len = read_u16(archive, cd_pos + 30)? as usize;
        let comment_len = read_u16(archive, cd_pos + 32)? as usize;
        let local_offset = read_u32(archive, cd_pos + 42)? as usize;
        let name = read_str(archive, cd_pos + 46, name_len)?;

        // The local header's own name/extra lengths can differ; re-read them.
        if read_u32(archive, local_offset)? != SIG_LOCAL {
            bail!("corrupt archive: bad local header for {name}");
        }
        let lh_name_len = read_u16(archive, local_offset + 26)? as usize;
        let lh_extra_len = read_u16(archive, local_offset + 28)? as usize;
        let data_start = local_offset + 30 + lh_name_len + lh_extra_len;
        // Bounds-check the payload range now so later slicing is safe.
        slice(archive, data_start, comp_size)?;

        records.push(RawRecord {
            name,
            method,
            crc,
            uncomp_size,
            payload_range: (data_start, data_start + comp_size),
        });
        cd_pos += 46 + name_len + extra_len + comment_len;
    }
    Ok(records)
}

/// Parse a Zstd-OCF archive back into its uncompressed entries, in central
/// directory order. Handles **both** per-entry and shared-dictionary archives:
/// if a [`DICT_ENTRY`] is present, it is loaded first and applied to the
/// dictionary-eligible text entries (the same `is_dict_eligible` rule used when
/// packing). The dictionary entry itself is not returned. Each entry's CRC32 is
/// verified against its decompressed bytes — the integrity check that makes the
/// round-trip credible.
pub fn unpack_zstd(archive: &[u8]) -> Result<Vec<OcfEntry>> {
    let records = parse_records(archive)?;

    // First pass: load the shared dictionary, if this is a shared-dict archive.
    let dict: Option<Vec<u8>> = records
        .iter()
        .find(|r| r.name == DICT_ENTRY)
        .map(|r| archive[r.payload_range.0..r.payload_range.1].to_vec());

    // Second pass: decode every entry except the dictionary itself.
    let mut entries = Vec::with_capacity(records.len());
    for r in &records {
        if r.name == DICT_ENTRY {
            continue;
        }
        let payload = &archive[r.payload_range.0..r.payload_range.1];
        let data = match r.method {
            METHOD_STORED => payload.to_vec(),
            METHOD_ZSTD => {
                let mut dec = FrameDecoder::new();
                let mut out = vec![0u8; r.uncomp_size];
                let written = match (&dict, is_dict_eligible(&r.name)) {
                    (Some(d), true) => dec
                        .decode_all_with_dict_bytes(payload, &mut out, d)
                        .map_err(|e| {
                            anyhow::anyhow!("zstd dict-decode failed for {}: {e:?}", r.name)
                        })?,
                    _ => dec
                        .decode_all(payload, &mut out)
                        .map_err(|e| anyhow::anyhow!("zstd decode failed for {}: {e:?}", r.name))?,
                };
                out.truncate(written);
                out
            }
            other => bail!("unsupported compression method {other} for {}", r.name),
        };

        let actual = crc32fast::hash(&data);
        if actual != r.crc {
            bail!(
                "CRC mismatch for {}: expected {:#010x}, got {actual:#010x}",
                r.name,
                r.crc
            );
        }
        entries.push(OcfEntry {
            name: r.name.clone(),
            data,
        });
    }
    Ok(entries)
}

/// ZIP "version needed to extract": 2.0 covers Stored; method 93 (Zstd) is a
/// 6.3.x feature, so we advertise 6.3 for those entries.
fn version_needed(method: u16) -> u16 {
    if method == METHOD_ZSTD { 63 } else { 20 }
}

/// Scan backwards for the End Of Central Directory record. We never write a
/// trailing comment, so the EOCD sits at the very end, but scanning tolerates
/// one anyway.
fn find_eocd(archive: &[u8]) -> Result<usize> {
    if archive.len() < 22 {
        bail!("archive too small to be a ZIP");
    }
    let max_back = archive.len().saturating_sub(22);
    let floor = max_back.saturating_sub(u16::MAX as usize);
    for pos in (floor..=max_back).rev() {
        if read_u32(archive, pos)? == SIG_EOCD {
            return Ok(pos);
        }
    }
    bail!("not a ZIP archive: no end-of-central-directory record found")
}

// ---- little-endian helpers ------------------------------------------------

fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn read_u16(buf: &[u8], at: usize) -> Result<u16> {
    let b = slice(buf, at, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32(buf: &[u8], at: usize) -> Result<u32> {
    let b = slice(buf, at, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_str(buf: &[u8], at: usize, len: usize) -> Result<String> {
    let b = slice(buf, at, len)?;
    String::from_utf8(b.to_vec()).map_err(|_| anyhow::anyhow!("non-UTF-8 entry name in archive"))
}

fn slice(buf: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    buf.get(at..at + len)
        .ok_or_else(|| anyhow::anyhow!("archive truncated: need {len} bytes at offset {at}"))
}

/// Convert a `usize` length into the `u32` a 32-bit ZIP field requires, failing
/// loudly (rather than truncating) if a book somehow exceeds 4 GiB.
fn u32_len(len: usize, what: &str) -> Result<u32> {
    u32::try_from(len).map_err(|_| anyhow::anyhow!("{what} exceeds 4 GiB ZIP limit ({len} bytes)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<OcfEntry> {
        vec![
            OcfEntry {
                name: "mimetype".into(),
                data: b"application/epub+zip".to_vec(),
            },
            OcfEntry {
                name: "META-INF/container.xml".into(),
                data: b"<?xml version=\"1.0\"?><container/>".to_vec(),
            },
            OcfEntry {
                // Repetitive text so zstd actually compresses it.
                name: "OEBPS/chapter1.xhtml".into(),
                data: "<p>the quick brown fox</p>\n".repeat(500).into_bytes(),
            },
        ]
    }

    /// Several near-identical chapters: the case a shared dictionary should win.
    fn many_chapters() -> Vec<OcfEntry> {
        let mut entries = vec![OcfEntry {
            name: "mimetype".into(),
            data: b"application/epub+zip".to_vec(),
        }];
        for i in 0..12 {
            let body = format!(
                "<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\">\
                 <head><title>Chapter {i}</title><link rel=\"stylesheet\" href=\"s.css\"/></head>\
                 <body><h1>Chapter {i}</h1>{}</body></html>",
                "<p>The same boilerplate sentence repeats across chapters.</p>".repeat(40)
            );
            entries.push(OcfEntry {
                name: format!("OEBPS/chapter{i}.xhtml"),
                data: body.into_bytes(),
            });
        }
        entries.push(OcfEntry {
            name: "OEBPS/s.css".into(),
            data: b"body{margin:1em;font-family:serif} h1{font-weight:bold}".to_vec(),
        });
        entries
    }

    #[test]
    fn round_trips_entries_byte_for_byte() {
        let entries = sample_entries();
        let archive = pack_zstd(&entries, 3).unwrap();
        let back = unpack_zstd(&archive).unwrap();
        assert_eq!(entries, back);
    }

    #[test]
    fn shared_dict_round_trips_byte_for_byte() {
        let entries = many_chapters();
        let archive = pack_zstd_shared_dict(&entries, 19).unwrap();
        let back = unpack_zstd(&archive).unwrap();
        assert_eq!(entries, back, "shared-dict archive must decode losslessly");
    }

    #[test]
    fn shared_dict_stores_the_dictionary_entry() {
        let entries = many_chapters();
        let archive = pack_zstd_shared_dict(&entries, 19).unwrap();
        let names: Vec<String> = parse_records(&archive)
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert!(names.iter().any(|n| n == DICT_ENTRY), "dict entry present");
        // ...but it is not surfaced as book content.
        let back = unpack_zstd(&archive).unwrap();
        assert!(!back.iter().any(|e| e.name == DICT_ENTRY));
    }

    // Note: whether shared-dict actually *beats* per-entry is data-dependent
    // (the dictionary must be stored, so the cross-file savings have to outweigh
    // it — true for real books, not necessarily for tiny synthetic fixtures).
    // That size comparison is measured honestly by `zstd-bench` on a real
    // corpus, not asserted here where it would be brittle.

    #[test]
    fn pack_best_is_never_larger_than_per_entry() {
        for entries in [sample_entries(), many_chapters()] {
            let per_entry = pack_zstd(&entries, 19).unwrap().len();
            let best = pack_zstd_best(&entries, 19).unwrap();
            assert!(
                best.len() <= per_entry,
                "size-safe pack ({}) must not exceed per-entry ({per_entry})",
                best.len()
            );
            // ...and it still round-trips losslessly whichever mode it picked.
            assert_eq!(unpack_zstd(&best).unwrap(), entries);
        }
    }

    #[test]
    fn shared_dict_falls_back_when_too_little_text() {
        // Only the mimetype + one tiny xml: not enough to train on.
        let entries = vec![
            OcfEntry {
                name: "mimetype".into(),
                data: b"application/epub+zip".to_vec(),
            },
            OcfEntry {
                name: "META-INF/container.xml".into(),
                data: b"<container/>".to_vec(),
            },
        ];
        let archive = pack_zstd_shared_dict(&entries, 3).unwrap();
        let names: Vec<String> = parse_records(&archive)
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert!(
            !names.iter().any(|n| n == DICT_ENTRY),
            "fallback to per-entry: no dictionary stored"
        );
        assert_eq!(unpack_zstd(&archive).unwrap(), entries);
    }

    #[test]
    fn mimetype_is_stored_uncompressed_and_first() {
        let entries = sample_entries();
        let archive = pack_zstd(&entries, 3).unwrap();
        assert_eq!(read_u32(&archive, 0).unwrap(), SIG_LOCAL);
        assert_eq!(read_u16(&archive, 8).unwrap(), METHOD_STORED);
        let needle = b"application/epub+zip";
        assert!(
            archive.windows(needle.len()).any(|w| w == needle),
            "stored mimetype payload should be present verbatim"
        );
    }

    #[test]
    fn non_mimetype_entries_use_method_93() {
        let entries = sample_entries();
        let archive = pack_zstd(&entries, 3).unwrap();
        let raw_total: usize = entries.iter().map(|e| e.data.len()).sum();
        assert!(
            archive.len() < raw_total,
            "zstd archive ({}) should beat raw total ({raw_total})",
            archive.len()
        );
    }

    #[test]
    fn rejects_truncated_archive() {
        let entries = sample_entries();
        let mut archive = pack_zstd(&entries, 3).unwrap();
        archive.truncate(archive.len() / 2);
        assert!(unpack_zstd(&archive).is_err());
    }

    #[test]
    fn detects_crc_corruption() {
        let entries = sample_entries();
        let mut archive = pack_zstd(&entries, 3).unwrap();
        let idx = archive.len() / 2;
        archive[idx] ^= 0xFF;
        assert!(unpack_zstd(&archive).is_err());
    }
}
