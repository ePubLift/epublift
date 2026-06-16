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
use epublift::{EpubVersion, Options};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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

    /// Produce a Kobo .kepub.epub: inject koboSpan markup for Kobo's reading
    /// features. Composes with the normal upgrades; output is named
    /// "<name>.kepub.epub" unless -o is given.
    #[arg(long)]
    kepub: bool,
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
    let input = args
        .input
        .canonicalize()
        .with_context(|| format!("Input file not found: {}", args.input.display()))?;

    let mut options = Options {
        quality: args.quality.clamp(1, 100) as u8,
        ascii: args.ascii,
        target_version: EpubVersion::LATEST,
        kepub: args.kepub,
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
