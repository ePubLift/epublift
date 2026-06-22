//! Metadata enrichment: fill a book's missing OPF metadata from an online
//! catalogue, keyed by ISBN. Open Library and Google Books are supported (see
//! [`fetch_isbn`]). Any future provider plugs into the same [`Http`] abstraction;
//! Amazon was evaluated and dropped (no usable API — scraping only). See
//! `docs/metadata.md`.
//!
//! **Language-aware (the core rule):** metadata is written only in the book's own
//! language (`dc:language`). We match by ISBN-13 exactly so the edition record
//! yields right-language title/author/publisher, and we **skip** fields whose
//! language doesn't match the book (e.g. English subjects/description on a Turkish
//! book) unless `--allow-foreign-meta`. No translation is ever performed.
//!
//! This module is network-agnostic: callers supply an [`Http`] implementation, so
//! the parsing + language policy is unit-tested with fixtures (no sockets). See
//! `docs/metadata.md`.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::meta::{Metadata, MetadataUpdate};

/// Minimal HTTP GET abstraction so the provider logic is testable without a real
/// client (and so the TLS/HTTP backend can be chosen independently).
pub trait Http {
    /// Fetch `url`, returning the response body as text.
    fn get(&self, url: &str) -> Result<String>;
}

/// A provider-neutral, normalized lookup result. Edition-level fields (title,
/// subtitle, authors, publisher) carry the edition's language; work-level fields
/// (subjects, description) are language-agnostic (usually English).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    pub publisher: Option<String>,
    pub date: Option<String>,
    pub subjects: Vec<String>,
    pub description: Option<String>,
    pub isbn13: Option<String>,
    /// Cover image URL, if any (not written yet — phase 2).
    pub cover_url: Option<String>,
    /// The edition's language as BCP-47 (mapped from Open Library's 3-letter
    /// code), used to enforce the language policy.
    pub edition_language: Option<String>,
}

/// Options controlling how a lookup is merged into the book.
#[derive(Debug, Default, Clone)]
pub struct EnrichOptions {
    /// Force the book's language (BCP-47) when the OPF has no `dc:language`.
    pub lang_override: Option<String>,
    /// Replace fields that already have a value (default: only fill gaps).
    pub overwrite: bool,
    /// Keep fields whose language doesn't match the book's (default: skip them).
    pub allow_foreign_meta: bool,
    /// Fetch and write the description (often publisher-authored; opt-in).
    pub include_description: bool,
}

/// Why a field was not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The book already has a value (and `--overwrite` wasn't given).
    AlreadySet,
    /// The field's language doesn't match the book's.
    LanguageMismatch,
    /// The description was found but `--include-description` wasn't given.
    DescriptionOmitted,
}

/// A field that will be written, with its (stable) key and display value.
#[derive(Debug, Clone)]
pub struct Applied {
    pub field: &'static str,
    pub value: String,
}

/// A field that was not written, with its key and the reason.
#[derive(Debug, Clone)]
pub struct Skipped {
    pub field: &'static str,
    pub reason: SkipReason,
}

/// An advisory that front-ends localize (the field keys/reasons stay structured
/// so the UI can translate them — see the web `applyEnrich`).
#[derive(Debug, Clone)]
pub enum Warning {
    /// The matched edition's language differs from the book's.
    EditionLanguageMismatch { edition: String, book: String },
}

/// What an enrichment will do, for preview (`--dry-run`) and reporting. Field
/// names/reasons are kept as stable keys so callers can render them in any
/// language; [`EnrichPlan::to_text`] is the English CLI rendering.
#[derive(Debug, Default)]
pub struct EnrichPlan {
    pub update: MetadataUpdate,
    pub applied: Vec<Applied>,
    pub skipped: Vec<Skipped>,
    pub warnings: Vec<Warning>,
    /// The language the policy resolved for the book.
    pub book_lang: String,
}

impl EnrichPlan {
    /// A human-readable English summary for the CLI.
    pub fn to_text(&self) -> String {
        let mut o = String::new();
        for w in &self.warnings {
            let Warning::EditionLanguageMismatch { edition, book } = w;
            o.push_str(&format!(
                "[!] The matched edition's language '{edition}' differs from the book's '{book}'; \
                 edition fields skipped (use --allow-foreign-meta).\n"
            ));
        }
        if self.applied.is_empty() {
            o.push_str("[=] Nothing to add — the book's metadata is already complete.\n");
        } else {
            o.push_str("[+] Will set:\n");
            for a in &self.applied {
                if a.value.is_empty() {
                    o.push_str(&format!("      {}\n", a.field));
                } else {
                    o.push_str(&format!("      {} = {}\n", a.field, a.value));
                }
            }
        }
        if !self.skipped.is_empty() {
            o.push_str("[-] Skipped:\n");
            for s in &self.skipped {
                let reason = match s.reason {
                    SkipReason::AlreadySet => "already set",
                    SkipReason::LanguageMismatch => "language mismatch",
                    SkipReason::DescriptionOmitted => "omitted; use --include-description",
                };
                o.push_str(&format!("      {} ({reason})\n", s.field));
            }
        }
        o
    }
}

// ---------------------------------------------------------------------------
// Open Library provider
// ---------------------------------------------------------------------------

/// The Open Library provider.
pub struct OpenLibrary;

impl OpenLibrary {
    /// Look up `isbn`. Uses the flattened `jscmd=data` view for names/subjects,
    /// the edition record for the language (critical for the policy) + work link,
    /// and the work record for the description (only when `want_description`).
    pub fn fetch(&self, isbn: &str, http: &dyn Http, want_description: bool) -> Result<Fetched> {
        let isbn = normalize_isbn(isbn);

        let data = http
            .get(&format!(
                "https://openlibrary.org/api/books?bibkeys=ISBN:{isbn}&jscmd=data&format=json"
            ))
            .context("Open Library request failed")?;
        let mut f = parse_jscmd_data(&data)
            .with_context(|| format!("ISBN {isbn} not found in Open Library"))?;

        // Edition record: language + work link.
        if let Ok(ed) = http.get(&format!("https://openlibrary.org/isbn/{isbn}.json")) {
            let (lang, work_key) = parse_edition(&ed);
            f.edition_language = lang;
            if want_description
                && let Some(wk) = work_key
                && let Ok(w) = http.get(&format!("https://openlibrary.org{wk}.json"))
            {
                apply_work(&mut f, &w);
            }
        }

        f.isbn13.get_or_insert(isbn);
        Ok(f)
    }
}

// ---------------------------------------------------------------------------
// Google Books provider
// ---------------------------------------------------------------------------

/// The Google Books provider. A single request returns everything we need —
/// title, authors, publisher, date, categories, the ISBNs, and (critically for
/// the language policy) the edition `language` — so there is no follow-up call.
pub struct GoogleBooks;

impl GoogleBooks {
    /// Look up `isbn` via `volumes?q=isbn:<isbn>`.
    ///
    /// Anonymous Google Books requests share a small daily quota (HTTP 429 when
    /// exhausted). Set `GOOGLE_BOOKS_API_KEY` to use your own key and raise it.
    pub fn fetch(&self, isbn: &str, http: &dyn Http, want_description: bool) -> Result<Fetched> {
        let isbn = normalize_isbn(isbn);
        let mut url = format!("https://www.googleapis.com/books/v1/volumes?q=isbn:{isbn}");
        if let Ok(key) = std::env::var("GOOGLE_BOOKS_API_KEY")
            && !key.trim().is_empty()
        {
            url.push_str("&key=");
            url.push_str(key.trim());
        }
        let body = http.get(&url).context("Google Books request failed")?;
        let mut f = parse_google(&body, want_description)
            .with_context(|| format!("ISBN {isbn} not found in Google Books"))?;
        f.isbn13.get_or_insert(isbn);
        Ok(f)
    }
}

fn parse_google(json: &str, want_description: bool) -> Result<Fetched> {
    let v: Value = serde_json::from_str(json).context("invalid JSON from Google Books")?;
    let vi = v
        .get("items")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|item| item.get("volumeInfo"))
        .context("book not found")?;

    let isbn13 = vi
        .get("industryIdentifiers")
        .and_then(Value::as_array)
        .and_then(|ids| {
            ids.iter().find_map(|id| {
                (id.get("type").and_then(Value::as_str) == Some("ISBN_13"))
                    .then(|| id.get("identifier").and_then(Value::as_str))
                    .flatten()
                    .map(str::to_string)
            })
        });

    Ok(Fetched {
        title: get_str(vi, "title"),
        subtitle: get_str(vi, "subtitle"),
        authors: str_array(vi, "authors"),
        publisher: get_str(vi, "publisher"),
        date: get_str(vi, "publishedDate"),
        subjects: str_array(vi, "categories"),
        // Only surface the description when the caller asked for it, matching the
        // Open Library provider (so the "description omitted" hint stays correct).
        description: want_description
            .then(|| get_str(vi, "description"))
            .flatten(),
        isbn13,
        cover_url: vi
            .get("imageLinks")
            .and_then(|l| l.get("thumbnail").or_else(|| l.get("smallThumbnail")))
            .and_then(Value::as_str)
            .map(str::to_string),
        // Google Books gives the edition language as a BCP-47 primary subtag
        // (e.g. "en", "tr") — exactly what the language policy needs, no mapping.
        edition_language: get_str(vi, "language"),
    })
}

/// Look up `isbn` with the named provider. `openlibrary` (default) and `google`
/// are supported; both return a normalized [`Fetched`] for [`plan_enrich`].
pub fn fetch_isbn(
    provider: &str,
    isbn: &str,
    http: &dyn Http,
    want_description: bool,
) -> Result<Fetched> {
    match provider {
        "openlibrary" | "ol" => OpenLibrary.fetch(isbn, http, want_description),
        "google" | "googlebooks" | "google-books" => {
            GoogleBooks.fetch(isbn, http, want_description)
        }
        other => anyhow::bail!("unknown provider '{other}'. Supported: openlibrary, google."),
    }
}

/// Strip hyphens/spaces from an ISBN.
fn normalize_isbn(isbn: &str) -> String {
    isbn.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect()
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Collect the `name` field of each object in array `key`.
fn names(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|o| o.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Collect a JSON array of plain strings at `key` (trimmed, non-empty).
fn str_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_jscmd_data(json: &str) -> Result<Fetched> {
    let v: Value = serde_json::from_str(json).context("invalid JSON from Open Library")?;
    // Response is keyed by the bibkey (e.g. "ISBN:978…"); take the first entry.
    let obj = v
        .as_object()
        .and_then(|m| m.values().next())
        .context("book not found")?;

    Ok(Fetched {
        title: get_str(obj, "title"),
        subtitle: get_str(obj, "subtitle"),
        authors: names(obj, "authors"),
        publisher: names(obj, "publishers").into_iter().next(),
        date: get_str(obj, "publish_date"),
        subjects: names(obj, "subjects"),
        isbn13: obj
            .get("identifiers")
            .and_then(|i| i.get("isbn_13"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(str::to_string),
        cover_url: obj
            .get("cover")
            .and_then(|c| c.get("large").or_else(|| c.get("medium")))
            .and_then(Value::as_str)
            .map(str::to_string),
        ..Fetched::default()
    })
}

/// Returns `(edition_language_bcp47, work_key)` from an edition record.
fn parse_edition(json: &str) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return (None, None);
    };
    let lang = v
        .get("languages")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|o| o.get("key"))
        .and_then(Value::as_str)
        .map(|k| map_lang(k.trim_start_matches("/languages/")));
    let work = v
        .get("works")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|o| o.get("key"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (lang, work)
}

/// Fold a work record's description/subjects into the result (without clobbering
/// what the edition view already provided).
fn apply_work(f: &mut Fetched, json: &str) {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return;
    };
    if f.description.is_none() {
        // `description` is either a string or `{ "type": …, "value": … }`.
        f.description = v.get("description").and_then(|d| {
            d.as_str()
                .or_else(|| d.get("value").and_then(Value::as_str))
                .map(str::to_string)
        });
    }
    if f.subjects.is_empty()
        && let Some(arr) = v.get("subjects").and_then(Value::as_array)
    {
        f.subjects = arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
}

/// Map an Open Library / ISO 639-2/3 language code to a BCP-47 primary subtag.
fn map_lang(code: &str) -> String {
    match code.to_ascii_lowercase().as_str() {
        "eng" => "en",
        "tur" => "tr",
        "kor" => "ko",
        "ger" | "deu" => "de",
        "fre" | "fra" => "fr",
        "spa" => "es",
        "ita" => "it",
        "rus" => "ru",
        "ara" => "ar",
        "jpn" => "ja",
        "chi" | "zho" => "zh",
        "por" => "pt",
        "dut" | "nld" => "nl",
        "gre" | "ell" => "el",
        "heb" => "he",
        "hin" => "hi",
        "fas" | "per" => "fa",
        "pol" => "pl",
        "swe" => "sv",
        "ukr" => "uk",
        other => other,
    }
    .to_string()
}

/// The BCP-47 primary subtag, lowercased (e.g. `"tr-TR"` → `"tr"`).
fn primary_subtag(s: &str) -> String {
    s.split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// Language-aware merge
// ---------------------------------------------------------------------------

enum Decide {
    Set,
    Present,
    Lang,
}

fn decide(present: bool, overwrite: bool, lang_ok: bool) -> Decide {
    if present && !overwrite {
        Decide::Present
    } else if !lang_ok {
        Decide::Lang
    } else {
        Decide::Set
    }
}

/// Build the gap-filling, language-aware plan to merge `fetched` into `existing`.
///
/// Resolves the book's language (from `--lang` or `dc:language`), then for each
/// field decides Set / skip-present / skip-language. Edition-level fields obey the
/// matched edition language; work-level fields (subjects, description) are treated
/// as foreign unless the book is English or `--allow-foreign-meta` is set.
pub fn plan_enrich(
    existing: &Metadata,
    fetched: &Fetched,
    opts: &EnrichOptions,
) -> Result<EnrichPlan> {
    let book_lang = opts
        .lang_override
        .clone()
        .or_else(|| existing.languages.first().cloned())
        .context("book has no dc:language; pass --lang <BCP-47> (e.g. --lang tr)")?;
    let book_primary = primary_subtag(&book_lang);

    let mut plan = EnrichPlan {
        book_lang: book_lang.clone(),
        ..EnrichPlan::default()
    };

    // Edition-level language gate.
    let edition_ok = match &fetched.edition_language {
        Some(el) if !primary_subtag(el).eq_ignore_ascii_case(&book_primary) => {
            plan.warnings.push(Warning::EditionLanguageMismatch {
                edition: el.clone(),
                book: book_lang.clone(),
            });
            opts.allow_foreign_meta
        }
        _ => true, // matches, or unknown → best effort
    };
    // Work-level fields (subjects/description) are usually English.
    let work_ok = book_primary == "en" || opts.allow_foreign_meta;

    // Decide Set / skip-present / skip-language per field, then act inline (a
    // macro keeps the borrow of `plan` simple — no nested closures).
    macro_rules! consider {
        ($field:expr, $value:expr, $present:expr, $lang_ok:expr, $set:block) => {
            match decide($present, opts.overwrite, $lang_ok) {
                Decide::Set => {
                    $set
                    plan.applied.push(Applied { field: $field, value: $value });
                }
                Decide::Present => plan.skipped.push(Skipped {
                    field: $field,
                    reason: SkipReason::AlreadySet,
                }),
                Decide::Lang => plan.skipped.push(Skipped {
                    field: $field,
                    reason: SkipReason::LanguageMismatch,
                }),
            }
        };
    }

    // Title (edition-level).
    let has_main = existing
        .titles
        .iter()
        .any(|t| t.title_type.as_deref() != Some("subtitle"));
    if let Some(t) = &fetched.title {
        consider!("title", t.clone(), has_main, edition_ok, {
            plan.update.title = Some(t.clone());
        });
    }
    // Subtitle (edition-level).
    let has_sub = existing
        .titles
        .iter()
        .any(|t| t.title_type.as_deref() == Some("subtitle"));
    if let Some(s) = &fetched.subtitle {
        consider!("subtitle", s.clone(), has_sub, edition_ok, {
            plan.update.subtitle = Some(s.clone());
        });
    }
    // Authors (edition-level).
    if !fetched.authors.is_empty() {
        consider!(
            "authors",
            fetched.authors.join(", "),
            !existing.creators.is_empty(),
            edition_ok,
            {
                plan.update.authors = Some(fetched.authors.clone());
            }
        );
    }
    // Publisher (edition-level).
    if let Some(p) = &fetched.publisher {
        consider!(
            "publisher",
            p.clone(),
            existing.publisher.is_some(),
            edition_ok,
            {
                plan.update.publisher = Some(p.clone());
            }
        );
    }
    // Date (language-neutral).
    if let Some(d) = &fetched.date {
        consider!("date", d.clone(), existing.date.is_some(), true, {
            plan.update.date = Some(d.clone());
        });
    }
    // ISBN (language-neutral) → dc:source.
    let has_isbn = existing
        .source
        .as_deref()
        .is_some_and(|s| s.contains("isbn"))
        || existing.identifiers.iter().any(|i| {
            i.scheme
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
                || i.value.to_ascii_lowercase().contains("isbn")
        });
    if let Some(isbn) = &fetched.isbn13 {
        consider!("isbn", isbn.clone(), has_isbn, true, {
            plan.update.isbn = Some(isbn.clone());
        });
    }
    // Subjects (work-level).
    if !fetched.subjects.is_empty() {
        consider!(
            "subjects",
            fetched.subjects.join(", "),
            !existing.subjects.is_empty(),
            work_ok,
            {
                plan.update.subjects = Some(fetched.subjects.clone());
            }
        );
    }
    // Description (work-level, opt-in).
    if let Some(d) = &fetched.description {
        if opts.include_description {
            consider!(
                "description",
                String::new(),
                existing.description.is_some(),
                work_ok,
                {
                    plan.update.description = Some(d.clone());
                }
            );
        } else {
            plan.skipped.push(Skipped {
                field: "description",
                reason: SkipReason::DescriptionOmitted,
            });
        }
    }

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{Identifier, Title};
    use std::collections::HashMap;

    struct Mock(HashMap<String, String>);
    impl Http for Mock {
        fn get(&self, url: &str) -> Result<String> {
            self.0
                .get(url)
                .cloned()
                .with_context(|| format!("no fixture for {url}"))
        }
    }

    fn turkish_book() -> Metadata {
        Metadata {
            languages: vec!["tr".to_string()],
            titles: vec![Title {
                value: "Eski Başlık".to_string(),
                title_type: None,
            }],
            ..Metadata::default()
        }
    }

    #[test]
    fn fills_matching_language_skips_foreign_subjects() {
        let fetched = Fetched {
            title: Some("Senin Kovan Ne Kadar Dolu".to_string()),
            authors: vec!["Tom Rath".to_string()],
            publisher: Some("Optimist".to_string()),
            date: Some("2018".to_string()),
            subjects: vec!["Self-actualization".to_string()], // English work-level
            isbn13: Some("9786051113111".to_string()),
            edition_language: Some("tr".to_string()),
            ..Fetched::default()
        };
        let plan = plan_enrich(&turkish_book(), &fetched, &EnrichOptions::default()).unwrap();

        // Title already present → skipped; authors/publisher/date/isbn filled.
        assert!(plan.update.authors.is_some());
        assert_eq!(plan.update.publisher.as_deref(), Some("Optimist"));
        assert_eq!(plan.update.isbn.as_deref(), Some("9786051113111"));
        assert!(plan.skipped.iter().any(|s| s.field == "title"));
        // English subjects skipped on a Turkish book.
        assert!(plan.update.subjects.is_none());
        assert!(
            plan.skipped
                .iter()
                .any(|s| s.field == "subjects" && s.reason == SkipReason::LanguageMismatch)
        );
    }

    #[test]
    fn allow_foreign_keeps_subjects() {
        let fetched = Fetched {
            subjects: vec!["Psychology".to_string()],
            edition_language: Some("tr".to_string()),
            ..Fetched::default()
        };
        let opts = EnrichOptions {
            allow_foreign_meta: true,
            ..EnrichOptions::default()
        };
        let plan = plan_enrich(&turkish_book(), &fetched, &opts).unwrap();
        assert_eq!(
            plan.update.subjects.as_deref(),
            Some(&["Psychology".to_string()][..])
        );
    }

    #[test]
    fn edition_language_mismatch_warns_and_skips() {
        let fetched = Fetched {
            title: Some("How Full Is Your Bucket?".to_string()),
            edition_language: Some("en".to_string()), // English edition for a Turkish book
            ..Fetched::default()
        };
        // Use a book with no existing title so the only reason to skip is language.
        let mut book = turkish_book();
        book.titles.clear();
        let plan = plan_enrich(&book, &fetched, &EnrichOptions::default()).unwrap();
        assert!(plan.update.title.is_none());
        assert!(!plan.warnings.is_empty());
    }

    #[test]
    fn does_not_overwrite_without_flag_but_does_with_it() {
        let fetched = Fetched {
            publisher: Some("Yeni Yayınevi".to_string()),
            edition_language: Some("tr".to_string()),
            ..Fetched::default()
        };
        let mut book = turkish_book();
        book.publisher = Some("Eski Yayınevi".to_string());

        let plan = plan_enrich(&book, &fetched, &EnrichOptions::default()).unwrap();
        assert!(plan.update.publisher.is_none());

        let opts = EnrichOptions {
            overwrite: true,
            ..EnrichOptions::default()
        };
        let plan = plan_enrich(&book, &fetched, &opts).unwrap();
        assert_eq!(plan.update.publisher.as_deref(), Some("Yeni Yayınevi"));
    }

    #[test]
    fn missing_language_requires_override() {
        let book = Metadata::default(); // no dc:language
        let fetched = Fetched {
            title: Some("X".to_string()),
            ..Fetched::default()
        };
        assert!(plan_enrich(&book, &fetched, &EnrichOptions::default()).is_err());
        let opts = EnrichOptions {
            lang_override: Some("tr".to_string()),
            ..EnrichOptions::default()
        };
        assert!(plan_enrich(&book, &fetched, &opts).is_ok());
    }

    #[test]
    fn open_library_fetch_parses_fixtures() {
        let mut m = HashMap::new();
        m.insert(
            "https://openlibrary.org/api/books?bibkeys=ISBN:9780140328721&jscmd=data&format=json"
                .to_string(),
            r#"{"ISBN:9780140328721":{"title":"Fantastic Mr Fox","authors":[{"name":"Roald Dahl"}],"publishers":[{"name":"Puffin"}],"publish_date":"1988","subjects":[{"name":"Foxes"}],"identifiers":{"isbn_13":["9780140328721"]}}}"#.to_string(),
        );
        m.insert(
            "https://openlibrary.org/isbn/9780140328721.json".to_string(),
            r#"{"languages":[{"key":"/languages/eng"}],"works":[{"key":"/works/OL45804W"}]}"#
                .to_string(),
        );
        let f = OpenLibrary
            .fetch("978-0-14-032872-1", &Mock(m), false)
            .unwrap();
        assert_eq!(f.title.as_deref(), Some("Fantastic Mr Fox"));
        assert_eq!(f.authors, vec!["Roald Dahl"]);
        assert_eq!(f.publisher.as_deref(), Some("Puffin"));
        assert_eq!(f.edition_language.as_deref(), Some("en")); // mapped from "eng"
        assert_eq!(f.isbn13.as_deref(), Some("9780140328721"));
    }

    #[test]
    fn identifiers_count_as_existing_isbn() {
        let book = Metadata {
            languages: vec!["en".to_string()],
            identifiers: vec![Identifier {
                value: "9780140328721".to_string(),
                scheme: Some("ISBN".to_string()),
                unique: false,
            }],
            ..Metadata::default()
        };
        let fetched = Fetched {
            isbn13: Some("9780140328721".to_string()),
            edition_language: Some("en".to_string()),
            ..Fetched::default()
        };
        let plan = plan_enrich(&book, &fetched, &EnrichOptions::default()).unwrap();
        assert!(plan.update.isbn.is_none()); // already have an ISBN identifier
    }

    #[test]
    fn google_books_fetch_parses_fixture() {
        let mut m = HashMap::new();
        m.insert(
            "https://www.googleapis.com/books/v1/volumes?q=isbn:9780140328721".to_string(),
            r#"{"items":[{"volumeInfo":{"title":"Fantastic Mr Fox","authors":["Roald Dahl"],"publisher":"Puffin","publishedDate":"1988","categories":["Juvenile Fiction"],"language":"en","industryIdentifiers":[{"type":"ISBN_10","identifier":"0140328726"},{"type":"ISBN_13","identifier":"9780140328721"}],"description":"A story about a clever fox."}}]}"#.to_string(),
        );
        // want_description = false → description must be omitted (parity with OL).
        let f = fetch_isbn("google", "978-0-14-032872-1", &Mock(m), false).unwrap();
        assert_eq!(f.title.as_deref(), Some("Fantastic Mr Fox"));
        assert_eq!(f.authors, vec!["Roald Dahl"]);
        assert_eq!(f.publisher.as_deref(), Some("Puffin"));
        assert_eq!(f.edition_language.as_deref(), Some("en"));
        assert_eq!(f.isbn13.as_deref(), Some("9780140328721"));
        assert_eq!(f.subjects, vec!["Juvenile Fiction"]);
        assert!(f.description.is_none());
    }

    #[test]
    fn google_books_includes_description_when_asked() {
        let mut m = HashMap::new();
        m.insert(
            "https://www.googleapis.com/books/v1/volumes?q=isbn:9780140328721".to_string(),
            r#"{"items":[{"volumeInfo":{"title":"X","language":"en","description":"A story."}}]}"#
                .to_string(),
        );
        let f = fetch_isbn("google", "9780140328721", &Mock(m), true).unwrap();
        assert_eq!(f.description.as_deref(), Some("A story."));
    }

    #[test]
    fn unknown_provider_errors() {
        assert!(fetch_isbn("nope", "123", &Mock(HashMap::new()), false).is_err());
    }
}
