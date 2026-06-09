# 🚀 epublift — EPUB 3.3 Upgrader & WebP Optimizer (Rust Edition)

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Rust](https://img.shields.io/badge/rust-1.94+-orange.svg)](https://www.rust-lang.org/)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg?style=flat-square)](http://makeapullrequest.com)

A fast, standard-compliant command-line utility written in **Rust** to optimize, modernize, and significantly shrink EPUB files. It seamlessly upgrades legacy EPUB structures (EPUB 2.0) to the modern **EPUB 3.3** specification while replacing heavy raster images (JPEG/PNG) with highly-compressed, modern **WebP** formats.

This is a Rust port of the original Python implementation (`epublift-py`), preserving identical behavior and the same AGPL-3.0 license.

---

## ✨ Key Features

*   **🔒 Workspace Safety**: Extracts and processes files inside a system-managed secure temporary directory. The original file remains completely untouched unless the entire operations pipeline completes successfully.
*   **📸 WebP Image Optimization**:
    *   Automatically converts heavy JPEG and PNG images to WebP format.
    *   Preserves PNG alpha channel transparency.
    *   Allows customizable quality level settings (1–100).
    *   Automatically scans and updates all image references in CSS, XHTML/HTML files, SVG graphics, and the OPF manifest.
*   **⚡ EPUB 3.3 Compliance Upgrade**:
    *   Upgrades package declarations in the OPF metadata to version `3.0`.
    *   Injects required `dcterms:modified` UTC metadata timestamps.
    *   Parses legacy `toc.ncx` maps and generates a standard **EPUB 3 Navigation Document (`nav.xhtml`)** with clean nested elements.
    *   Converts outdated `<guide>` landmark reference lists into HTML5 `<nav epub:type="landmarks">` maps.
    *   Standardizes legacy XHTML DOCTYPEs (like XHTML 1.1) to modern HTML5 `<!DOCTYPE html>` structure.
*   **📊 Detailed Audit Reports**: Generates a detailed size comparison table and conversion metrics report in an easy-to-read text file.

---

## 🛠️ Technical Design & Pipeline

```mermaid
graph TD
    A[Input EPUB Archive] -->|Secure Extract| B[System Temp Directory]
    B --> C[Locate & Parse content.opf]
    C --> D[Identify Manifest Images]
    D -->|Convert JPEG/PNG to WebP| E[Compress & Optimize Images]
    E -->|Href Remapping| F[Update Links in XHTML, CSS, SVGs & Manifest]
    F -->|Structure Modernization| G[Upgrade OPF version to 3.0]
    G --> H[Inject dc/dcterms namespaces & UTC modified time]
    H --> I[Parse toc.ncx and generate nav.xhtml]
    I --> J[Clean & Standardize DOCTYPEs to HTML5]
    J -->|Repackage ZIP| K[Store mimetype Uncompressed & Deflate others]
    K --> L[Generate Clean EPUB 3.3 Output]
    L --> M[Write Performance & Compliance Audit Report]
```

### 📱 E-Reader Compatibility
To ensure broad compatibility, epublift retains legacy `toc.ncx` maps and OPF pointers alongside the newly-generated EPUB 3.3 `nav.xhtml` navigation document. This creates a fully **backward-compatible** hybrid document that runs smoothly on vintage EPUB 2 devices while delivering high-speed modern features and layout compliance on new EPUB 3.3 devices.

---

## 📥 Installation

This utility requires the **Rust toolchain** (1.94+) and a C compiler (used to build the bundled `libwebp` encoder).

### Build from source

```bash
# Build an optimized release binary
cargo build --release

# The binary is produced at:
#   target/release/epublift
```

You can optionally install it onto your `PATH`:

```bash
cargo install --path .
```

---

## 🚀 How to Use

### Basic Command

```bash
epublift -i <path_to_input_epub>
```
*This command modernizes the input file and saves it in the same directory as `<input_name>_v3.3.epub`, generating a performance report in `<input_name>_report.txt`.*

During development you can also run it directly with Cargo:

```bash
cargo run --release -- -i book.epub
```

### Advanced Options

```bash
epublift -i book.epub -o optimized_book.epub -q 85 -r stats_report.txt
```

### Command Line Interface Options

| Argument | Long Flag | Description | Default |
| :--- | :--- | :--- | :--- |
| `-i` | `--input` | **[Required]** Path to the original EPUB file | *None* |
| `-o` | `--output` | Path to save the modernized EPUB | `<input>_v3.3.epub` |
| `-q` | `--quality`| WebP compression quality level (1-100) | `80` |
| `-r` | `--report` | Path to write the conversion audit report | `<input>_report.txt` |

---

## 🧪 Quick Sandbox Testing

A companion binary (`gen-sample`) builds a valid legacy EPUB 2.0 file containing test images and outdated structures, so you can safely evaluate the tool.

### Step 1: Generate the Sample EPUB 2.0 File
```bash
cargo run --release --bin gen-sample
```
*This creates a new legacy file named `sample_epub2.epub` in your current folder.*

### Step 2: Run epublift
```bash
cargo run --release --bin epublift -- -i sample_epub2.epub
```
*This converts the book, modernizes the structure to EPUB 3.3, and produces `sample_epub2_v3.3.epub` along with `sample_epub2_report.txt`.*

### Step 3: Inspect the Output Audit Report
```bash
cat sample_epub2_report.txt
```

---

## 📄 License & Sharing

This project is licensed under the **GNU Affero General Public License, Version 3 (AGPL-3.0)**.

### Why AGPL-3.0?
We believe in open source. By sharing this software under the AGPL license, we ensure that:
1. Anyone is free to use, modify, and distribute this tool.
2. If you modify this tool and run it as part of an online service (e.g. an e-book conversion website), you **must** make your modified source code available to users of that service.

For full terms and conditions, please consult the [LICENSE](LICENSE) file in the root of this repository.
