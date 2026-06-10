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

mod images;
mod nav;
mod opf;
mod report;
mod util;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::Parser;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use opf::RewriteParams;

/// Optimize EPUB structure to 3.3 and convert images to WebP.
#[derive(Parser, Debug)]
#[command(
    name = "epublift",
    about = "Optimize EPUB structure to 3.3 and convert images to WebP.",
    after_help = "Example Usage:\n  epublift -i book.epub -q 75\n  epublift --input book.epub --output optimized.epub --quality 80 --report results.txt"
)]
struct Args {
    /// Path to original EPUB file to lift
    #[arg(short, long)]
    input: PathBuf,

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
    let input_path = args
        .input
        .canonicalize()
        .with_context(|| format!("Input file not found: {}", args.input.display()))?;

    let raw_stem = input_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    let stem = if args.ascii {
        util::slugify_ascii(&raw_stem)
    } else {
        raw_stem
    };
    let parent = input_path.parent().unwrap_or_else(|| Path::new("."));

    let output_path = match args.output {
        Some(p) => p,
        None => parent.join(format!("{}_v3.3.epub", stem)),
    };
    let report_path = match args.report {
        Some(p) => p,
        None => parent.join(format!("{}_report.txt", stem)),
    };

    let quality = args.quality.clamp(1, 100) as u8;
    let original_size = fs::metadata(&input_path)?.len();

    let input_name = input_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    println!("[*] Starting optimization for: {}", input_name);
    println!("[*] Target output path: {}", output_path.display());
    println!("[*] WebP Image Quality: {}%", quality);

    let temp_dir = tempfile::Builder::new().prefix("epublift_").tempdir()?;
    let temp_path = temp_dir.path();

    // Step 1: Extract EPUB.
    println!("[*] Extracting original EPUB file...");
    extract_epub(&input_path, temp_path)?;

    // Step 2: Locate the OPF package document.
    let opf_path = locate_opf(temp_path)?;
    println!(
        "[+] Located package document (OPF): {}",
        opf_path
            .strip_prefix(temp_path)
            .unwrap_or(&opf_path)
            .display()
    );
    let package_dir = opf_path.parent().unwrap_or(temp_path).to_path_buf();

    let opf_xml = fs::read_to_string(&opf_path).context("Failed to read OPF package document")?;
    let info = opf::parse_opf_info(&opf_xml)?;

    // Step 3: Optimize images.
    println!("[*] Converting and compressing images to WebP...");
    let opt =
        images::optimize_images(&package_dir, &info.items, info.cover_id.as_deref(), quality)?;
    images::update_document_references(temp_path, &opt.ref_pairs);

    // Step 4: Upgrade structure to EPUB 3.3.
    println!("[*] Upgrading structure to EPUB 3.3 compliance...");

    // Decide whether we must generate a navigation document from toc.ncx.
    let mut add_nav = false;
    if !info.nav_exists
        && let Some(ncx_href) = &info.ncx_href
    {
        let ncx_path = package_dir.join(util::unquote(ncx_href));
        if ncx_path.exists() {
            println!("[+] Creating mandatory EPUB 3 Navigation Document from toc.ncx...");
            match nav::generate_nav_xhtml(
                &ncx_path,
                &package_dir.join("nav.xhtml"),
                &info.guide_refs,
            ) {
                Ok(()) => {
                    add_nav = true;
                    println!(
                        "  [+] Registered nav.xhtml with properties='nav' in package document."
                    );
                }
                Err(e) => println!("  [!] Failed to generate Navigation Document: {}", e),
            }
        }
    }

    if info.has_guide {
        println!("  [+] Replaced legacy <guide> element with HTML5 landmarks navigation.");
    }

    // Standardize content DOCTYPEs and namespaces.
    util::standardize_xhtml_files(temp_path)?;

    // Rewrite the OPF with all upgrades and write it back.
    let params = RewriteParams {
        modified_ts: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        manifest_changes: opt.manifest_changes.into_iter().collect::<HashMap<_, _>>(),
        add_nav,
        remove_guide: info.has_guide,
    };
    let new_opf = opf::rewrite_opf(&opf_xml, &params)?;
    fs::write(&opf_path, new_opf)?;

    // Step 5: Repackage EPUB.
    println!("[*] Repackaging folder into EPUB file...");
    repackage_epub(temp_path, &output_path)?;

    // Step 6: Generate report.
    let final_size = fs::metadata(&output_path)?.len();
    let output_name = output_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    report::write_report(
        &report_path,
        &input_name,
        &output_name,
        original_size,
        final_size,
        &opt.metrics,
    )?;

    println!("\n[+] Optimization complete!");
    println!("[+] Output EPUB: {}", output_path.display());
    println!("[+] Report file: {}", report_path.display());

    let saved = original_size as i64 - final_size as i64;
    let pct = if original_size > 0 {
        saved as f64 / original_size as f64 * 100.0
    } else {
        0.0
    };
    println!(
        "[+] Size reduced from {:.2} MB to {:.2} MB ({:.1}% savings)",
        original_size as f64 / 1024.0 / 1024.0,
        final_size as f64 / 1024.0 / 1024.0,
        pct
    );

    Ok(())
}

/// Extract every entry of the EPUB zip into `dest`.
fn extract_epub(input: &Path, dest: &Path) -> Result<()> {
    let file = File::open(input)?;
    let mut zip = ZipArchive::new(file)?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let out_path = match entry.enclosed_name() {
            Some(p) => dest.join(p),
            None => continue, // skip unsafe (zip-slip) paths
        };

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&out_path)?;
            io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}

/// Read `META-INF/container.xml` to find the OPF package document path.
fn locate_opf(temp_dir: &Path) -> Result<PathBuf> {
    let container_path = temp_dir.join("META-INF").join("container.xml");
    if !container_path.exists() {
        bail!("Invalid EPUB: META-INF/container.xml is missing.");
    }

    let xml = fs::read_to_string(&container_path)?;
    let doc = roxmltree::Document::parse(&xml).context("Failed to parse container.xml")?;

    let rootfile = doc
        .descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "rootfile"
                && n.attribute("full-path").is_some()
        })
        .context("Could not find rootfile element in container.xml")?;

    let full_path = rootfile.attribute("full-path").unwrap();
    Ok(temp_dir.join(full_path))
}

/// Repackage the working directory into a valid EPUB zip. `mimetype` is written
/// first and stored uncompressed; everything else is deflated.
fn repackage_epub(temp_dir: &Path, output: &Path) -> Result<()> {
    let file = File::create(output)?;
    let mut zip = ZipWriter::new(file);

    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // 1. mimetype first, uncompressed.
    let mimetype_path = temp_dir.join("mimetype");
    zip.start_file("mimetype", stored)?;
    if mimetype_path.exists() {
        let data = fs::read(&mimetype_path)?;
        io::Write::write_all(&mut zip, &data)?;
    } else {
        io::Write::write_all(&mut zip, b"application/epub+zip")?;
    }

    // 2. Everything else, deflated.
    for entry in WalkDir::new(temp_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(temp_dir)?;
        let arcname = rel.to_string_lossy().replace('\\', "/");
        if arcname == "mimetype" {
            continue;
        }
        zip.start_file(arcname, deflated)?;
        let data = fs::read(path)?;
        io::Write::write_all(&mut zip, &data)?;
    }

    zip.finish()?;
    Ok(())
}
