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

use anyhow::{Result, bail};
use structured_zstd::decoding::FrameDecoder;
use structured_zstd::encoding::{CompressionLevel, compress_to_vec};

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

/// Metadata retained per entry so the central directory can be written after
/// all local headers + data have been emitted.
struct CentralRecord {
    name: String,
    method: u16,
    crc: u32,
    comp_size: u32,
    uncomp_size: u32,
    local_offset: u32,
}

/// Pack `entries` into an experimental Zstd-OCF archive.
///
/// The first entry named `mimetype` (conventionally the first entry overall) is
/// **stored** uncompressed, exactly as OCF requires; every other entry is
/// compressed with Zstandard (method 93) at `level` (C zstd level numbering,
/// 1–22). The returned bytes are a complete `.epub`-shaped ZIP — non-conformant
/// by design.
pub fn pack_zstd(entries: &[OcfEntry], level: i32) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut records: Vec<CentralRecord> = Vec::with_capacity(entries.len());

    for entry in entries {
        if entry.name.len() > u16::MAX as usize {
            bail!("entry name too long for ZIP: {}", entry.name);
        }
        let uncomp_size = u32_len(entry.data.len(), &entry.name)?;
        let crc = crc32fast::hash(&entry.data);

        // mimetype must be stored; everything else gets Zstd.
        let (method, payload) = if entry.name == "mimetype" {
            (METHOD_STORED, entry.data.clone())
        } else {
            let compressed = compress_to_vec(entry.data.as_slice(), CompressionLevel::Level(level));
            (METHOD_ZSTD, compressed)
        };
        let comp_size = u32_len(payload.len(), &entry.name)?;
        let local_offset = u32_len(buf.len(), "archive offset")?;

        // Local file header.
        push_u32(&mut buf, SIG_LOCAL);
        push_u16(&mut buf, version_needed(method));
        push_u16(&mut buf, 0); // general purpose bit flag
        push_u16(&mut buf, method);
        push_u16(&mut buf, DOS_TIME);
        push_u16(&mut buf, DOS_DATE);
        push_u32(&mut buf, crc);
        push_u32(&mut buf, comp_size);
        push_u32(&mut buf, uncomp_size);
        push_u16(&mut buf, entry.name.len() as u16);
        push_u16(&mut buf, 0); // extra field length
        buf.extend_from_slice(entry.name.as_bytes());
        buf.extend_from_slice(&payload);

        records.push(CentralRecord {
            name: entry.name.clone(),
            method,
            crc,
            comp_size,
            uncomp_size,
            local_offset,
        });
    }

    // Central directory.
    let cd_offset = u32_len(buf.len(), "central directory offset")?;
    for rec in &records {
        push_u32(&mut buf, SIG_CENTRAL);
        push_u16(&mut buf, 20); // version made by (2.0, MS-DOS)
        push_u16(&mut buf, version_needed(rec.method));
        push_u16(&mut buf, 0); // general purpose bit flag
        push_u16(&mut buf, rec.method);
        push_u16(&mut buf, DOS_TIME);
        push_u16(&mut buf, DOS_DATE);
        push_u32(&mut buf, rec.crc);
        push_u32(&mut buf, rec.comp_size);
        push_u32(&mut buf, rec.uncomp_size);
        push_u16(&mut buf, rec.name.len() as u16);
        push_u16(&mut buf, 0); // extra field length
        push_u16(&mut buf, 0); // file comment length
        push_u16(&mut buf, 0); // disk number start
        push_u16(&mut buf, 0); // internal file attributes
        push_u32(&mut buf, 0); // external file attributes
        push_u32(&mut buf, rec.local_offset);
        buf.extend_from_slice(rec.name.as_bytes());
    }
    let cd_size = u32_len(buf.len() - cd_offset as usize, "central directory size")?;

    // End of central directory.
    let count = records.len() as u16;
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

/// Parse a Zstd-OCF archive back into its uncompressed entries, in central
/// directory order. Each entry's CRC32 is verified against its decompressed
/// bytes — this is the integrity check that makes the round-trip credible.
pub fn unpack_zstd(archive: &[u8]) -> Result<Vec<OcfEntry>> {
    let eocd = find_eocd(archive)?;
    let cd_count = read_u16(archive, eocd + 10)? as usize;
    let mut cd_pos = read_u32(archive, eocd + 16)? as usize;

    let mut entries = Vec::with_capacity(cd_count);
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

        // Locate the entry payload via its local header (its own name/extra
        // lengths can differ from the central record's, so re-read them).
        if read_u32(archive, local_offset)? != SIG_LOCAL {
            bail!("corrupt archive: bad local header for {name}");
        }
        let lh_name_len = read_u16(archive, local_offset + 26)? as usize;
        let lh_extra_len = read_u16(archive, local_offset + 28)? as usize;
        let data_start = local_offset + 30 + lh_name_len + lh_extra_len;
        let payload = slice(archive, data_start, comp_size)?;

        let data = match method {
            METHOD_STORED => payload.to_vec(),
            METHOD_ZSTD => {
                let mut dec = FrameDecoder::new();
                let mut out = Vec::with_capacity(uncomp_size);
                dec.decode_all_to_vec(payload, &mut out)
                    .map_err(|e| anyhow::anyhow!("zstd decode failed for {name}: {e:?}"))?;
                out
            }
            other => bail!("unsupported compression method {other} for {name}"),
        };

        let actual = crc32fast::hash(&data);
        if actual != crc {
            bail!("CRC mismatch for {name}: expected {crc:#010x}, got {actual:#010x}");
        }
        entries.push(OcfEntry { name, data });

        cd_pos += 46 + name_len + extra_len + comment_len;
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
    // Comment max length is 65535, so we never need to look further back.
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

    #[test]
    fn round_trips_entries_byte_for_byte() {
        let entries = sample_entries();
        let archive = pack_zstd(&entries, 3).unwrap();
        let back = unpack_zstd(&archive).unwrap();
        assert_eq!(entries, back);
    }

    #[test]
    fn mimetype_is_stored_uncompressed_and_first() {
        let entries = sample_entries();
        let archive = pack_zstd(&entries, 3).unwrap();
        // First local header is at offset 0; method field is at +8.
        assert_eq!(read_u32(&archive, 0).unwrap(), SIG_LOCAL);
        assert_eq!(read_u16(&archive, 8).unwrap(), METHOD_STORED);
        // The literal mimetype bytes appear once, uncompressed.
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
        // The big chapter should be Zstd and genuinely smaller than its input.
        let back_archive_len = archive.len();
        let raw_total: usize = entries.iter().map(|e| e.data.len()).sum();
        assert!(
            back_archive_len < raw_total,
            "zstd archive ({back_archive_len}) should beat raw total ({raw_total})"
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
        // Flip a byte inside the first zstd payload (well past the stored
        // mimetype header) and expect the CRC check to catch it.
        let idx = archive.len() / 2;
        archive[idx] ^= 0xFF;
        // Either decode fails or the CRC check trips; both are errors.
        assert!(unpack_zstd(&archive).is_err());
    }
}
