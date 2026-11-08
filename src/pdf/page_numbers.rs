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

//! The printed page numbers paginaverto leaves in. It drops a bare numeral and
//! a running head that recurs letter for letter; a page number dressed in
//! something else ("X 12 X", "- 12 -", "~ 12 ~") is neither, and comes through
//! as a paragraph of its own on every page, or glued to the front of the
//! paragraph that continues over the page. This finds the lines that recur
//! across the book with only their number changed, and takes them out.

use std::collections::HashMap;

use paginaverto::{Block, BlockKind, Document, Run};

use crate::page_numbers::{Template, qualifies};

/// Where a page-number line was found.
struct Hit {
    /// The block.
    at: usize,
    /// Its number.
    value: u32,
    /// How many bytes of the block's text the line takes: all of it, or a
    /// prefix the next page's paragraph was glued to.
    len: usize,
}

/// Take the printed page numbers out of `doc`'s blocks. How many went.
pub(crate) fn strip(doc: &mut Document) -> usize {
    strip_blocks(&mut doc.blocks, doc.pages.len())
}

/// [`strip`] on the blocks of a book of `pages` pages (0 when not known).
fn strip_blocks(blocks: &mut Vec<Block>, pages: usize) -> usize {
    let is_text = |b: &Block| !matches!(b.kind, BlockKind::Figure(_));
    let is_paragraph = |b: &Block| matches!(b.kind, BlockKind::Paragraph { .. });

    // The lines that are a paragraph on their own, by shape.
    let mut whole: HashMap<String, (Template, Vec<usize>)> = HashMap::new();
    for (at, block) in blocks.iter().enumerate().filter(|(_, b)| is_paragraph(b)) {
        if let Some(t) = Template::of(&block.text()) {
            whole
                .entry(t.key())
                .or_insert_with(|| (t, Vec::new()))
                .1
                .push(at);
        }
    }

    // The first and last text block of each page, where positions are known.
    let mut first: HashMap<u32, usize> = HashMap::new();
    let mut last: HashMap<u32, usize> = HashMap::new();
    for (at, block) in blocks.iter().enumerate().filter(|(_, b)| is_text(b)) {
        for b in &block.boxes {
            first.entry(b.page).or_insert(at);
            last.insert(b.page, at);
        }
    }
    let at_edge = |at: usize| {
        blocks[at]
            .boxes
            .iter()
            .any(|b| first.get(&b.page) == Some(&at) || last.get(&b.page) == Some(&at))
    };

    let mut hits: Vec<Hit> = Vec::new();
    for (template, own) in whole.into_values() {
        if own.len() < 2 {
            continue;
        }
        // Every paragraph that is the line, or starts with it.
        let mut found: Vec<Hit> = Vec::new();
        for (at, block) in blocks.iter().enumerate().filter(|(_, b)| is_paragraph(b)) {
            let text = block.text();
            let line = text.trim_start();
            if let Some((value, len)) = template.matches(line) {
                found.push(Hit {
                    at,
                    value,
                    len: len + (text.len() - line.len()),
                });
            }
        }
        // Enough of them for the book, and rising through it.
        let values: Vec<u32> = found.iter().map(|h| h.value).collect();
        if !qualifies(&values, pages) {
            continue;
        }
        // A line on its own sits at the top or the bottom of its page.
        let placed = own
            .iter()
            .filter(|&&at| !blocks[at].boxes.is_empty())
            .count();
        let edge = own.iter().filter(|&&at| at_edge(at)).count();
        if edge * 3 < placed * 2 {
            continue;
        }
        hits.extend(found);
    }
    if hits.is_empty() {
        return 0;
    }
    hits.sort_by_key(|h| h.at);
    hits.dedup_by_key(|h| h.at);

    let removed = hits.len();
    let mut hits = hits.into_iter().peekable();
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for (at, mut block) in std::mem::take(blocks).into_iter().enumerate() {
        let Some(hit) = hits.next_if(|h| h.at == at) else {
            out.push(block);
            continue;
        };
        let runs = cut(block.runs(), hit.len);
        if runs.iter().all(|r| r.text.trim().is_empty()) {
            continue;
        }
        // The paragraph the line was glued to: on its own again, or joined to
        // the one the page before broke off, as paginaverto joins them.
        let continues = runs
            .first()
            .and_then(|r| r.text.chars().next())
            .is_some_and(char::is_lowercase);
        if continues
            && let Some(prev) = out.iter_mut().rev().find(|b| is_text(b))
            && let BlockKind::Paragraph { runs: before } = &mut prev.kind
            && !ends_sentence(before)
        {
            join(before, runs);
            prev.boxes.extend(block.boxes.iter().copied());
            continue;
        }
        block.kind = BlockKind::Paragraph { runs };
        out.push(block);
    }
    *blocks = out;
    removed
}

/// The runs without their first `len` bytes.
fn cut(runs: &[Run], mut len: usize) -> Vec<Run> {
    let mut out = Vec::new();
    for run in runs {
        if len >= run.text.len() {
            len -= run.text.len();
            continue;
        }
        out.push(Run::new(&run.text[len..], run.italic));
        len = 0;
    }
    out
}

/// The paragraph ends a sentence, as paginaverto decides it.
fn ends_sentence(runs: &[Run]) -> bool {
    runs.last().is_some_and(|r| {
        r.text
            .trim_end()
            .ends_with(['.', '!', '?', ':', ';', '…', '"', '”', '’', ')'])
    })
}

/// Continue `before` with `next`, a space between, unless a line-break hyphen
/// ends `before`: then the word is made whole.
fn join(before: &mut Vec<Run>, next: Vec<Run>) {
    if let Some(last) = before.last_mut() {
        let hyphenated = last.text.ends_with('-')
            && last
                .text
                .chars()
                .rev()
                .nth(1)
                .is_some_and(char::is_alphabetic);
        if hyphenated {
            last.text.pop();
        } else {
            last.text.push(' ');
        }
    }
    before.extend(next);
}

#[cfg(test)]
mod tests {
    use super::{Template, strip_blocks};
    use paginaverto::{Block, Run};

    fn para(text: &str) -> Block {
        Block::paragraph("b", vec![Run::new(text, false)])
    }

    /// A book of `pages` pages, each a paragraph of body text and the page
    /// number written as `footer` says, with the number at `#`.
    fn book(pages: u32, footer: &str) -> Vec<Block> {
        let mut blocks = Vec::new();
        for n in 1..=pages {
            blocks.push(para(&format!(
                "Body text of page {n}, long enough to be a paragraph."
            )));
            blocks.push(para(&footer.replace('#', &n.to_string())));
        }
        blocks
    }

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks.iter().map(Block::text).collect()
    }

    #[test]
    fn a_line_is_its_shape_around_one_number() {
        let t = Template::of("X 12 X").unwrap();
        assert_eq!((t.head.as_str(), t.tail.as_str()), ("X ", " X"));
        assert_eq!(t.matches("X 13 X"), Some((13, 6)));
        assert_eq!(t.matches("X 13 X dle of it."), Some((13, 7)));
        assert_eq!(t.matches("X 13 Xylophone"), None);
        assert_eq!(t.matches("Y 13 X"), None);
        assert!(Template::of("Page 1 of 20").is_none(), "two numbers");
        assert!(Template::of("He was 12 when the war came to the town.").is_none());
        // A bare number is a shape, but not one to find at a paragraph's start.
        let bare = Template::of("12").unwrap();
        assert_eq!(bare.matches("13"), Some((13, 2)));
        assert_eq!(bare.matches("13 men walked in."), None);
    }

    #[test]
    fn decorated_page_numbers_go() {
        let mut blocks = book(12, "X # X");
        assert_eq!(strip_blocks(&mut blocks, 12), 12);
        assert_eq!(blocks.len(), 12);
        assert!(texts(&blocks).iter().all(|p| p.starts_with("Body text")));
    }

    #[test]
    fn a_number_glued_to_the_next_page_goes_and_the_paragraph_is_whole_again() {
        let mut blocks = book(6, "- # -");
        // paginaverto glued page 4's footer to page 5's first line, which
        // continues page 4's last sentence; at page 2 a hyphen broke the word.
        blocks[6] = para("Body text of page 4, long enough to be the middle of a");
        blocks[7] = para("- 4 - sentence that goes on.");
        blocks[2] = para("Body text of page 2, long enough to be inter-");
        blocks[3] = para("- 2 - rupted by a hyphen.");
        assert_eq!(strip_blocks(&mut blocks, 6), 6);
        let got = texts(&blocks);
        assert_eq!(got.len(), 6, "{got:?}");
        assert_eq!(
            got[3],
            "Body text of page 4, long enough to be the middle of a sentence that goes on."
        );
        assert_eq!(
            got[1],
            "Body text of page 2, long enough to be interrupted by a hyphen."
        );
    }

    #[test]
    fn a_glued_line_that_starts_a_sentence_stays_a_paragraph() {
        let mut blocks = book(6, "~ # ~");
        blocks[7] = para("~ 4 ~ The next page starts a new sentence.");
        assert_eq!(strip_blocks(&mut blocks, 6), 6);
        let got = texts(&blocks);
        assert_eq!(got.len(), 7, "{got:?}");
        assert_eq!(got[4], "The next page starts a new sentence.");
    }

    #[test]
    fn italics_survive_the_cut() {
        let mut blocks = book(4, "X # X");
        blocks[3] = Block::paragraph(
            "b",
            vec![
                Run::new("X 2 X ", false),
                Run::new("kitabı", true),
                Run::new(" bitti.", false),
            ],
        );
        blocks[2] = para("Body text of page 2, long enough to be");
        assert_eq!(strip_blocks(&mut blocks, 4), 4);
        let runs = blocks[1].runs();
        assert_eq!(runs.len(), 3, "{runs:?}");
        assert_eq!(runs[0].text, "Body text of page 2, long enough to be ");
        assert!(runs[1].italic && runs[1].text == "kitabı");
        assert_eq!(runs[2].text, " bitti.");
    }

    #[test]
    fn numbered_chapter_titles_are_not_page_numbers() {
        // "Chapter n" once in every 12 paragraphs, in a book of 60 pages.
        let mut blocks = Vec::new();
        for n in 1..=5 {
            blocks.push(para(&format!("Chapter {n}")));
            for _ in 0..12 {
                blocks.push(para("Body text here."));
            }
        }
        assert_eq!(strip_blocks(&mut blocks, 60), 0);
        assert_eq!(blocks.len(), 65);
    }

    #[test]
    fn numbers_that_do_not_rise_are_not_page_numbers() {
        let mut blocks = Vec::new();
        for n in [3, 1, 2, 1, 3, 2] {
            blocks.push(para("Some body text."));
            blocks.push(para(&format!("Figure {n}")));
        }
        assert_eq!(strip_blocks(&mut blocks, 6), 0);
    }

    #[test]
    fn too_few_lines_for_the_book_are_left() {
        let mut blocks = book(3, "X # X");
        assert_eq!(strip_blocks(&mut blocks.clone(), 0), 3, "no page count");
        assert_eq!(
            strip_blocks(&mut blocks, 40),
            0,
            "three lines in forty pages"
        );
    }
}
