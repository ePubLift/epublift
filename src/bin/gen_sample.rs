// gen-sample - Build a legacy EPUB 2.0 file for testing epublift.
//
// Part of epublift. Licensed under the GNU AGPL-3.0-or-later; see LICENSE.
//
// This mirrors the original `test_epub_generator.py`. Unlike the Python
// version it does not draw text onto the images (to avoid a font dependency);
// it generates solid/shape-based raster images, which serve the same purpose
// of exercising the WebP conversion pipeline.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const SRC: &str = "temp_epub_src";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    create_dummy_images()?;
    create_text_files()?;
    package_epub("sample_epub2.epub")?;
    Ok(())
}

fn create_dummy_images() -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(format!("{SRC}/OEBPS/images"))?;

    // cover.jpg: solid dark-blue 600x800.
    let mut cover = image::RgbImage::new(600, 800);
    for px in cover.pixels_mut() {
        *px = image::Rgb([0x2c, 0x3e, 0x50]);
    }
    cover.save(format!("{SRC}/OEBPS/images/cover.jpg"))?;

    // logo.png: transparent 200x200 with a red filled circle.
    let mut logo = image::RgbaImage::new(200, 200);
    let (cx, cy, r) = (100.0_f32, 100.0_f32, 80.0_f32);
    for (x, y, px) in logo.enumerate_pixels_mut() {
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        if dx * dx + dy * dy <= r * r {
            *px = image::Rgba([0xe7, 0x4c, 0x3c, 0xff]);
        } else {
            *px = image::Rgba([0, 0, 0, 0]);
        }
    }
    logo.save(format!("{SRC}/OEBPS/images/logo.png"))?;

    Ok(())
}

fn write_file(rel: &str, contents: &str) -> std::io::Result<()> {
    let path = format!("{SRC}/{rel}");
    if let Some(parent) = Path::new(&path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

fn create_text_files() -> std::io::Result<()> {
    write_file("mimetype", "application/epub+zip")?;

    write_file(
        "META-INF/container.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
    )?;

    write_file(
        "OEBPS/styles.css",
        r#"body {
    font-family: sans-serif;
    margin: 1em;
    color: #333333;
}
.logo-container {
    text-align: center;
    background-image: url('images/logo.png');
    background-repeat: no-repeat;
    height: 200px;
    width: 200px;
    margin: auto;
}
"#,
    )?;

    write_file(
        "OEBPS/chapter1.html",
        r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
    <title>Chapter 1: The Beginning</title>
    <link rel="stylesheet" type="text/css" href="styles.css"/>
</head>
<body>
    <h1>Chapter 1: The Beginning</h1>
    <p>This is a paragraph with a cover image below.</p>
    <p><img src="images/cover.jpg" alt="Cover Image" style="max-width: 100%;"/></p>
</body>
</html>"#,
    )?;

    write_file(
        "OEBPS/chapter2.html",
        r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
    <title>Chapter 2: The Next Step</title>
    <link rel="stylesheet" type="text/css" href="styles.css"/>
</head>
<body>
    <h1>Chapter 2: The Next Step</h1>
    <p>This is chapter 2. It references the logo in CSS and also inline:</p>
    <div class="logo-container"></div>
    <p><img src="images/logo.png" alt="Logo" /></p>
</body>
</html>"#,
    )?;

    write_file(
        "OEBPS/toc.ncx",
        r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd">
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="urn:uuid:12345678-1234-5678-1234-567812345678"/>
    <meta name="dtb:depth" content="1"/>
    <meta name="dtb:totalPageCount" content="0"/>
    <meta name="dtb:maxPageNumber" content="0"/>
  </head>
  <docTitle>
    <text>Test EPUB 2 Book</text>
  </docTitle>
  <navMap>
    <navPoint id="navPoint-1" playOrder="1">
      <navLabel>
        <text>Chapter 1: The Beginning</text>
      </navLabel>
      <content src="chapter1.html"/>
    </navPoint>
    <navPoint id="navPoint-2" playOrder="2">
      <navLabel>
        <text>Chapter 2: The Next Step</text>
      </navLabel>
      <content src="chapter2.html"/>
    </navPoint>
  </navMap>
</ncx>"#,
    )?;

    write_file(
        "OEBPS/content.opf",
        r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="BookId" opf:scheme="UUID">urn:uuid:12345678-1234-5678-1234-567812345678</dc:identifier>
    <dc:title>Test EPUB 2 Book</dc:title>
    <dc:creator opf:role="aut">Jane Doe</dc:creator>
    <dc:language>en</dc:language>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="style" href="styles.css" media-type="text/css"/>
    <item id="cover-image" href="images/cover.jpg" media-type="image/jpeg"/>
    <item id="logo-image" href="images/logo.png" media-type="image/png"/>
    <item id="chapter1" href="chapter1.html" media-type="text/html"/>
    <item id="chapter2" href="chapter2.html" media-type="text/html"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="chapter1"/>
    <itemref idref="chapter2"/>
  </spine>
  <guide>
    <reference type="cover" title="Cover Page" href="chapter1.html"/>
  </guide>
</package>"#,
    )?;

    Ok(())
}

fn package_epub(output: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(output)?;
    let mut zip = ZipWriter::new(file);

    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored)?;
    zip.write_all(b"application/epub+zip")?;

    for entry in walkdir::WalkDir::new(SRC)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(SRC)?;
        let arcname = rel.to_string_lossy().replace('\\', "/");
        if arcname == "mimetype" {
            continue;
        }
        zip.start_file(arcname, deflated)?;
        zip.write_all(&fs::read(path)?)?;
    }

    zip.finish()?;
    fs::remove_dir_all(SRC)?;
    println!("Sample EPUB file created successfully: {output}");
    Ok(())
}
