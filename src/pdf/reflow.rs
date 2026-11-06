//! Render paginaverto's chapters to XHTML, then hand off to the shared
//! `epub_writer` for OCF/OPF/nav packaging.

use std::path::Path;

use anyhow::Result;

use crate::epub_writer::{ImageAsset, RenderedChapter, esc, package_epub};

use paginaverto::{Block, BlockKind, Chapter, Run};

/// Write `chapters` as a reflow EPUB to `out`.
pub(crate) fn write_epub(
    out: &Path,
    title: &str,
    author: Option<&str>,
    language: &str,
    chapters: &[Chapter<'_>],
) -> Result<()> {
    let mut images: Vec<ImageAsset> = Vec::new();
    let mut rendered: Vec<RenderedChapter> = Vec::new();

    for ch in chapters {
        // No heading is made up for a chapter the PDF gives none (its front
        // matter): the nav leaves it out.
        let mut body = match &ch.title {
            Some(t) => format!("<h1>{}</h1>\n", esc(t)),
            None => String::new(),
        };
        for block in ch.blocks {
            render_block(block, &mut body, &mut images);
        }
        rendered.push(RenderedChapter {
            title: ch.title.clone(),
            body,
        });
    }

    package_epub(out, title, author, language, &rendered, &images, None)
}

fn render_block(block: &Block, body: &mut String, images: &mut Vec<ImageAsset>) {
    match &block.kind {
        BlockKind::Heading { level, runs } => {
            // The chapter's own title is the h1; headings inside it sit below.
            let tag = heading_tag(*level);
            body.push_str(&format!("<{tag}>{}</{tag}>\n", render_runs(runs)));
        }
        BlockKind::Paragraph { runs } => {
            body.push_str(&format!("<p>{}</p>\n", render_runs(runs)));
        }
        BlockKind::Figure(fig) => {
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
        // A kind from a newer paginaverto: its text as a paragraph, as
        // paginaverto asks, so nothing is lost.
        _ => {
            if !block.runs().is_empty() {
                body.push_str(&format!("<p>{}</p>\n", render_runs(block.runs())));
            }
        }
    }
}

/// The element for a heading of `level` inside a chapter: h2 for level 2, down
/// to h6.
fn heading_tag(level: u32) -> String {
    format!("h{}", level.clamp(2, 6))
}

/// A block's text, italic runs in `<em>`.
fn render_runs(runs: &[Run]) -> String {
    let mut out = String::new();
    for run in runs {
        if run.italic && !run.text.trim().is_empty() {
            out.push_str(&format!("<em>{}</em>", esc(&run.text)));
        } else {
            out.push_str(&esc(&run.text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{heading_tag, write_epub};
    use paginaverto::{Block, Document, Run};

    #[test]
    fn writes_a_valid_epub() {
        let mut doc = Document::default();
        doc.blocks = vec![
            Block::heading("h1", 1, vec![Run::new("Chapter <1>", false)]),
            Block::paragraph(
                "p1",
                vec![Run::new("Hello & ", false), Run::new("welcome", true)],
            ),
            Block::heading("h2", 2, vec![Run::new("A part", false)]),
            Block::paragraph("p2", vec![Run::new("More.", false)]),
        ];
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("book.epub");
        write_epub(&out, "My Title", None, "en", &doc.chapters()).unwrap();
        let report = epubveri::validate_bytes(std::fs::read(&out).unwrap());
        assert_eq!(report.fatals() + report.errors(), 0);
    }

    #[test]
    fn headings_inside_a_chapter_sit_below_its_title() {
        assert_eq!(heading_tag(1), "h2");
        assert_eq!(heading_tag(2), "h2");
        assert_eq!(heading_tag(3), "h3");
        assert_eq!(heading_tag(9), "h6");
    }
}
