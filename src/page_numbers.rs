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

//! Printed page numbers, by shape. A book's page numbers are the lines that
//! recur across it with only their number changed: a bare "12", or one
//! dressed in something ("X 12 X", "- 12 -", "× 12 ×"). What is shared here is
//! the shape of such a line and the test for whether a run of them is the
//! book's page numbers; [`crate::pdf`] applies it to paginaverto's blocks and
//! [`crate::smart_import`] to the Markdown an OCR service returns per page.

/// A page-number line is short: this many characters at most, …
pub(crate) const MAX_CHARS: usize = 40;
/// … and this many words besides the number.
pub(crate) const MAX_WORDS: usize = 6;
/// A line recurs on at least this many pages, …
pub(crate) const MIN_HITS: usize = 3;
/// … and on at least a quarter of the book's pages: a chapter's numbered title
/// ("Chapter 12") recurs too, but once in many pages.
pub(crate) const PAGES_PER_HIT: usize = 4;

/// A line reduced to its shape: what comes before and after the number.
pub(crate) struct Template {
    pub(crate) head: String,
    pub(crate) tail: String,
}

impl Template {
    /// The line's shape, if it is short, holds one number, and is not a
    /// sentence: the text up to and after a run of 1 to 5 digits, the only
    /// digits in it.
    pub(crate) fn of(text: &str) -> Option<Template> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_CHARS {
            return None;
        }
        let start = text.find(|c: char| c.is_ascii_digit())?;
        let end = text[start..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(text.len(), |n| start + n);
        if end - start > 5 || text[end..].chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
        let (head, tail) = (&text[..start], &text[end..]);
        if head.split_whitespace().count() + tail.split_whitespace().count() > MAX_WORDS {
            return None;
        }
        // A sentence is not a page number, however short.
        if tail.ends_with(['.', '!', '?', ',', ';', ':']) {
            return None;
        }
        Some(Template {
            head: head.to_string(),
            tail: tail.to_string(),
        })
    }

    /// The shape as a map key.
    pub(crate) fn key(&self) -> String {
        format!("{}\u{0}{}", self.head, self.tail)
    }

    /// Does `text` start with this line? Its number, and how many bytes it
    /// takes, when the rest is nothing, or a word after a space. A bare number
    /// has no shape to match a paragraph's first word by.
    pub(crate) fn matches(&self, text: &str) -> Option<(u32, usize)> {
        let rest = text.strip_prefix(self.head.as_str())?;
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits == 0 || digits > 5 {
            return None;
        }
        let value = rest[..digits].parse().ok()?;
        let after = rest[digits..].strip_prefix(self.tail.as_str())?;
        let len = text.len() - after.len();
        let dressed = !self.head.is_empty() || !self.tail.is_empty();
        match after.chars().next() {
            None => Some((value, len)),
            Some(' ') if dressed => Some((value, len + 1)),
            _ => None,
        }
    }
}

/// Are these, in book order, a book's page numbers? Enough of them for a book
/// of `pages` pages (0 when not known), and rising through it.
pub(crate) fn qualifies(values: &[u32], pages: usize) -> bool {
    if values.len() < MIN_HITS || values.len() * PAGES_PER_HIT < pages {
        return false;
    }
    let rising = values.windows(2).filter(|w| w[1] > w[0]).count();
    rising * 3 >= (values.len() - 1) * 2
}

/// Take the printed page numbers out of a book given as one Markdown string
/// per page, as an OCR service returns it. How many went.
///
/// A page number is the first or last line of its page (two lines of leeway
/// for a picture or a rule beside it). Markdown's dressing of a line is not
/// its shape: `# 12`, `**12**` and `12` are the same page number.
pub(crate) fn strip_markdown_pages(pages: &mut [String]) -> usize {
    use std::collections::HashMap;

    /// A line's text without its Markdown dressing; nothing for a picture.
    fn bare(line: &str) -> &str {
        let line = line.trim();
        if line.starts_with("![") {
            return "";
        }
        let line = line.trim_start_matches('#').trim_start();
        line.trim_matches(|c| matches!(c, '*' | '_' | ' '))
    }

    /// A line of the book: (page, line).
    type Line = (usize, usize);

    // The lines at a page's edges.
    let edges: Vec<Line> = pages
        .iter()
        .enumerate()
        .flat_map(|(p, md)| {
            let text: Vec<usize> = md
                .lines()
                .enumerate()
                .filter(|(_, l)| !l.trim().is_empty())
                .map(|(i, _)| i)
                .collect();
            let n = text.len();
            let mut at: Vec<usize> = text.iter().take(2).copied().collect();
            at.extend(text.iter().skip(n.saturating_sub(2)).copied());
            at.sort_unstable();
            at.dedup();
            at.into_iter().map(move |l| (p, l))
        })
        .collect();

    let line_at = |pages: &[String], (p, l): Line| -> String {
        bare(pages[p].lines().nth(l).unwrap_or("")).to_string()
    };

    // Every shape found at an edge, with where it was found and its numbers.
    let mut shapes: HashMap<String, (Template, Vec<(Line, u32)>)> = HashMap::new();
    for &edge in &edges {
        let text = line_at(pages, edge);
        if let Some(t) = Template::of(&text)
            && let Some((value, _)) = t.matches(text.trim())
        {
            shapes
                .entry(t.key())
                .or_insert_with(|| (t, Vec::new()))
                .1
                .push((edge, value));
        }
    }

    let mut doomed: Vec<Line> = Vec::new();
    for (_, found) in shapes.into_values() {
        let values: Vec<u32> = found.iter().map(|&(_, v)| v).collect();
        if qualifies(&values, pages.len()) {
            doomed.extend(found.into_iter().map(|(at, _)| at));
        }
    }
    doomed.sort_unstable();
    doomed.dedup();

    let mut by_page: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(p, l) in &doomed {
        by_page.entry(p).or_default().push(l);
    }
    for (p, lines) in by_page {
        pages[p] = pages[p]
            .lines()
            .enumerate()
            .filter(|(i, _)| !lines.contains(i))
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
    }
    doomed.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(pages: usize, footer: impl Fn(usize) -> String) -> Vec<String> {
        (1..=pages)
            .map(|p| {
                format!(
                    "Text of page {p}, which goes on.\n\nMore of it.\n\n{}",
                    footer(p)
                )
            })
            .collect()
    }

    #[test]
    fn a_line_is_its_shape_around_one_number() {
        let t = Template::of("× 12 ×").unwrap();
        assert_eq!((t.head.as_str(), t.tail.as_str()), ("× ", " ×"));
        assert_eq!(t.matches("× 13 ×"), Some((13, "× 13 ×".len())));
        assert_eq!(t.matches("× 13 × and on"), Some((13, "× 13 × ".len())));
        assert!(t.matches("× 13 ×x").is_none());
        assert!(Template::of("Chapter 12, part 3").is_none());
        assert!(Template::of("no number here").is_none());
        assert!(Template::of("Text of page 12.").is_none(), "a sentence");
        assert!(Template::of("12").unwrap().matches("12 men").is_none());
    }

    #[test]
    fn a_run_of_numbers_qualifies_when_it_rises_through_the_book() {
        assert!(qualifies(&[1, 2, 3], 0));
        assert!(qualifies(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 40));
        assert!(!qualifies(&[1, 2], 0));
        assert!(!qualifies(&[1, 2, 3], 40));
        assert!(!qualifies(&[5, 5, 5, 5], 0));
        assert!(qualifies(&[3, 4, 5, 2, 6, 7], 0));
    }

    #[test]
    fn decorated_footers_leave_the_ocr_pages() {
        let mut pages = book(8, |p| format!("× {p} ×"));
        assert_eq!(strip_markdown_pages(&mut pages), 8);
        assert!(pages.iter().all(|p| !p.contains('×')));
        assert!(pages[3].contains("Text of page 4, which goes on."));
    }

    #[test]
    fn a_footer_the_ocr_set_as_maths_goes() {
        let mut pages = book(8, |p| format!("$\\times {p} \\times$"));
        assert_eq!(strip_markdown_pages(&mut pages), 8);
        assert!(pages.iter().all(|p| !p.contains("times")));
    }

    #[test]
    fn bare_and_dressed_up_numbers_are_one_shape() {
        let mut pages = book(9, |p| match p % 3 {
            0 => format!("**{p}**"),
            1 => format!("# {p}"),
            _ => p.to_string(),
        });
        assert_eq!(strip_markdown_pages(&mut pages), 9);
        assert!(pages.iter().all(|p| !p.contains('#') && !p.contains("**")));
    }

    #[test]
    fn a_header_above_a_picture_goes_too() {
        let mut pages: Vec<String> = (1..=6)
            .map(|p| {
                format!(
                    "- {p} -\n\n![fig](./p{p}_img.jpeg)\n\nCaption of page {p}, under the picture."
                )
            })
            .collect();
        assert_eq!(strip_markdown_pages(&mut pages), 6);
        assert!(pages[0].starts_with("![fig]"));
        assert!(pages[0].contains("Caption of page 1, under"));
    }

    #[test]
    fn numbered_headings_in_the_text_are_not_page_numbers() {
        let mut pages: Vec<String> = (1..=16)
            .map(|p| format!("Paragraph on page {p}, which goes on.\n\nAnd another, longer one."))
            .collect();
        pages[2] = format!("## Chapter 1\n\n{}", pages[2]);
        pages[7] = format!("## Chapter 2\n\n{}", pages[7]);
        pages[12] = format!("## Chapter 3\n\n{}", pages[12]);
        let before = pages.clone();
        assert_eq!(strip_markdown_pages(&mut pages), 0);
        assert_eq!(pages, before);
    }

    #[test]
    fn a_number_in_the_middle_of_a_page_is_not_a_page_number() {
        let mut pages: Vec<String> = (1..=6)
            .map(|p| {
                format!(
                    "Start of page {p}.\n\nSecond line.\n\n{p}\n\nThird line.\n\nLast line of it."
                )
            })
            .collect();
        assert_eq!(strip_markdown_pages(&mut pages), 0);
    }

    #[test]
    fn the_same_numbers_on_every_page_are_not_page_numbers() {
        let mut pages = book(6, |_| "Volume 2".to_string());
        assert_eq!(strip_markdown_pages(&mut pages), 0);
    }
}
