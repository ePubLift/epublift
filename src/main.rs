// epublift - Optimize EPUB files: convert images to WebP and upgrade to EPUB 3.3.
// Copyright (C) 2024  Baris Kayadelen
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use anyhow::{Context, Result};
use clap::Parser;
use epublift::{EpubVersion, ImageStrategy, Options};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// CLI surface for [`epublift::ZstdMode`] (experimental).
#[cfg(feature = "zstd-experimental")]
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum ZstdModeArg {
    PerEntry,
    SharedDict,
}

#[cfg(feature = "zstd-experimental")]
impl From<ZstdModeArg> for epublift::ZstdMode {
    fn from(m: ZstdModeArg) -> Self {
        match m {
            ZstdModeArg::PerEntry => epublift::ZstdMode::PerEntry,
            ZstdModeArg::SharedDict => epublift::ZstdMode::SharedDict,
        }
    }
}

/// Optimize EPUB structure to 3.3 and convert images to WebP.
///
/// With no subcommand, `epublift -i book.epub` runs the optimizer (the original,
/// backwards-compatible behavior). The `archive` / `restore` subcommands manage
/// `.eparc` archives.
#[derive(Parser, Debug)]
#[command(
    name = "epublift",
    about = "Optimize EPUBs to 3.3 (default), or archive/restore them as .eparc.",
    after_help = "Examples:\n  epublift -i book.epub -q 75\n  epublift archive ~/Books            # shrink a library to .eparc\n  epublift restore book.eparc         # back to a content-exact .epub",
    args_conflicts_with_subcommands = true
)]
struct Args {
    #[command(subcommand)]
    #[cfg(feature = "archival")]
    command: Option<Command>,

    // ----- default (optimize) options; used when no subcommand is given -----
    /// Path to original EPUB file to lift
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Path to save the optimized EPUB (optional)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// WebP compression quality from 1 to 100 (default: 80)
    #[arg(short, long, default_value_t = 80)]
    quality: i32,

    /// Path to write the summary size report (optional)
    #[arg(short, long)]
    report: Option<PathBuf>,

    /// Transliterate auto-generated output/report names to ASCII
    /// (e.g. "Işık Doğudan" -> "Isik_Dogudan"). Ignored when -o/-r are given.
    #[arg(long)]
    ascii: bool,

    /// Produce a Kobo .kepub.epub: inject koboSpan markup for Kobo's reading
    /// features. Composes with the normal upgrades; output is named
    /// "<name>.kepub.epub" unless -o is given. Implies --keep-images, since Kobo
    /// e-ink readers cannot render WebP.
    #[arg(long)]
    kepub: bool,

    /// Keep images in their original format (skip JPEG/PNG -> WebP). Use this for
    /// readers that don't render WebP — notably Kobo e-ink devices. Structure is
    /// still upgraded to EPUB 3.3.
    #[arg(long)]
    keep_images: bool,

    /// [EXPERIMENTAL] Target EPUB version: "3.3" (default) or "3.4". 3.4 re-encodes
    /// images to AVIF (or JPEG XL), which become core media types in EPUB 3.4.
    /// Requires the `epub34` build feature. See docs/epub-3.4.md.
    #[cfg(feature = "epub34")]
    #[arg(long, default_value = "3.3", value_name = "3.3|3.4")]
    target: String,

    /// [EXPERIMENTAL] Image format for --target 3.4: "avif" (default) or "jxl".
    #[cfg(feature = "epub34")]
    #[arg(long, value_name = "avif|jxl")]
    image_format: Option<String>,

    /// [EXPERIMENTAL] Package the container with Zstandard (ZIP method 93)
    /// instead of Deflate, to measure the size delta. The result is
    /// NON-CONFORMANT and will NOT open in current reading systems — research
    /// only. See docs/design/zstd-ocf-experimental.md.
    #[cfg(feature = "zstd-experimental")]
    #[arg(long)]
    zstd: bool,

    /// [EXPERIMENTAL] Zstandard level (C zstd numbering, 1-22). Higher = smaller
    /// and slower.
    #[cfg(feature = "zstd-experimental")]
    #[arg(long, default_value_t = 19, value_name = "1-22")]
    zstd_level: i32,

    /// [EXPERIMENTAL] How Zstandard shares context across entries: `per-entry`
    /// (each entry independent) or `shared-dict` (one dictionary trained from
    /// the book's text, stored as META-INF/zstd-dict.bin — the cross-chapter
    /// win). Only meaningful with --zstd.
    #[cfg(feature = "zstd-experimental")]
    #[arg(
        long,
        value_name = "per-entry|shared-dict",
        default_value = "per-entry"
    )]
    zstd_mode: ZstdModeArg,

    /// [EXPERIMENTAL] Decode a *_zstd-experimental.epub back into a conformant
    /// Deflate EPUB (the lossless round-trip check). With this flag, --input is
    /// the experimental archive.
    #[cfg(feature = "zstd-experimental")]
    #[arg(long)]
    zstd_decode: bool,
}

/// `.eparc` archive subcommands (see docs/design/eparc-format.md).
#[cfg(feature = "archival")]
#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Shrink EPUB(s) into compact `.eparc` archives to save disk space.
    Archive(ArchiveArgs),
    /// Restore `.eparc` archive(s) back to a content-exact `.epub`.
    Restore(RestoreArgs),
}

#[cfg(feature = "archival")]
#[derive(clap::Args, Debug)]
struct ArchiveArgs {
    /// EPUB file(s), or a directory whose `.epub` files are all archived.
    #[arg(required = true, value_name = "EPUB|DIR")]
    paths: Vec<PathBuf>,

    /// Directory to write `.eparc` files into (default: next to each input).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[cfg(feature = "archival")]
#[derive(clap::Args, Debug)]
struct RestoreArgs {
    /// `.eparc` archive file(s), or a directory of them.
    #[arg(required = true, value_name = "EPARC|DIR")]
    paths: Vec<PathBuf>,

    /// Directory to write restored `.epub` files into (default: next to each input).
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Re-emit at a specific EPUB version instead of content-exact. Only 3.3 is
    /// supported today (EPUB 3.4 is not yet published).
    #[arg(long, value_name = "3.3")]
    target: Option<String>,

    /// Modernize to a clean, current EPUB (equivalent to --target 3.3).
    #[arg(long)]
    modernize: bool,

    /// Keep original images (no WebP) when re-targeting — for readers that don't
    /// render WebP, e.g. Kobo e-ink.
    #[arg(long)]
    keep_images: bool,

    /// Produce a Kobo `.kepub.epub` when re-targeting (implies --keep-images).
    #[arg(long)]
    kepub: bool,

    /// WebP quality (1-100) when re-targeting with image conversion.
    #[arg(short, long, default_value_t = 80)]
    quality: i32,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\n[!] Fatal Error: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<()> {
    #[cfg(feature = "archival")]
    match &args.command {
        Some(Command::Archive(a)) => return run_archive(a),
        Some(Command::Restore(r)) => return run_restore(r),
        None => {}
    }
    run_convert(args)
}

/// The default (no-subcommand) optimize path — the original CLI behavior.
fn run_convert(args: Args) -> Result<()> {
    let input = args.input.clone().context(
        "no EPUB given — pass -i <FILE>, or a subcommand (archive/restore). See --help.",
    )?;
    let input = input
        .canonicalize()
        .with_context(|| format!("Input file not found: {}", input.display()))?;

    // Experimental decode mode: reconstruct a conformant EPUB and return early.
    #[cfg(feature = "zstd-experimental")]
    if args.zstd_decode {
        let output = args.output.clone().unwrap_or_else(|| {
            let name = input
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .replace("_zstd-experimental.epub", "_decoded.epub");
            let name = if name.ends_with("_decoded.epub") {
                name
            } else {
                format!("{}_decoded.epub", epublift::output_stem(&input, false))
            };
            input.parent().unwrap_or_else(|| Path::new(".")).join(name)
        });
        println!(
            "[*] Decoding experimental Zstd-OCF archive: {}",
            input.display()
        );
        epublift::decode_zstd_epub(&input, &output)?;
        println!("[+] Reconstructed conformant EPUB: {}", output.display());
        return Ok(());
    }

    #[cfg(feature = "zstd-experimental")]
    let packaging = if args.zstd {
        epublift::Packaging::Zstd {
            mode: args.zstd_mode.into(),
            level: args.zstd_level,
        }
    } else {
        epublift::Packaging::Deflate
    };
    #[cfg(not(feature = "zstd-experimental"))]
    let packaging = epublift::Packaging::Deflate;

    // Target version + image format. 3.4 (AVIF/JXL) is experimental and only
    // available under the `epub34` feature; the default build is 3.3/WebP.
    #[cfg(feature = "epub34")]
    let target_version = match args.target.trim() {
        "3.3" => EpubVersion::V3_3,
        "3.4" => EpubVersion::V3_4,
        other => anyhow::bail!("unknown --target '{other}'. Supported: 3.3, 3.4."),
    };
    #[cfg(not(feature = "epub34"))]
    let target_version = EpubVersion::LATEST;
    #[cfg(feature = "epub34")]
    let image_format = match args.image_format.as_deref() {
        None => None,
        Some("avif") => Some(epublift::ImageFormat::Avif),
        Some("jxl") => Some(epublift::ImageFormat::Jxl),
        Some(other) => {
            anyhow::bail!("unknown --image-format '{other}'. Supported: avif, jxl.")
        }
    };
    #[cfg(not(feature = "epub34"))]
    let image_format = None;

    let mut options = Options {
        quality: args.quality.clamp(1, 100) as u8,
        ascii: args.ascii,
        target_version,
        image_strategy: if args.keep_images {
            ImageStrategy::KeepOriginal
        } else {
            ImageStrategy::WebP
        },
        image_format,
        kepub: args.kepub,
        packaging,
        output: args.output.clone(),
    };
    // Resolve the output path up front so we can show it before converting.
    let output_path = options
        .output
        .clone()
        .unwrap_or_else(|| epublift::default_output_path(&input, &options));
    options.output = Some(output_path.clone());

    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let report_path = match args.report {
        Some(p) => p,
        None => parent.join(format!(
            "{}_report.txt",
            epublift::output_stem(&input, args.ascii)
        )),
    };

    let input_name = input
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    println!("[*] Starting optimization for: {}", input_name);
    println!("[*] Target output path: {}", output_path.display());
    println!("[*] WebP Image Quality: {}%", options.quality);

    let report = epublift::convert(&input, &options, |msg| println!("{}", msg))?;

    // Step 6: Generate report.
    report.write_text_report(&report_path)?;

    println!("\n[+] Optimization complete!");
    println!("[+] Output EPUB: {}", report.output_path.display());
    println!("[+] Report file: {}", report_path.display());
    println!(
        "[+] Size reduced from {:.2} MB to {:.2} MB ({:.1}% savings)",
        report.original_size as f64 / 1024.0 / 1024.0,
        report.final_size as f64 / 1024.0 / 1024.0,
        report.percent_saved()
    );

    Ok(())
}

/// `epublift archive` — shrink EPUB(s) into `.eparc`.
#[cfg(feature = "archival")]
fn run_archive(args: &ArchiveArgs) -> Result<()> {
    use anyhow::bail;

    let epubs = collect_with_ext(&args.paths, "epub");
    if epubs.is_empty() {
        bail!("no .epub files found in the given paths");
    }
    if let Some(dir) = &args.output {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create output directory: {}", dir.display()))?;
    }

    let (mut total_in, mut total_out) = (0u64, 0u64);
    for epub in &epubs {
        let out = sibling_path(epub, "eparc", args.output.as_deref());
        let stats = epublift::eparc::archive_epub(epub, &out)
            .with_context(|| format!("Failed to archive {}", epub.display()))?;
        total_in += stats.original_size;
        total_out += stats.archive_size;
        println!(
            "[+] {} -> {} ({:.1}% smaller; {} compressed + {} stored)",
            file_name(epub),
            file_name(&out),
            stats.percent_saved(),
            stats.compressed_entries,
            stats.stored_entries,
        );
    }
    if epubs.len() > 1 {
        println!(
            "[=] {} books: {:.2} MB -> {:.2} MB ({:.1}% smaller)",
            epubs.len(),
            total_in as f64 / 1024.0 / 1024.0,
            total_out as f64 / 1024.0 / 1024.0,
            saved_pct(total_in, total_out),
        );
    }
    Ok(())
}

/// `epublift restore` — `.eparc` back to a `.epub`. Content-exact by default; with
/// `--target`/`--modernize`/`--keep-images`/`--kepub` it runs the optimizer on the
/// restored book so the output matches the reader/device the user is targeting.
#[cfg(feature = "archival")]
fn run_restore(args: &RestoreArgs) -> Result<()> {
    use anyhow::bail;

    let archives = collect_with_ext(&args.paths, "eparc");
    if archives.is_empty() {
        bail!("no .eparc files found in the given paths");
    }
    if let Some(dir) = &args.output {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create output directory: {}", dir.display()))?;
    }

    // A re-target is requested when any transform flag is present; otherwise the
    // restore is content-exact (the original book, byte-for-byte).
    let retarget = args.target.is_some() || args.modernize || args.keep_images || args.kepub;
    let target_version = match &args.target {
        Some(t) => parse_target(t)?,
        None => EpubVersion::LATEST,
    };

    for eparc in &archives {
        if !retarget {
            let out = sibling_path(eparc, "epub", args.output.as_deref());
            let stats = epublift::eparc::restore_eparc(eparc, &out)
                .with_context(|| format!("Failed to restore {}", eparc.display()))?;
            println!(
                "[+] {} -> {} ({} entries, {:.2} MB)",
                file_name(eparc),
                file_name(&out),
                stats.entries,
                stats.output_size as f64 / 1024.0 / 1024.0,
            );
            continue;
        }

        // Re-target: restore content-exact into a temp dir, then run convert().
        let tmp = tempfile::Builder::new()
            .prefix("eparc_restore_")
            .tempdir()?;
        let restored = tmp.path().join("restored.epub");
        epublift::eparc::restore_eparc(eparc, &restored)
            .with_context(|| format!("Failed to restore {}", eparc.display()))?;

        let out_dir = args.output.clone().unwrap_or_else(|| {
            eparc
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        });
        let options = Options {
            quality: args.quality.clamp(1, 100) as u8,
            ascii: false,
            target_version,
            image_strategy: if args.keep_images {
                ImageStrategy::KeepOriginal
            } else {
                ImageStrategy::WebP
            },
            image_format: None,
            kepub: args.kepub,
            packaging: epublift::Packaging::Deflate,
            output: None,
        };
        // Name the output the way `convert` does, but in the chosen directory and
        // based on the book's name (not the temp file).
        let stem = eparc
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "book".to_string());
        let name_basis = out_dir.join(format!("{stem}.epub"));
        let final_out = epublift::default_output_path(&name_basis, &options);
        let options = Options {
            output: Some(final_out.clone()),
            ..options
        };

        let report = epublift::convert(&restored, &options, |_| {})
            .with_context(|| format!("Failed to re-target {}", eparc.display()))?;
        println!(
            "[+] {} -> {} (re-targeted: {})",
            file_name(eparc),
            file_name(&report.output_path),
            retarget_label(args),
        );
    }
    Ok(())
}

/// Parse the `--target` value into an [`EpubVersion`], with friendly errors for
/// the versions we deliberately don't support yet.
#[cfg(feature = "archival")]
fn parse_target(s: &str) -> Result<EpubVersion> {
    use anyhow::bail;
    match s.trim() {
        "3.3" => Ok(EpubVersion::V3_3),
        "3.4" => {
            bail!("EPUB 3.4 is not supported yet (the spec isn't published). Use --target 3.3.")
        }
        "2" | "2.0" => {
            bail!("downgrading to EPUB 2.0 isn't supported — epublift is an upgrader.")
        }
        other => bail!("unknown --target '{other}'. Supported: 3.3."),
    }
}

/// A short human label of the re-target for the restore output line.
#[cfg(feature = "archival")]
fn retarget_label(args: &RestoreArgs) -> String {
    let mut parts = Vec::new();
    if args.kepub {
        parts.push("kepub".to_string());
    } else {
        parts.push(format!("EPUB {}", EpubVersion::LATEST.tag()));
    }
    if args.keep_images {
        parts.push("original images".to_string());
    }
    parts.join(", ")
}

/// Expand the given paths into a sorted, de-duplicated list of files with the
/// wanted extension (directories are scanned recursively).
#[cfg(feature = "archival")]
fn collect_with_ext(paths: &[PathBuf], ext: &str) -> Vec<PathBuf> {
    use walkdir::WalkDir;
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            for e in WalkDir::new(p).into_iter().filter_map(|e| e.ok()) {
                if e.file_type().is_file() && has_ext(e.path(), ext) {
                    out.push(e.into_path());
                }
            }
        } else if has_ext(p, ext) {
            out.push(p.clone());
        } else {
            eprintln!("[skip] not a .{ext} or directory: {}", p.display());
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(feature = "archival")]
fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

/// `<stem>.<new_ext>`, placed in `out_dir` if given, else next to `path`.
#[cfg(feature = "archival")]
fn sibling_path(path: &Path, new_ext: &str, out_dir: Option<&Path>) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    let name = format!("{stem}.{new_ext}");
    match out_dir {
        Some(d) => d.join(name),
        None => path.parent().unwrap_or_else(|| Path::new(".")).join(name),
    }
}

#[cfg(feature = "archival")]
fn file_name(path: &Path) -> std::borrow::Cow<'_, str> {
    path.file_name().unwrap_or_default().to_string_lossy()
}

#[cfg(feature = "archival")]
fn saved_pct(input: u64, output: u64) -> f64 {
    if input > 0 {
        (1.0 - output as f64 / input as f64) * 100.0
    } else {
        0.0
    }
}
