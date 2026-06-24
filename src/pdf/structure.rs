//! Turn extracted page text into clean, structured chapters (hybrid approach).
//!
//! Text + paragraphs come from lopdf `extract_text` (clean on both born-digital
//! and OCR-text-layer PDFs — it emits ≈ one block per paragraph). Headings are
//! detected lexically (short + mostly uppercase, gated on real body following),
//! boosted on born-digital pages by the font-size signal from [`super::extract`]
//! (unreliable on OCR-layer scans, whose per-word geometry is noisy). Running
//! heads/feet are stripped by cross-page recurrence; line-break hyphens are
//! joined.

use std::collections::{HashMap, HashSet};

use regex::Regex;

use super::extract::{self, PageText};

/// A chapter: an optional heading plus its paragraphs.
#[derive(Debug, Clone, Default)]
pub(crate) struct Chapter {
    pub title: Option<String>,
    pub paragraphs: Vec<String>,
}

enum Item {
    Heading(String),
    Body(String),
}

/// Build the book's chapters from per-page extracted text.
pub(crate) fn build_book(pages: &[PageText]) -> Vec<Chapter> {
    let heads = recurring_templates(pages);
    let dehyphen = Regex::new(r"(\p{L})-\s+(\p{Ll})").unwrap();

    // Flatten kept blocks into a stream of headings / body paragraphs.
    let mut items: Vec<Item> = Vec::new();
    for page in pages {
        let big: HashSet<&str> = page.big_font.iter().map(String::as_str).collect();
        for block in &page.blocks {
            if is_page_number(block) || heads.contains(&template(block)) {
                continue;
            }
            let clean = clean_block(block, &dehyphen);
            if clean.is_empty() {
                continue;
            }
            if is_heading(&clean, page.born_digital, &big) {
                items.push(Item::Heading(clean));
            } else {
                items.push(Item::Body(clean));
            }
        }
    }

    // A heading only starts a chapter if real body follows (≥400 chars before
    // the next heading); otherwise it's a false positive (front-matter, stacked
    // titles) and is demoted to a paragraph.
    const MIN_CHAPTER_BODY: usize = 400;
    let mut chapters: Vec<Chapter> = vec![Chapter::default()];
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Body(p) => chapters.last_mut().unwrap().paragraphs.push(p.clone()),
            Item::Heading(h) => {
                let body: usize = items[i + 1..]
                    .iter()
                    .take_while(|it| matches!(it, Item::Body(_)))
                    .map(|it| match it {
                        Item::Body(p) => p.len(),
                        _ => 0,
                    })
                    .sum();
                if body >= MIN_CHAPTER_BODY {
                    chapters.push(Chapter {
                        title: Some(h.clone()),
                        paragraphs: Vec::new(),
                    });
                } else {
                    chapters.last_mut().unwrap().paragraphs.push(h.clone());
                }
            }
        }
    }

    chapters.retain(|c| c.title.is_some() || !c.paragraphs.is_empty());
    chapters
}

/// Normalise to a recurrence template: letters only, lowercased, roman-numeral
/// tokens dropped (so "INTRODUCTION ix" variants / "12 TITLE" collapse).
fn template(s: &str) -> String {
    extract::letters_only(s)
        .split_whitespace()
        .filter(|w| !is_roman(w))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_roman(w: &str) -> bool {
    !w.is_empty() && w.len() <= 8 && w.chars().all(|c| "ivxlcdm".contains(c))
}

fn is_page_number(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.len() <= 6
        && (t.chars().all(|c| c.is_ascii_digit())
            || t.chars().all(|c| "ivxlcdmIVXLCDM".contains(c)))
}

/// First/last block of each page, normalised; those recurring across enough
/// pages are running heads/feet.
fn recurring_templates(pages: &[PageText]) -> HashSet<String> {
    let mut freq: HashMap<String, usize> = HashMap::new();
    for page in pages {
        if let Some(first) = page.blocks.first() {
            *freq.entry(template(first)).or_default() += 1;
        }
        if page.blocks.len() > 1
            && let Some(last) = page.blocks.last()
        {
            *freq.entry(template(last)).or_default() += 1;
        }
    }
    let threshold = (pages.len() / 30).max(5);
    freq.into_iter()
        .filter(|(t, n)| t.len() > 3 && *n >= threshold)
        .map(|(t, _)| t)
        .collect()
}

fn clean_block(block: &str, dehyphen: &Regex) -> String {
    let joined = dehyphen.replace_all(block, "$1$2");
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A heading is short and either mostly-uppercase (works on both tiers) or, on
/// born-digital pages, rendered in a notably-large font.
fn is_heading(s: &str, born_digital: bool, big_font: &HashSet<&str>) -> bool {
    if s.split_whitespace().count() > 12 || s.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 3 {
        return false;
    }
    let upper = letters.iter().filter(|c| c.is_uppercase()).count();
    let mostly_caps = upper as f32 / letters.len() as f32 >= 0.85;
    let large_font = born_digital && big_font.contains(extract::letters_only(s).as_str());
    mostly_caps || large_font
}
