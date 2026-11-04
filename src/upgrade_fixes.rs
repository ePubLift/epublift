//! The epubsana pass that finishes an upgrade.
//!
//! Moving a book from EPUB 2 to EPUB 3 makes some of what EPUB 2 accepted
//! invalid: inline SVG without the manifest's `svg` property, an empty
//! `<title>`, a `Content-Type` meta in its EPUB 2 spelling, an empty
//! `dc:rights`. epubsana repairs these, so the upgrade runs it on the packaged
//! book rather than repairing them itself. The named entities an XHTML 1.x
//! DTD declared are not left to it: the DOCTYPE swap that undeclares them
//! writes them out (`util::resolve_dtd_entities`), because one epubsana fix
//! per document, each followed by a check of the whole book, took 11 minutes
//! on an 805-document book.
//!
//! epubsana's safe fixes are applied as they are everywhere. Of the fixes that
//! need a decision, only [`UPGRADE_FIXES`] are applied: each answers something
//! EPUB 3 requires of a book EPUB 2 accepted, with one obvious answer in that
//! setting. Every other fix that needs a decision is skipped, as `epublift
//! repair` skips it without a terminal.

use anyhow::{Result, anyhow};
use epubsana::{Confirmer, Decision, Goal, Policy, ProposedFix, Workspace};
use std::fs;
use std::path::Path;

/// The fixes needing a decision that the upgrade applies without asking
/// (decided 2026-10-16): an empty `<title>` filled from the book's own table
/// of contents, heading or title; the `Content-Type` meta spelled as EPUB 3
/// requires; an empty optional metadata element dropped; an XHTML file
/// labelled `text/html` declared as `application/xhtml+xml`. Added 2026-10-21:
/// the package's `unique-identifier` attached to the book's one real
/// identifier when the element it names is empty or missing (epubsana declines
/// when there are two candidates). Added 2026-10-22: an `id` that is no longer
/// valid renamed, with every reference to it, because the XHTML 1.x DTD the
/// DOCTYPE swap removes trimmed the spaces from an `ID` attribute and HTML
/// does not (`id="k10 "`).
const UPGRADE_FIXES: &[&str] = &[
    "fix.empty_title",
    "fix.content_type_meta",
    "fix.empty_metadata_element",
    "fix.html_media_type",
    "fix.package_identifier",
    "fix.content_document_invalid_id",
];

/// A pass that changed the book is followed by another, at most this many in
/// all.
const MAX_PASSES: usize = 3;

struct UpgradeConfirmer;

impl Confirmer for UpgradeConfirmer {
    fn decide(&mut self, fix: &ProposedFix) -> Decision {
        if UPGRADE_FIXES.contains(&fix.fix_id) {
            Decision::Approve
        } else {
            Decision::Reject
        }
    }
}

/// Approves every fix, as `epublift repair -y` does.
struct ApproveAll;

impl Confirmer for ApproveAll {
    fn decide(&mut self, _fix: &ProposedFix) -> Decision {
        Decision::Approve
    }
}

/// A book labelled older than EPUB 2 (`version="1.0"`), repaired first, or `None`
/// for any other book. The label stops a validator at the package document, so
/// nothing else in the book has been checked and the upgrade's final pass would
/// be the first to meet what lies behind it. This is what `repair -y` does; the
/// rest of the upgrade then treats the book as the EPUB 2 it is.
pub fn repair_legacy_version(
    input: &Path,
    version: Option<&str>,
    progress: &dyn Fn(&str),
) -> Result<Option<tempfile::NamedTempFile>> {
    let major = version.and_then(|v| v.trim().split('.').next()?.parse::<u32>().ok());
    if !major.is_some_and(|m| m < 2) {
        return Ok(None);
    }
    progress(&format!(
        "[*] This book says it is EPUB {}; repairing it first (epubsana)...",
        version.unwrap_or_default().trim()
    ));
    let bytes = fs::read(input)?;
    let mut ws =
        Workspace::load(&bytes).map_err(|e| anyhow!("epubsana could not open the book: {e}"))?;
    for _ in 0..MAX_PASSES {
        let pass = epubsana::repair(
            &mut ws,
            Goal::Valid,
            Policy::AutoSafeThenAsk,
            &mut ApproveAll,
        )
        .map_err(|e| anyhow!("epubsana could not repair the book: {e}"))?;
        if pass.revealed == 0 || !pass.changed() {
            break;
        }
    }
    let out = ws
        .serialize()
        .map_err(|e| anyhow!("could not write the repaired book: {e}"))?;
    let mut file = tempfile::Builder::new()
        .prefix("epublift_legacy_")
        .suffix(".epub")
        .tempfile()?;
    std::io::Write::write_all(&mut file, &out)?;
    Ok(Some(file))
}

/// Run the pass over the packaged EPUB at `epub`, in place. Returns the number
/// of fixes applied.
pub fn apply(epub: &Path, progress: &dyn Fn(&str)) -> Result<usize> {
    let bytes = fs::read(epub)?;
    let mut ws = Workspace::load(&bytes)
        .map_err(|e| anyhow!("epubsana could not open the upgraded book: {e}"))?;
    let mut applied = 0;
    let mut changed = false;
    for _ in 0..MAX_PASSES {
        let pass = epubsana::repair(
            &mut ws,
            Goal::Valid,
            Policy::AutoSafeThenAsk,
            &mut UpgradeConfirmer,
        )
        .map_err(|e| anyhow!("epubsana could not repair the upgraded book: {e}"))?;
        applied += pass
            .fixes
            .iter()
            .filter(|f| f.outcome == epubsana::Outcome::Applied)
            .count();
        // Plan again after any change, not only when epubsana counts revealed
        // findings: an entity fixed in a document lets the schema check reach
        // it, and its empty <title> shows up only then.
        if !pass.changed() {
            break;
        }
        changed = true;
    }
    if changed {
        let out = ws
            .serialize()
            .map_err(|e| anyhow!("could not write the repaired book: {e}"))?;
        fs::write(epub, out)?;
    }
    progress(&format!(
        "  [+] Applied {applied} EPUB 3 fix(es) with epubsana."
    ));
    Ok(applied)
}
