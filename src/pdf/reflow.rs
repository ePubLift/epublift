//! Render structured PDF chapters (`Block`s) to XHTML, then hand off to the
//! shared `epub_writer` for OCF/OPF/nav packaging.

use std::path::Path;

use anyhow::Result;

use crate::epub_writer::{ImageAsset, RenderedChapter, esc, package_epub};

use super::structure::{Block, Chapter};

/// Write `chapters` as a reflow EPUB to `out`.
pub(crate) fn write_epub(
    out: &Path,
    title: &str,
    language: &str,
    chapters: &[Chapter],
) -> Result<()> {
    let mut images: Vec<ImageAsset> = Vec::new();
    let mut rendered: Vec<RenderedChapter> = Vec::new();

    for (i, ch) in chapters.iter().enumerate() {
        let heading = ch
            .title
            .clone()
            .unwrap_or_else(|| format!("Section {}", i + 1));

        let mut body = format!("<h1>{}</h1>\n", esc(&heading));
        for block in &ch.blocks {
            match block {
                Block::Paragraph(p) => body.push_str(&format!("<p>{}</p>\n", esc(p))),
                Block::Figure(fig) => {
                    let name = format!("fig{:03}.{}", images.len() + 1, fig.ext);
                    body.push_str(&format!(
                        "<div class=\"figure\"><img src=\"images/{name}\" alt=\"\"/></div>\n"
                    ));
                    images.push(ImageAsset {
                        name,
                        media_type: fig.media_type.to_string(),
                        data: fig.data.clone(),
                    });
                }
            }
        }
        rendered.push(RenderedChapter {
            title: heading,
            body,
        });
    }

    package_epub(out, title, language, &rendered, &images, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_epub() {
        let chapters = vec![Chapter {
            title: Some("Chapter <1>".to_string()),
            blocks: vec![Block::Paragraph("Hello & welcome.".to_string())],
        }];
        let out =
            std::env::temp_dir().join(format!("epublift_reflow_test_{}.epub", std::process::id()));
        write_epub(&out, "My Title", "en", &chapters).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_file(&out);

        assert_eq!(&bytes[..2], b"PK", "not a zip");
        // OCF requires the first entry to be an uncompressed `mimetype` whose
        // bytes therefore appear verbatim near the start of the archive.
        assert!(
            bytes.windows(20).any(|w| w == b"application/epub+zip"),
            "mimetype entry missing"
        );
    }
}
