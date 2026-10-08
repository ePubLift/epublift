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
    // `-V` / `--version`: `epublift <semver>[+<hash>[.dirty]]` (see build.rs).
    version = concat!(env!("CARGO_PKG_VERSION"), env!("EPUBLIFT_BUILD")),
    about = "Optimize EPUBs to 3.3 (default), or archive/restore them as .eparc.",
    after_help = "Examples:\n  epublift -i book.epub -q 75\n  epublift archive ~/Books            # shrink a library to .eparc\n  epublift restore book.eparc         # back to a content-exact .epub",
    args_conflicts_with_subcommands = true
)]
struct Args {
    #[command(subcommand)]
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
    /// "<name>_v3.3.kepub.epub" unless -o is given. Keeps original images by
    /// default (stock Kobo shows WebP as blank); use --kepub-webp to opt into WebP.
    #[arg(long)]
    kepub: bool,

    /// Keep images in their original format (skip JPEG/PNG -> WebP). Use this for
    /// readers that don't render WebP — notably Kobo e-ink devices. Structure is
    /// still upgraded to EPUB 3.3.
    #[arg(long)]
    keep_images: bool,

    /// With --kepub, emit WebP images instead of keeping originals. Only for Kobo
    /// devices that have the WebP image plugin installed (see kobo-webp-plugin/);
    /// without it, Kobo shows blank images. No effect unless --kepub is set.
    #[arg(long)]
    kepub_webp: bool,

    /// [EXPERIMENTAL] Target EPUB version: "3.3" (default) or "3.4". 3.4 uses the
    /// new core image types content-adaptively: photos (JPEG sources) → AVIF,
    /// line-art (PNG sources) → WebP. Needs the `epub34` feature. See docs/epub-3.4.md.
    #[cfg(feature = "epub34")]
    #[arg(long, default_value = "3.3", value_name = "3.3|3.4")]
    target: String,

    /// [EXPERIMENTAL] Image format for EPUB 3.4 (implies --target 3.4): "avif" or
    /// "jxl" forces one format; "best" encodes every candidate per image and keeps
    /// the smallest (thorough but slow). Default (no flag) is content-adaptive —
    /// AVIF for JPEG sources, WebP for PNG.
    #[cfg(feature = "epub34")]
    #[arg(long, value_name = "avif|jxl|best")]
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

/// Top-level subcommands. `meta` is always available; the `.eparc` archive
/// commands need the `archival` feature (default). See docs/.
// clap subcommand args are constructed once per invocation, so the size spread
// between variants doesn't matter (and boxing breaks the derive).
#[allow(clippy::large_enum_variant)]
#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Read or edit the book's metadata (title, authors, identifiers, …).
    Meta(MetaArgs),
    /// Shrink EPUB(s) into compact `.eparc` archives to save disk space.
    #[cfg(feature = "archival")]
    Archive(ArchiveArgs),
    /// Restore `.eparc` archive(s) back to a content-exact `.epub`.
    #[cfg(feature = "archival")]
    Restore(RestoreArgs),
    /// [EXPERIMENTAL] Import a PDF or Markdown file into a reflowable EPUB.
    #[cfg(any(feature = "pdf", feature = "markdown"))]
    Import(ImportArgs),
    /// Validate EPUB(s) against the spec (pure-Rust epubcheck alternative).
    #[cfg(feature = "validate")]
    Check(CheckArgs),
    /// Fix the defects `check` finds that have one safe fix, asking before
    /// any fix that needs a decision (runs epubsana).
    Repair(RepairArgs),
}

/// `epublift repair …` — fix what `check` finds, with epubsana (writes a new
/// EPUB; input untouched). See docs/repair.md.
///
/// Exit: 0 = the goal was met; 1 = defects remain; 2 = the book could not be
/// repaired at all (unreadable, or the output path is the input).
#[derive(clap::Args, Debug)]
struct RepairArgs {
    /// EPUB file to repair (never modified; a new file is written).
    #[arg(value_name = "EPUB")]
    input: PathBuf,
    /// Output path (default: `<name>_repaired.epub` next to the input).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Report what would be fixed without writing anything.
    #[arg(long, conflicts_with = "yes")]
    dry_run: bool,
    /// Apply every proposed fix without asking, including the ones that need a
    /// decision. Without it, safe fixes are applied and the rest are asked
    /// about in a terminal, or skipped when there is no terminal to ask in.
    #[arg(short, long)]
    yes: bool,
    /// How far to repair: `valid` (no errors remain) or `openable` (no fatal
    /// errors remain, so the book opens; errors may still be reported).
    #[arg(long, value_enum, value_name = "valid|openable", default_value_t = RepairGoal::Valid)]
    goal: RepairGoal,
    // The canonical one-liner, verbatim, as on `check`.
    #[arg(
        long,
        help = FORMAT_HELP,
        value_enum,
        value_name = "human|json",
        default_value_t = ReportFormat::Human
    )]
    format: ReportFormat,
}

/// `repair --goal`: epubsana's two bars, spelled as its own CLI spells them.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum RepairGoal {
    /// No fatal and no error findings remain.
    Valid,
    /// No fatal findings remain: the book opens.
    Openable,
}

impl From<RepairGoal> for epubsana::Goal {
    fn from(g: RepairGoal) -> Self {
        match g {
            RepairGoal::Valid => epubsana::Goal::Valid,
            RepairGoal::Openable => epubsana::Goal::Openable,
        }
    }
}

/// The veripublica conventions version whose FORMATS.md the `check` and
/// `repair` json envelopes meet — asserted by epublift about itself (FORMATS §1.1).
const ENVELOPE_CONVENTION: &str = "0.6";

/// `--format`'s help: veripublica conventions' canonical one-liner, verbatim.
const FORMAT_HELP: &str =
    "Report format. `human` (the default) is always supported; `json` is reserved for FORMATS.md.";

/// What `check --format` and `repair --format` can emit (veripublica
/// conventions CLI.md §3.6).
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum ReportFormat {
    /// A report for people.
    Human,
    /// The veripublica machine envelope (FORMATS.md): one JSON object per run.
    Json,
}

/// `epublift check …` — validate EPUB file(s) against the EPUB spec using our
/// pure-Rust epubveri engine (a JVM-free epubcheck alternative).
///
/// Exit: 0 = every input is valid; 1 = every input was validated and at least
/// one is invalid; 2 = an input could not be read at all. See docs/validate.md.
#[cfg(feature = "validate")]
#[derive(clap::Args, Debug)]
struct CheckArgs {
    /// EPUB file(s) to validate.
    #[arg(required = true, value_name = "EPUB")]
    paths: Vec<PathBuf>,

    /// Validate against an EPUB extension profile: `dict` (dictionaries),
    /// `edupub`, `idx` (indexes) or `preview`. Default: base EPUB 3 only.
    #[arg(long, value_name = "dict|edupub|idx|preview")]
    profile: Option<String>,

    // The canonical one-liner, verbatim (veripublica conventions CLI.md §3.1, §7);
    // an explicit `help` so clap keeps the closing full stop.
    #[arg(
        long,
        help = FORMAT_HELP,
        value_enum,
        value_name = "human|json",
        default_value_t = ReportFormat::Human,
        conflicts_with = "json"
    )]
    format: ReportFormat,

    /// Same as `--format json`.
    #[arg(long)]
    json: bool,

    /// Only report files that have problems; stay silent on clean passes.
    #[arg(short, long)]
    quiet: bool,
}

/// `epublift import …` — [EXPERIMENTAL] convert a PDF or Markdown file to a reflow
/// EPUB (routed by file extension). See docs/pdf-import.md.
#[cfg(any(feature = "pdf", feature = "markdown"))]
#[derive(clap::Args, Debug)]
struct ImportArgs {
    /// Path to the input file (PDF or Markdown, by extension).
    #[arg(short, long)]
    input: PathBuf,

    /// Path to write the EPUB (default: alongside the input).
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// [PDF only] Output layout: "reflow" (default, a real reflowable ebook) or
    /// "fixed" (preserve the page images — picture books, comics).
    #[arg(long, default_value = "reflow", value_name = "reflow|fixed")]
    mode: String,

    /// Content language (BCP-47, e.g. "tr"). Used for de-hyphenation/metadata.
    #[arg(long)]
    language: Option<String>,
}

/// `epublift meta …` — read or edit a book's metadata. See docs/metadata.md.
#[derive(clap::Args, Debug)]
struct MetaArgs {
    #[command(subcommand)]
    action: MetaAction,
}

#[allow(clippy::large_enum_variant)]
#[derive(clap::Subcommand, Debug)]
enum MetaAction {
    /// Print the book's current metadata.
    Show(MetaShowArgs),
    /// Edit the book's metadata by hand (writes a new EPUB; input untouched).
    Set(MetaSetArgs),
    /// Fill missing metadata from an online catalogue by ISBN (needs the
    /// `metadata` build feature).
    #[cfg(feature = "metadata")]
    Enrich(MetaEnrichArgs),
}

#[derive(clap::Args, Debug)]
struct MetaShowArgs {
    /// EPUB file to read.
    #[arg(value_name = "EPUB")]
    input: PathBuf,
    // The canonical one-liner, verbatim (CLI.md §3.1, §7). It holds here too:
    // `json` is *reserved*, which is why this command offers `metadata` instead.
    #[arg(
        long,
        help = FORMAT_HELP,
        value_enum,
        value_name = "human|metadata",
        default_value_t = MetaFormat::Human,
        conflicts_with = "json"
    )]
    format: MetaFormat,
    /// Same as `--format metadata`.
    #[arg(long)]
    json: bool,
}

/// What `meta show --format` can emit. Not `json`: that name is reserved for
/// the veripublica envelope (conventions CLI.md §3.6), and this is a plain
/// metadata object, so it gets a format name of its own.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum MetaFormat {
    /// A table for people.
    Human,
    /// The book's metadata as one JSON object.
    Metadata,
}

#[derive(clap::Args, Debug)]
struct MetaSetArgs {
    /// EPUB file to edit (never modified; a new file is written).
    #[arg(value_name = "EPUB")]
    input: PathBuf,
    /// Set the main title.
    #[arg(long)]
    title: Option<String>,
    /// Set the subtitle.
    #[arg(long)]
    subtitle: Option<String>,
    /// Set an author (repeatable, in order). Replaces all existing authors.
    #[arg(long = "author", value_name = "NAME")]
    authors: Vec<String>,
    /// Set the language (BCP-47, e.g. `tr`, `en`, `ko`).
    #[arg(long)]
    language: Option<String>,
    /// Set the publisher.
    #[arg(long)]
    publisher: Option<String>,
    /// Set the publication date (ISO 8601, e.g. `2024-03-15`).
    #[arg(long)]
    date: Option<String>,
    /// Set the description.
    #[arg(long)]
    description: Option<String>,
    /// Set a subject/tag (repeatable). Replaces all existing subjects.
    #[arg(long = "subject", value_name = "TERM")]
    subjects: Vec<String>,
    /// Set the series, as `Name` or `Name:position` (e.g. `Dune:2`).
    #[arg(long, value_name = "NAME[:POS]")]
    series: Option<String>,
    /// Set the print ISBN (written as `dc:source` `urn:isbn:…`).
    #[arg(long)]
    isbn: Option<String>,
    /// Make this JPEG or PNG the cover (put in as given, at most 10 MB).
    #[arg(long, value_name = "IMAGE")]
    cover: Option<PathBuf>,
    /// Output path (default: `<name>_meta.epub` next to the input).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[cfg(feature = "metadata")]
#[derive(clap::Args, Debug)]
struct MetaEnrichArgs {
    /// EPUB file to enrich (never modified; a new file is written).
    #[arg(value_name = "EPUB")]
    input: PathBuf,
    /// ISBN to look up (ISBN-13 recommended; hyphens allowed).
    #[arg(long)]
    isbn: String,
    /// Override the book's language (BCP-47) when the OPF has no `dc:language`.
    #[arg(long)]
    lang: Option<String>,
    /// Metadata provider: `openlibrary` (default) or `google` (Google Books).
    #[arg(long, default_value = "openlibrary", value_name = "openlibrary|google")]
    provider: String,
    /// Preview the changes without writing anything.
    #[arg(long)]
    dry_run: bool,
    /// Replace fields that already have a value (default: fill gaps only).
    #[arg(long)]
    overwrite: bool,
    /// Keep fields whose language doesn't match the book's (default: skip them).
    #[arg(long)]
    allow_foreign_meta: bool,
    /// Also fetch and write the description (often publisher-authored).
    #[arg(long)]
    include_description: bool,
    /// Also write the catalogue's subjects. Off by default: Open Library merges
    /// many libraries' headings, some in other languages (they are listed).
    #[arg(long)]
    include_subjects: bool,
    /// Also make the catalogue's cover the book's cover, when it has one.
    #[arg(long)]
    cover: bool,
    /// Output path (default: `<name>_meta.epub` next to the input).
    #[arg(short, long)]
    output: Option<PathBuf>,
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

    /// Re-emit at a specific EPUB version instead of content-exact: "3.3", or
    /// "3.4" (experimental, AVIF/JXL — needs the `epub34` build feature).
    #[arg(long, value_name = "3.3|3.4")]
    target: Option<String>,

    /// Modernize to a clean, current EPUB (equivalent to --target 3.3).
    #[arg(long)]
    modernize: bool,

    /// Keep original images (no WebP) when re-targeting — for readers that don't
    /// render WebP, e.g. Kobo e-ink.
    #[arg(long)]
    keep_images: bool,

    /// Produce a Kobo `.kepub.epub` when re-targeting (keeps original images by
    /// default; use --kepub-webp to opt into WebP).
    #[arg(long)]
    kepub: bool,

    /// With --kepub, emit WebP images instead of keeping originals. Only for Kobo
    /// devices with the WebP image plugin installed (see kobo-webp-plugin/).
    #[arg(long)]
    kepub_webp: bool,

    /// WebP quality (1-100) when re-targeting with image conversion.
    #[arg(short, long, default_value_t = 80)]
    quality: i32,

    /// [EXPERIMENTAL] Image format for a 3.4 re-target (implies --target 3.4):
    /// "avif" or "jxl" forces one, "best" keeps the smallest per image. Default
    /// (no flag) is content-adaptive — AVIF for JPEG sources, WebP for PNG.
    #[cfg(feature = "epub34")]
    #[arg(long, value_name = "avif|jxl|best")]
    image_format: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // `{:#}` prints the full anyhow context chain (e.g. "… failed:
            // rate limited (HTTP 429) …") so the root cause is visible.
            eprintln!("\n[!] Fatal Error: {:#}", e);
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<()> {
    match &args.command {
        Some(Command::Meta(m)) => return run_meta(m),
        #[cfg(feature = "archival")]
        Some(Command::Archive(a)) => return run_archive(a),
        #[cfg(feature = "archival")]
        Some(Command::Restore(r)) => return run_restore(r),
        #[cfg(any(feature = "pdf", feature = "markdown"))]
        Some(Command::Import(i)) => return run_import(i),
        #[cfg(feature = "validate")]
        Some(Command::Check(c)) => return run_check(c),
        Some(Command::Repair(r)) => return run_repair(r),
        None => {}
    }
    run_convert(args)
}

/// `epublift import` — [EXPERIMENTAL] convert a PDF or Markdown file to a reflow
/// EPUB, routed by the input's file extension.
#[cfg(any(feature = "pdf", feature = "markdown"))]
fn run_import(args: &ImportArgs) -> Result<()> {
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| args.input.with_extension("epub"));
    let ext = args
        .input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // A folder of Markdown, or a `.zip` of Markdown-plus-images, becomes one book:
    // every `.md` is appended in filename order and a `cover.*` image is used as
    // the cover. A single `.md` file imports just that file.
    #[cfg(feature = "markdown")]
    if args.input.is_dir()
        || ext == "zip"
        || matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkd" | "mkdn")
    {
        use epublift::markdown::{self, ImportOptions};
        let opts = ImportOptions {
            language: args.language.clone(),
        };
        let summary = if args.input.is_dir() {
            markdown::import_dir(&args.input, &output, &opts)?
        } else if ext == "zip" {
            let bytes = std::fs::read(&args.input)
                .with_context(|| format!("failed to read {}", args.input.display()))?;
            let title = args.input.file_stem().and_then(|s| s.to_str());
            markdown::import_zip(&bytes, &output, title, &opts)?
        } else {
            markdown::import(&args.input, &output, &opts)?
        };
        eprintln!(
            "[EXPERIMENTAL] imported {} → {} ({} chapters, {} images)",
            args.input.display(),
            output.display(),
            summary.chapters,
            summary.images,
        );
        return Ok(());
    }

    #[cfg(feature = "pdf")]
    if ext == "pdf" {
        use epublift::pdf::{self, ImportOptions, Mode};
        let mode = match args.mode.as_str() {
            "fixed" => Mode::Fixed,
            "reflow" => Mode::Reflow,
            other => anyhow::bail!("unknown --mode '{other}' (expected 'reflow' or 'fixed')"),
        };
        let opts = ImportOptions {
            mode,
            language: args.language.clone(),
        };
        let summary = pdf::import(&args.input, &output, &opts)?;
        eprintln!(
            "[EXPERIMENTAL] imported {} → {} ({} chapters, {} paragraphs)",
            args.input.display(),
            output.display(),
            summary.chapters,
            summary.paragraphs,
        );
        return Ok(());
    }

    let mut kinds: Vec<&str> = Vec::new();
    #[cfg(feature = "markdown")]
    kinds.push(".md/.markdown");
    #[cfg(feature = "pdf")]
    kinds.push(".pdf");
    anyhow::bail!(
        "don't know how to import {} — supported input types in this build: {}",
        args.input.display(),
        kinds.join(", "),
    )
}

/// `epublift check …` — validate EPUB file(s) with the pure-Rust epubveri
/// engine. Prints a per-file report (human-readable or `--format json`, the shared
/// veripublica envelope). Exit: `0` clean, `1` a book is invalid, `2` an input
/// could not be read at all. See docs/validate.md.
#[cfg(feature = "validate")]
fn run_check(args: &CheckArgs) -> Result<()> {
    use epubveri::envelope::{Envelope, Input};
    use std::io::Write;

    let profile = args.profile.as_deref();
    // One Input per path, in command-line order, each self-contained — the shape
    // the envelope wants. Every input is processed even if an earlier one could
    // not be read: a broken book must not hide the reports for the rest.
    let inputs: Vec<Input> = args
        .paths
        .iter()
        .map(|path| {
            let name = path.display().to_string();
            match epubveri::validate_path_with_profile(path, profile) {
                // No severity is filtered out, so nothing is `suppressed`.
                Ok(report) => Input::from_report(name, &report, &[]),
                // Not a verdict: we never got far enough to grade the book.
                Err(e) => Input::from_error(name, e.to_string()),
            }
        })
        .collect();

    // `for_tool` derives the aggregate status with the exit code's precedence,
    // so the status we print and the code we exit with cannot disagree.
    // The `convention` key is our own claim about the output's shape (FORMATS
    // §1.1), not inherited from epubveri: this envelope meets FORMATS 0.6. The
    // `check` command line itself does not follow CLI.md (see docs/validate.md).
    let envelope = Envelope::for_tool(
        "epublift",
        env!("CARGO_PKG_VERSION"),
        ENVELOPE_CONVENTION,
        None,
        inputs,
    );

    // `--json` is the older spelling of `--format json`; clap rejects the two
    // together, so only one of them can be set here.
    let format = if args.json {
        ReportFormat::Json
    } else {
        args.format
    };
    if format == ReportFormat::Json {
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        let mut stdout = std::io::stdout().lock();
        for input in &envelope.inputs {
            let name = &input.path;
            if let Some(err) = &input.error {
                let _ = writeln!(stdout, "ERROR {name}  (could not read: {err})");
                continue;
            }
            let (fatals, errs, warns) = match &input.summary {
                Some(s) => (s.fatals, s.errors, s.warnings),
                None => (0, 0, 0),
            };
            let pass = input.status == "ok";
            if pass && warns == 0 && args.quiet {
                continue;
            }
            // Fatals are counted apart from errors, so a book stopped dead by a
            // corrupt container would otherwise read "FAIL (0 errors)". Name them
            // when there are any; stay quiet in the ordinary case.
            let fatal_part = if fatals > 0 {
                format!("{fatals} fatal{}, ", plural(fatals))
            } else {
                String::new()
            };
            let status = if pass { "PASS" } else { "FAIL" };
            let _ = writeln!(
                stdout,
                "{status}  {name}  ({fatal_part}{errs} error{}, {warns} warning{})",
                plural(errs),
                plural(warns)
            );
            for m in &input.items {
                // Show the exact source spot when epubveri pinned one:
                // `file.xhtml:12:5` reads like a compiler diagnostic and
                // lets a fixer jump straight to it.
                let loc = match (&m.location, &m.position) {
                    (Some(l), Some(p)) => format!("{l}:{}:{}", p.line, p.column),
                    (Some(l), None) => l.clone(),
                    (None, Some(p)) => format!("{}:{}", p.line, p.column),
                    (None, None) => String::new(),
                };
                let sep = if loc.is_empty() { "" } else { "  " };
                // The envelope carries the wire value (lowercase, per FORMATS.md);
                // the human report has always shouted it, and it scans better in a
                // wall of findings. The JSON is the contract — this line isn't.
                let _ = writeln!(
                    stdout,
                    "    {:<7} {}  {}{sep}{}",
                    m.severity.to_uppercase(),
                    m.code,
                    loc,
                    m.message
                );
            }
        }
        // Honest footer: epubveri is pre-1.0 and not yet full epubcheck parity.
        if !args.quiet || envelope.status != "ok" {
            eprintln!(
                "\nValidated with epubveri (pure-Rust, JVM-free; pre-1.0 — 99.7% of \
                 epubcheck's own test cases caught with the same message ID, not yet \
                 full parity)."
            );
        }
    }

    // The envelope's status is defined as a mirror of the exit code, so read it
    // back rather than re-deriving: `problems` = a book is invalid (a verdict),
    // `error` = an input could not be processed at all. Exiting directly (not via
    // anyhow) keeps a verdict from printing as "Fatal Error".
    let code = match envelope.status {
        "ok" => 0,
        "problems" => 1,
        _ => 2,
    };
    if code != 0 {
        std::io::stdout().flush().ok();
        std::process::exit(code);
    }
    Ok(())
}

/// `""` or `"s"` for singular/plural counts.
#[cfg(feature = "validate")]
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `epublift meta …` — read or edit a book's metadata.
fn run_meta(args: &MetaArgs) -> Result<()> {
    match &args.action {
        MetaAction::Show(s) => {
            let input = s
                .input
                .canonicalize()
                .with_context(|| format!("Input file not found: {}", s.input.display()))?;
            let md = epublift::read_metadata(&input)?;
            let cover = epublift::read_cover(&input)?;
            // `--json` is the older spelling of `--format metadata`.
            if s.json || s.format == MetaFormat::Metadata {
                let json = md.to_json();
                let body = json.strip_suffix("\n}").unwrap_or(&json);
                println!("{body},\n  \"cover\": {}\n}}", cover_json(cover.as_ref()));
            } else {
                print!("{}", md.to_text());
                println!("{:<14}{}", "Cover:", cover_line(cover.as_ref()));
            }
            Ok(())
        }
        MetaAction::Set(s) => run_meta_set(s),
        #[cfg(feature = "metadata")]
        MetaAction::Enrich(e) => run_meta_enrich(e),
    }
}

/// "OEBPS/cover.jpeg (image/jpeg, 600×800, 85 KB)", or "none".
fn cover_line(c: Option<&epublift::cover::CurrentCover>) -> String {
    match c {
        None => "none".to_string(),
        Some(c) => format!(
            "{} ({}, {}, {} KB)",
            c.path,
            c.media_type,
            size_text(c.width, c.height),
            c.bytes.len().div_ceil(1024)
        ),
    }
}

fn size_text(w: Option<u32>, h: Option<u32>) -> String {
    match (w, h) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => "size unknown".to_string(),
    }
}

/// The `cover` member of `meta show --format metadata`.
fn cover_json(c: Option<&epublift::cover::CurrentCover>) -> String {
    match c {
        None => "null".to_string(),
        Some(c) => serde_json::json!({
            "path": c.path,
            "media_type": c.media_type,
            "width": c.width,
            "height": c.height,
            "bytes": c.bytes.len(),
        })
        .to_string(),
    }
}

/// Read and check a `--cover` image.
fn load_cover(path: &Path) -> Result<epublift::cover::CoverImage> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    epublift::cover::CoverImage::from_bytes(bytes).with_context(|| format!("{}", path.display()))
}

/// Report a cover change after a write.
fn print_cover_change(c: &epublift::cover::CoverChange) {
    match &c.replaced {
        Some(old) => println!(
            "[+] Cover replaced: {old} -> {} ({}×{})",
            c.path, c.width, c.height
        ),
        None => println!("[+] Cover added: {} ({}×{})", c.path, c.width, c.height),
    }
    if let Some(page) = &c.cover_page_added {
        println!("    cover page added first in the reading order: {page}");
    }
    if !c.pages_updated.is_empty() {
        println!("    pages updated: {}", c.pages_updated.join(", "));
    }
    if c.kept_old_image {
        println!("    the old image was kept: a page that uses it could not be updated");
    }
}

/// Default output for a metadata edit: `<name>_meta.epub` next to the input.
fn default_meta_output(input: &Path) -> PathBuf {
    let stem = epublift::output_stem(input, false);
    input
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}_meta.epub"))
}

/// `epublift meta set …` — apply manual metadata edits.
fn run_meta_set(s: &MetaSetArgs) -> Result<()> {
    let input = s
        .input
        .canonicalize()
        .with_context(|| format!("Input file not found: {}", s.input.display()))?;

    let update = epublift::meta::MetadataUpdate {
        title: s.title.clone(),
        subtitle: s.subtitle.clone(),
        authors: (!s.authors.is_empty()).then(|| s.authors.clone()),
        language: s.language.clone(),
        publisher: s.publisher.clone(),
        date: s.date.clone(),
        description: s.description.clone(),
        subjects: (!s.subjects.is_empty()).then(|| s.subjects.clone()),
        series: s.series.as_deref().map(epublift::meta::Series::parse),
        isbn: s.isbn.clone(),
    };
    if update.is_empty() && s.cover.is_none() {
        anyhow::bail!(
            "nothing to set — pass at least one field (e.g. --title). See `epublift meta set --help`."
        );
    }
    let cover = s.cover.as_deref().map(load_cover).transpose()?;

    let output = s
        .output
        .clone()
        .unwrap_or_else(|| default_meta_output(&input));

    let written = epublift::write_metadata_and_cover(&input, &update, cover.as_ref(), &output)?;
    println!("[+] Wrote updated metadata to: {}", output.display());
    if let Some(c) = &written.cover {
        print_cover_change(c);
    }
    print!("{}", written.metadata.to_text());
    Ok(())
}

/// Default output for a repair: `<name>_repaired.epub` next to the input.
fn default_repair_output(input: &Path) -> PathBuf {
    let stem = epublift::output_stem(input, false);
    input
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}_repaired.epub"))
}

/// The most passes one `repair` makes. A pass runs again only when the last
/// one let epubveri check part of the book for the first time (`revealed`), and
/// each such pass clears the cause of the blindness it found, so in practice a
/// second pass is the end of it; the cap only bounds the worst case.
const MAX_REPAIR_PASSES: usize = 3;

/// `epublift repair …` — fix what `check` finds, with epubsana's core
/// `repair()`: the function its own CLI and its browser build run, so the same
/// book with the same approvals comes back identical from all three. Exits like
/// `check`: 0 goal met, 1 defects remain, 2 the book could not be repaired.
fn run_repair(args: &RepairArgs) -> Result<()> {
    use std::io::Write;
    match repair_book(args) {
        Ok(true) => Ok(()),
        // A verdict, not a failure: exit directly so it does not print as one.
        Ok(false) => {
            std::io::stdout().flush().ok();
            std::process::exit(1)
        }
        Err(e) => {
            eprintln!("\n[!] Fatal Error: {e:#}");
            std::process::exit(2)
        }
    }
}

/// Repair one book and report it; `Ok(goal met)`.
fn repair_book(args: &RepairArgs) -> Result<bool> {
    use epubsana::{Confirmer, Policy, Workspace};
    use std::io::IsTerminal;

    let input = args
        .input
        .canonicalize()
        .with_context(|| format!("Input file not found: {}", args.input.display()))?;
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| default_repair_output(&input));
    if same_file(&input, &output) {
        anyhow::bail!(
            "the output path is the input ({}); repair never modifies the original, \
             so choose a different --output",
            output.display()
        );
    }
    let bytes =
        std::fs::read(&input).with_context(|| format!("could not read {}", input.display()))?;
    let mut ws = Workspace::load(&bytes)
        .map_err(|e| anyhow::anyhow!("could not open {} as an EPUB: {e}", input.display()))?;

    // Safe fixes are applied without asking; a fix that needs a decision is
    // asked about in a terminal, skipped without one, approved under --yes.
    let interactive = !args.yes && !args.dry_run && std::io::stdin().is_terminal();
    let (policy, mut confirmer): (Policy, Box<dyn Confirmer>) = if args.dry_run {
        (Policy::DryRun, Box::new(DeclineFix))
    } else if args.yes {
        (Policy::AskEach, Box::new(ApproveFix))
    } else if interactive {
        (Policy::AutoSafeThenAsk, Box::new(AskFix))
    } else {
        (Policy::AutoSafeThenAsk, Box::new(DeclineFix))
    };

    // A fix that lets epubveri into part of the book for the first time (an
    // unrecognised package version corrected, a fatal cleared) reveals findings
    // that were always there but were not in the plan. Planning again on the
    // result is what epubsana says to do with them, so we do it here rather
    // than tell the user to run us twice. A dry run changes nothing, so it
    // never has a second pass to plan.
    let mut passes = Vec::new();
    loop {
        let pass = epubsana::repair(&mut ws, args.goal.into(), policy, confirmer.as_mut())
            .map_err(|e| anyhow::anyhow!("could not repair {}: {e}", input.display()))?;
        let again = pass.revealed > 0 && pass.changed();
        passes.push(pass);
        if !again || passes.len() == MAX_REPAIR_PASSES {
            break;
        }
    }
    let report = merge_passes(&passes);

    // Write only when a fix was applied: a run that changed nothing leaves no
    // file behind. A dry run names the file it would write, if any.
    let written = if args.dry_run {
        (!report.fixes.is_empty()).then(|| output.clone())
    } else if report.changed() {
        let repaired = ws
            .serialize()
            .map_err(|e| anyhow::anyhow!("could not write the repaired EPUB: {e}"))?;
        std::fs::write(&output, repaired)
            .with_context(|| format!("could not write {}", output.display()))?;
        Some(output.clone())
    } else {
        None
    };

    if args.format == ReportFormat::Json {
        let input = epubsana::envelope::input(
            args.input.display().to_string(),
            written.as_ref().map(|p| p.display().to_string()),
            &report,
        );
        let mut envelope = epubsana::envelope::Envelope::for_tool(
            "epublift",
            env!("CARGO_PKG_VERSION"),
            ENVELOPE_CONVENTION,
            None,
            vec![input],
        );
        envelope.dry_run = args.dry_run;
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        print_repair_report(&passes, &report, written.as_deref(), args, interactive);
    }
    Ok(report.goal_met)
}

/// Several passes as one run: every pass's fixes in order, the counts the book
/// started from and the counts it ended with, and the last pass's verdict.
fn merge_passes(passes: &[epubsana::ChangeReport]) -> epubsana::ChangeReport {
    let (first, last) = (&passes[0], &passes[passes.len() - 1]);
    epubsana::ChangeReport {
        fixes: passes.iter().flat_map(|p| p.fixes.clone()).collect(),
        fatals_after: last.fatals_after,
        errors_after: last.errors_after,
        warnings_after: last.warnings_after,
        infos_after: last.infos_after,
        usages_after: last.usages_after,
        revealed: last.revealed,
        goal: last.goal,
        goal_met: last.goal_met,
        ..first.clone()
    }
}

/// Whether `output` names the same file as `input` (already canonical), even
/// when `output` does not exist yet.
fn same_file(input: &Path, output: &Path) -> bool {
    if let Ok(o) = output.canonicalize() {
        return o == input;
    }
    let parent = match output.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    match (parent.canonicalize(), output.file_name()) {
        (Ok(p), Some(name)) => p.join(name) == input,
        _ => false,
    }
}

/// `--yes`: approve every fix.
struct ApproveFix;
impl epubsana::Confirmer for ApproveFix {
    fn decide(&mut self, _fix: &epubsana::ProposedFix) -> epubsana::Decision {
        epubsana::Decision::Approve
    }
}

/// No terminal to ask in (or a dry run): decline every fix that needs a decision.
struct DeclineFix;
impl epubsana::Confirmer for DeclineFix {
    fn decide(&mut self, _fix: &epubsana::ProposedFix) -> epubsana::Decision {
        epubsana::Decision::Reject
    }
}

/// Ask on the terminal about each fix that needs a decision. The question goes
/// to stderr so that stdout carries only the report (or the one JSON object).
struct AskFix;
impl epubsana::Confirmer for AskFix {
    fn decide(&mut self, fix: &epubsana::ProposedFix) -> epubsana::Decision {
        use std::io::Write;
        eprintln!("\n[?] {}", fix.title);
        eprintln!("    why: {}", fix.rationale);
        for c in &fix.preview {
            eprintln!("    - {}", c.note);
        }
        eprint!("    Apply this fix? [y/N] ");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("y") {
            epubsana::Decision::Approve
        } else {
            epubsana::Decision::Reject
        }
    }
}

/// "2 fatal errors, 5 errors, 1 warning" — fatals named first and always,
/// because a fatal-only book has no errors and is not remotely valid.
fn severity_counts(fatals: usize, errors: usize, warnings: usize) -> String {
    let n =
        |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    format!(
        "{}, {}, {}",
        n(fatals, "fatal error"),
        n(errors, "error"),
        n(warnings, "warning")
    )
}

/// Print a human-readable report of a repair run.
fn print_repair_report(
    passes: &[epubsana::ChangeReport],
    report: &epubsana::ChangeReport,
    written: Option<&Path>,
    args: &RepairArgs,
    interactive: bool,
) {
    use epubsana::Outcome;

    let goal = report.goal.as_str();
    if report.fixes.is_empty() {
        if report.goal_met {
            println!("[=] Nothing to repair: this EPUB already meets the goal '{goal}'.");
        } else {
            println!(
                "[=] Nothing epublift can repair safely: {}, and none has a fix that \
                 is certain to be right. No file was written; `epublift check` lists them.",
                severity_counts(
                    report.fatals_before,
                    report.errors_before,
                    report.warnings_before
                )
            );
        }
        return;
    }

    let mut index = 0;
    for (n, pass) in passes.iter().enumerate() {
        if n > 0 {
            println!(
                "\n  Pass {}: the fixes above let epubveri check part of the book it could \
                 not check before, and it found {} more finding(s) that were always there.",
                n + 1,
                passes[n - 1].revealed
            );
        }
        for f in &pass.fixes {
            index += 1;
            let outcome = match f.outcome {
                Outcome::Applied => "fixed",
                Outcome::Skipped => "skipped",
                Outcome::Proposed => "would fix",
                Outcome::Reverted => "undone",
            };
            let decision = match f.tier {
                epubsana::Tier::ConfirmNeeded => " (needs a decision)",
                epubsana::Tier::AutoSafe => "",
            };
            println!("  [{index}] {outcome}{decision}: {}", f.title);
            if let Some((id, rule)) = f.reverted_for {
                let rule = rule.map(|r| format!(" ({r})")).unwrap_or_default();
                println!("      undone because it added a {id}{rule} finding");
            }
            // A skipped fix changed nothing, so its title is all there is to say.
            if f.outcome != Outcome::Skipped {
                for c in &f.changes {
                    println!("      - {}", c.note);
                }
            }
        }
    }

    println!(
        "\n  Before: {}\n  After:  {}",
        severity_counts(
            report.fatals_before,
            report.errors_before,
            report.warnings_before
        ),
        severity_counts(
            report.fatals_after,
            report.errors_after,
            report.warnings_after
        ),
    );

    let skipped = report.skipped().count();
    if skipped > 0 && !interactive && !args.yes && !args.dry_run {
        println!(
            "\n[i] {skipped} fix(es) need a decision and were skipped, because there is no \
             terminal to ask in. Run in a terminal to be asked, or pass --yes to apply them."
        );
    }
    if report.revealed > 0 {
        println!(
            "\n[i] The last pass let epubveri check more of the book and it found {} more \
             finding(s). Run `epublift repair` again on the output to repair the ones it can.",
            report.revealed
        );
    }

    match written {
        Some(out) if args.dry_run => println!("\n[=] Dry run: would write {}", out.display()),
        Some(out) => println!("\n[+] Wrote repaired EPUB to: {}", out.display()),
        None if args.dry_run => println!("\n[=] Dry run: nothing would be written."),
        None => println!("\n[=] No fix was applied, so no file was written."),
    }
    if report.goal_met {
        println!("[+] Goal '{goal}' met.");
    } else {
        let book = if written.is_some() && !args.dry_run {
            "the output"
        } else {
            "the book"
        };
        println!(
            "[!] Goal '{goal}' not met: what remains needs a person to decide. \
             `epublift check` on {book} lists it."
        );
    }
}

/// `epublift meta enrich …` — fill missing metadata from an online catalogue.
#[cfg(feature = "metadata")]
fn run_meta_enrich(e: &MetaEnrichArgs) -> Result<()> {
    use epublift::enrich::{self, EnrichOptions};

    let input = e
        .input
        .canonicalize()
        .with_context(|| format!("Input file not found: {}", e.input.display()))?;
    let existing = epublift::read_metadata(&input)?;

    let opts = EnrichOptions {
        lang_override: e.lang.clone(),
        overwrite: e.overwrite,
        allow_foreign_meta: e.allow_foreign_meta,
        include_description: e.include_description,
        include_subjects: e.include_subjects,
    };

    let provider_label = match e.provider.as_str() {
        "google" | "googlebooks" | "google-books" => "Google Books",
        _ => "Open Library",
    };
    println!("[*] Looking up ISBN {} on {provider_label}…", e.isbn);
    let http = epublift::http::RustlsHttp::new()?;
    let fetched = enrich::fetch_isbn(&e.provider, &e.isbn, &http, opts.include_description)?;
    let plan = enrich::plan_enrich(&existing, &fetched, &opts)?;

    print!("{}", plan.to_text());
    if !e.include_subjects && !fetched.subjects.is_empty() {
        println!(
            "[i] {provider_label} lists {} subject(s), from many libraries and not all in the \
             book's language: {}. Pass --include-subjects to add them.",
            fetched.subjects.len(),
            fetched.subjects.join("; ")
        );
    }

    // The catalogue's cover: shown next to the book's own, applied only on
    // --cover (often it is smaller than the one the book has).
    let mut cover = None;
    match &fetched.cover_url {
        None => println!("[i] {provider_label} has no cover for this ISBN."),
        Some(url) => match enrich::fetch_cover(url, &http) {
            Err(err) => println!("[!] {provider_label}'s cover could not be used: {err:#}"),
            Ok(c) => {
                let current = epublift::read_cover(&input)?;
                let mine = current.as_ref().map(|c| size_text(c.width, c.height));
                println!(
                    "[i] {provider_label} has a cover: {}×{} (the book's: {}).",
                    c.width,
                    c.height,
                    mine.as_deref().unwrap_or("none")
                );
                if current.as_ref().is_some_and(|cur| {
                    cur.width.zip(cur.height).is_some_and(|(w, h)| {
                        u64::from(w) * u64::from(h) > u64::from(c.width) * u64::from(c.height)
                    })
                }) {
                    println!("    It is smaller than the book's own cover.");
                }
                if e.cover {
                    cover = Some(c);
                } else {
                    println!("    Pass --cover to make it the book's cover.");
                }
            }
        },
    }

    if e.dry_run {
        println!("[i] Dry run — no changes written.");
        return Ok(());
    }
    if plan.update.is_empty() && cover.is_none() {
        return Ok(());
    }

    let output = e
        .output
        .clone()
        .unwrap_or_else(|| default_meta_output(&input));
    let written =
        epublift::write_metadata_and_cover(&input, &plan.update, cover.as_ref(), &output)?;
    println!("[+] Wrote enriched metadata to: {}", output.display());
    if let Some(c) = &written.cover {
        print_cover_change(c);
    }
    print!("{}", written.metadata.to_text());
    Ok(())
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

    // Target version + image format policy. 3.4 (AVIF/JXL) is experimental and
    // only available under the `epub34` feature; the default build is 3.3/WebP.
    #[cfg(feature = "epub34")]
    let image_policy = match args.image_format.as_deref() {
        None => None,
        Some("avif") => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::Avif)),
        Some("jxl") => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::Jxl)),
        Some("best") => Some(epublift::FormatPolicy::Best),
        Some(other) => {
            anyhow::bail!("unknown --image-format '{other}'. Supported: avif, jxl, best.")
        }
    };
    // avif/jxl/best are 3.4 formats, so an explicit `--image-format` implies 3.4.
    #[cfg(feature = "epub34")]
    let target_version = if image_policy.is_some() {
        EpubVersion::V3_4
    } else {
        match args.target.trim() {
            "3.3" => EpubVersion::V3_3,
            "3.4" => EpubVersion::V3_4,
            other => anyhow::bail!("unknown --target '{other}'. Supported: 3.3, 3.4."),
        }
    };
    #[cfg(not(feature = "epub34"))]
    let target_version = EpubVersion::LATEST;
    #[cfg(not(feature = "epub34"))]
    let image_policy = None;

    let mut options = Options {
        quality: args.quality.clamp(1, 100) as u8,
        ascii: args.ascii,
        target_version,
        image_strategy: if args.keep_images {
            ImageStrategy::KeepOriginal
        } else {
            ImageStrategy::WebP
        },
        image_policy,
        avif_speed: 4,
        kepub: args.kepub,
        kepub_webp: args.kepub_webp,
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
            "[+] {} -> {} ({}; {} compressed + {} stored)",
            file_name(epub),
            file_name(&out),
            size_change(stats.percent_saved()),
            stats.compressed_entries,
            stats.stored_entries,
        );
    }
    if epubs.len() > 1 {
        println!(
            "[=] {} books: {:.2} MB -> {:.2} MB ({})",
            epubs.len(),
            total_in as f64 / 1024.0 / 1024.0,
            total_out as f64 / 1024.0 / 1024.0,
            size_change(saved_pct(total_in, total_out)),
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

    // Image format policy for a 3.4 re-target (avif/jxl/best implies 3.4).
    #[cfg(feature = "epub34")]
    let image_policy = match args.image_format.as_deref() {
        None => None,
        Some("avif") => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::Avif)),
        Some("jxl") => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::Jxl)),
        Some("best") => Some(epublift::FormatPolicy::Best),
        Some(other) => {
            anyhow::bail!("unknown --image-format '{other}'. Supported: avif, jxl, best.")
        }
    };
    #[cfg(not(feature = "epub34"))]
    let image_policy: Option<epublift::FormatPolicy> = None;

    // A re-target is requested when any transform flag is present; otherwise the
    // restore is content-exact (the original book, byte-for-byte).
    let retarget = args.target.is_some()
        || args.modernize
        || args.keep_images
        || args.kepub
        || image_policy.is_some();
    let target_version = if image_policy.is_some() {
        EpubVersion::V3_4
    } else {
        match &args.target {
            Some(t) => parse_target(t)?,
            None => EpubVersion::LATEST,
        }
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
            image_policy,
            avif_speed: 4,
            kepub: args.kepub,
            kepub_webp: args.kepub_webp,
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
            retarget_label(args, target_version),
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
        #[cfg(feature = "epub34")]
        "3.4" => Ok(EpubVersion::V3_4),
        #[cfg(not(feature = "epub34"))]
        "3.4" => {
            bail!("EPUB 3.4 (AVIF/JXL) needs the experimental `epub34` build feature.")
        }
        "2" | "2.0" => {
            bail!("downgrading to EPUB 2.0 isn't supported — epublift is an upgrader.")
        }
        other => bail!("unknown --target '{other}'. Supported: 3.3, 3.4."),
    }
}

/// A short human label of the re-target for the restore output line.
#[cfg(feature = "archival")]
fn retarget_label(args: &RestoreArgs, target_version: EpubVersion) -> String {
    let mut parts = Vec::new();
    if args.kepub {
        parts.push("kepub".to_string());
    } else {
        parts.push(format!("EPUB {}", target_version.tag()));
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

/// "2.0% smaller" or, when the output grew, "2.0% larger" — never a negative
/// "smaller".
#[cfg(feature = "archival")]
fn size_change(saved_pct: f64) -> String {
    if saved_pct < 0.0 {
        format!("{:.1}% larger", -saved_pct)
    } else {
        format!("{saved_pct:.1}% smaller")
    }
}

#[cfg(feature = "archival")]
fn saved_pct(input: u64, output: u64) -> f64 {
    if input > 0 {
        (1.0 - output as f64 / input as f64) * 100.0
    } else {
        0.0
    }
}
