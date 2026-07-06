//! Detect and fix common OPF structural defects: duplicate spine itemrefs,
//! empty/legacy metadata, and dangling manifest/spine references.
//!
//! Follows the same two-pass idiom as `meta.rs`: [`plan_repairs`] computes what
//! to drop in a read-only `roxmltree` pass, then [`apply_repairs`] streams the
//! original XML through `quick-xml`, skipping the planned elements and copying
//! everything else byte-for-byte. Repair only removes content, it never adds
//! or rewrites anything — an already-clean OPF comes back unchanged.
//!
//! Scope is OPF-only (`<manifest>`/`<spine>`/`<metadata>`) — content-document
//! link checking (hrefs inside chapter XHTML) is out of scope for this pass.

use crate::util::unquote;
use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};
use std::collections::{HashMap, HashSet};

/// Dublin Core element local names that may appear as direct children of
/// `<metadata>` and are eligible to be dropped when empty.
const DC_ELEMENTS: &[&str] = &[
    "title",
    "creator",
    "contributor",
    "subject",
    "description",
    "publisher",
    "date",
    "rights",
    "identifier",
    "language",
    "source",
    "coverage",
    "relation",
    "format",
    "type",
];

/// Element types the EPUB spec requires at least one of. An empty instance is
/// only dropped if a non-empty sibling of the same type survives — repair
/// must never leave a book with zero titles/identifiers/languages.
const REQUIRED_DC: &[&str] = &["title", "identifier", "language"];

/// Which category of structural defect a [`RepairFinding`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairKind {
    DuplicateSpineItemref,
    EmptyMetadata,
    DanglingManifestItem,
    DanglingSpineItemref,
}

/// One concrete defect found (and fixed).
#[derive(Debug, Clone)]
pub struct RepairFinding {
    pub kind: RepairKind,
    /// Human-readable detail, e.g. `spine itemref idref="c1" (duplicate; kept
    /// first occurrence)` or `dc:date (empty)`.
    pub detail: String,
}

/// Summary of what a repair pass found and fixed.
#[derive(Debug, Default, Clone)]
pub struct RepairReport {
    pub duplicate_spine_itemrefs: usize,
    pub empty_metadata_dropped: usize,
    pub dangling_manifest_items: usize,
    pub dangling_spine_itemrefs: usize,
    pub findings: Vec<RepairFinding>,
}

impl RepairReport {
    /// True when nothing needed fixing.
    pub fn is_clean(&self) -> bool {
        self.duplicate_spine_itemrefs == 0
            && self.empty_metadata_dropped == 0
            && self.dangling_manifest_items == 0
            && self.dangling_spine_itemrefs == 0
    }
}

/// Computed up front so the streaming pass just skips what's planned — mirrors
/// `meta::DropPlan`. Kept private; [`RepairReport`] is the public summary.
#[derive(Debug, Default)]
struct RepairPlan {
    findings: Vec<RepairFinding>,
    dup_spine: usize,
    empty_meta: usize,
    dangling_manifest: usize,
    dangling_spine: usize,
    /// 0-based occurrence index among `<spine><itemref>` children (doc order).
    drop_spine_idx: HashSet<usize>,
    /// Manifest `<item>` `id`s whose `href` doesn't resolve to a real archive entry.
    drop_manifest_ids: HashSet<String>,
    /// 0-based occurrence index among direct `<metadata>` element children (doc order).
    drop_metadata_idx: HashSet<usize>,
    /// `id`s of dropped metadata elements, so a `<meta refines="#id">` pointing
    /// at one of them is cascaded into the drop too.
    drop_refines_ids: HashSet<String>,
}

impl From<RepairPlan> for RepairReport {
    fn from(p: RepairPlan) -> Self {
        RepairReport {
            duplicate_spine_itemrefs: p.dup_spine,
            empty_metadata_dropped: p.empty_meta,
            dangling_manifest_items: p.dangling_manifest,
            dangling_spine_itemrefs: p.dangling_spine,
            findings: p.findings,
        }
    }
}

/// Fix common OPF structural defects. Returns the rewritten OPF XML and a
/// summary of what was found/fixed. If nothing was found, `xml` is returned
/// byte-for-byte unchanged. `opf_entry_name` is the OPF's own zip entry path
/// (e.g. `"OEBPS/content.opf"`, from `container.xml`); `zip_entries` is the
/// full set of archive entry names, used for the dangling-href check.
pub(crate) fn repair_opf(
    xml: &str,
    opf_entry_name: &str,
    zip_entries: &HashSet<String>,
) -> Result<(String, RepairReport)> {
    let plan = plan_repairs(xml, opf_entry_name, zip_entries)?;
    if plan.findings.is_empty() {
        return Ok((xml.to_string(), RepairReport::default()));
    }
    let new_xml = apply_repairs(xml, &plan)?;
    Ok((new_xml, RepairReport::from(plan)))
}

/// Directory prefix of the OPF's own zip entry name (e.g.
/// `"OEBPS/content.opf"` -> `"OEBPS"`; `"content.opf"` -> `""`).
fn opf_dir_of(opf_entry_name: &str) -> &str {
    match opf_entry_name.rfind('/') {
        Some(i) => &opf_entry_name[..i],
        None => "",
    }
}

/// Percent-decode + resolve an OPF-relative `href` against the OPF's own zip
/// directory (`opf_dir`, no trailing slash; `""` if the OPF is at the archive
/// root), normalizing `.`/`..` segments. Returns `None` for a remote
/// (`scheme://`) or `data:` URI — those are never flagged as dangling.
fn resolve_opf_relative(opf_dir: &str, href: &str) -> Option<String> {
    if href.is_empty() || href.contains("://") || href.starts_with("data:") {
        return None;
    }
    // Strip a trailing fragment — manifest hrefs shouldn't carry one, but
    // tolerate it rather than false-positive on a dirty real-world OPF.
    let href = href.split('#').next().unwrap_or(href);
    if href.is_empty() {
        return None;
    }
    let decoded = unquote(href);
    let joined = if opf_dir.is_empty() {
        decoded
    } else {
        format!("{opf_dir}/{decoded}")
    };
    // Stack-based normalize: split on '/', drop '.', pop on '..'. Zip entries
    // are logical '/'-separated paths, not OS paths — do NOT use Path::join.
    let mut stack: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    Some(stack.join("/"))
}

/// Read-only pass building the [`RepairPlan`].
fn plan_repairs(
    xml: &str,
    opf_entry_name: &str,
    zip_entries: &HashSet<String>,
) -> Result<RepairPlan> {
    let doc = roxmltree::Document::parse(xml).context("Failed to parse OPF package document")?;
    let mut plan = RepairPlan::default();
    let opf_dir = opf_dir_of(opf_entry_name);
    let unique_id = doc
        .root_element()
        .attribute("unique-identifier")
        .unwrap_or("");

    // --- Manifest: dangling hrefs ---------------------------------------
    let mut surviving_ids: HashSet<String> = HashSet::new();
    if let Some(manifest) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "manifest")
    {
        for item in manifest
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "item")
        {
            let id = item.attribute("id").unwrap_or("").to_string();
            let href = item.attribute("href").unwrap_or("");
            let dangling = match resolve_opf_relative(opf_dir, href) {
                Some(path) => !zip_entries.contains(&path),
                None => false, // remote/data: URIs are never checked
            };
            if dangling {
                plan.drop_manifest_ids.insert(id.clone());
                plan.dangling_manifest += 1;
                plan.findings.push(RepairFinding {
                    kind: RepairKind::DanglingManifestItem,
                    detail: format!(
                        "manifest item id=\"{id}\" href=\"{href}\" (file not found in archive)"
                    ),
                });
            } else if !id.is_empty() {
                surviving_ids.insert(id);
            }
        }
    }

    // --- Spine: dangling idrefs + duplicates ----------------------------
    if let Some(spine) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "spine")
    {
        let mut seen: HashSet<String> = HashSet::new();
        for (i, itemref) in spine
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "itemref")
            .enumerate()
        {
            let idref = itemref.attribute("idref").unwrap_or("").to_string();
            if !surviving_ids.contains(&idref) {
                plan.drop_spine_idx.insert(i);
                plan.dangling_spine += 1;
                plan.findings.push(RepairFinding {
                    kind: RepairKind::DanglingSpineItemref,
                    detail: format!("spine itemref idref=\"{idref}\" (no matching manifest item)"),
                });
            } else if seen.contains(&idref) {
                plan.drop_spine_idx.insert(i);
                plan.dup_spine += 1;
                plan.findings.push(RepairFinding {
                    kind: RepairKind::DuplicateSpineItemref,
                    detail: format!(
                        "spine itemref idref=\"{idref}\" (duplicate; kept first occurrence)"
                    ),
                });
            } else {
                seen.insert(idref);
            }
        }
    }

    // --- Metadata: empty/legacy dc:* elements ----------------------------
    if let Some(metadata) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")
    {
        // Count non-empty instances of each required type first, so an empty
        // one is only dropped when a valid sibling of the same type survives.
        let mut non_empty_count: HashMap<&str, usize> =
            REQUIRED_DC.iter().map(|n| (*n, 0)).collect();
        for child in metadata.children().filter(|n| n.is_element()) {
            let name = child.tag_name().name();
            if REQUIRED_DC.contains(&name) {
                let empty = child.text().map(|t| t.trim().is_empty()).unwrap_or(true);
                if !empty {
                    *non_empty_count.entry(name).or_insert(0) += 1;
                }
            }
        }

        for (j, child) in metadata.children().filter(|n| n.is_element()).enumerate() {
            let name = child.tag_name().name();
            if !DC_ELEMENTS.contains(&name) {
                continue;
            }
            let empty = child.text().map(|t| t.trim().is_empty()).unwrap_or(true);
            if !empty {
                continue;
            }
            let id = child.attribute("id").unwrap_or("");
            // Never drop the package's own unique-identifier anchor — fixing
            // one defect must not create a dangling `unique-identifier`.
            if name == "identifier" && !unique_id.is_empty() && id == unique_id {
                continue;
            }
            // Required types: only drop if a non-empty sibling survives.
            if REQUIRED_DC.contains(&name) && *non_empty_count.get(name).unwrap_or(&0) == 0 {
                continue;
            }
            plan.drop_metadata_idx.insert(j);
            plan.empty_meta += 1;
            if !id.is_empty() {
                plan.drop_refines_ids.insert(id.to_string());
            }
            plan.findings.push(RepairFinding {
                kind: RepairKind::EmptyMetadata,
                detail: format!("dc:{name} (empty)"),
            });
        }

        // Cascade: a <meta refines="#id"> pointing at a just-dropped element
        // would otherwise be left dangling.
        for (j, child) in metadata.children().filter(|n| n.is_element()).enumerate() {
            if child.tag_name().name() != "meta" || plan.drop_metadata_idx.contains(&j) {
                continue;
            }
            if let Some(refines) = child.attribute("refines") {
                let target = refines.trim_start_matches('#');
                if plan.drop_refines_ids.contains(target) {
                    plan.drop_metadata_idx.insert(j);
                    plan.empty_meta += 1;
                    plan.findings.push(RepairFinding {
                        kind: RepairKind::EmptyMetadata,
                        detail: format!(
                            "meta refines=\"#{target}\" (orphaned by empty-metadata cleanup)"
                        ),
                    });
                }
            }
        }
    }

    Ok(plan)
}

/// Local (namespace-stripped) name of an XML element.
fn local_name(q: QName) -> String {
    let s = String::from_utf8_lossy(q.as_ref());
    match s.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => s.into_owned(),
    }
}

/// Read an attribute's raw value from a start/empty element.
fn get_attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key.as_bytes())
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn item_id_dropped(e: &BytesStart, plan: &RepairPlan) -> bool {
    get_attr(e, "id")
        .map(|id| plan.drop_manifest_ids.contains(&id))
        .unwrap_or(false)
}

/// Stream `xml` through quick-xml, dropping what `plan` marked. No injection
/// step is needed — repair only removes, never adds.
fn apply_repairs(xml: &str, plan: &RepairPlan) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());

    let mut in_manifest = false;
    let mut in_spine = false;
    let mut in_metadata = false;
    let mut spine_idx: usize = 0;
    let mut metadata_idx: usize = 0;
    let mut skip_depth: u32 = 0;

    loop {
        let ev = reader
            .read_event()
            .context("Failed to read OPF while applying repairs")?;

        if matches!(ev, Event::Eof) {
            break;
        }

        // While skipping a dropped element's subtree, every event (nested
        // starts/ends, text, comments, …) is consumed without being written.
        if skip_depth > 0 {
            match &ev {
                Event::Start(_) => skip_depth += 1,
                Event::End(_) => skip_depth -= 1,
                _ => {}
            }
            continue;
        }

        match ev {
            Event::Start(e) => {
                let name = local_name(e.name());
                let drop = match name.as_str() {
                    "manifest" => {
                        in_manifest = true;
                        false
                    }
                    "spine" => {
                        in_spine = true;
                        false
                    }
                    "metadata" => {
                        in_metadata = true;
                        false
                    }
                    "item" if in_manifest => item_id_dropped(&e, plan),
                    "itemref" if in_spine => {
                        let d = plan.drop_spine_idx.contains(&spine_idx);
                        spine_idx += 1;
                        d
                    }
                    _ if in_metadata => {
                        let d = plan.drop_metadata_idx.contains(&metadata_idx);
                        metadata_idx += 1;
                        d
                    }
                    _ => false,
                };
                if drop {
                    skip_depth = 1;
                } else {
                    writer.write_event(Event::Start(e))?;
                }
            }
            Event::Empty(e) => {
                let name = local_name(e.name());
                let drop = match name.as_str() {
                    "item" if in_manifest => item_id_dropped(&e, plan),
                    "itemref" if in_spine => {
                        let d = plan.drop_spine_idx.contains(&spine_idx);
                        spine_idx += 1;
                        d
                    }
                    _ if in_metadata => {
                        let d = plan.drop_metadata_idx.contains(&metadata_idx);
                        metadata_idx += 1;
                        d
                    }
                    _ => false,
                };
                if !drop {
                    writer.write_event(Event::Empty(e))?;
                }
            }
            Event::End(e) => {
                let name = local_name(e.name());
                match name.as_str() {
                    "manifest" => in_manifest = false,
                    "spine" => in_spine = false,
                    "metadata" => in_metadata = false,
                    _ => {}
                }
                writer.write_event(Event::End(e))?;
            }
            other => writer.write_event(other)?,
        }
    }

    Ok(String::from_utf8(writer.into_inner())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPF_CLEAN: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:uuid:1</dc:identifier>
    <dc:title>A Clean Book</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"##;

    fn entries(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    fn base_entries() -> HashSet<String> {
        entries(&["OEBPS/content.opf", "OEBPS/c1.xhtml", "OEBPS/c2.xhtml"])
    }

    #[test]
    fn no_op_on_clean_opf() {
        let zip = base_entries();
        let (xml, report) = repair_opf(OPF_CLEAN, "OEBPS/content.opf", &zip).unwrap();
        assert!(report.is_clean());
        assert_eq!(xml, OPF_CLEAN);
    }

    #[test]
    fn dedupes_spine_itemrefs() {
        let opf = OPF_CLEAN.replace(
            "<itemref idref=\"c2\"/>",
            "<itemref idref=\"c2\"/>\n    <itemref idref=\"c1\"/>",
        );
        let zip = base_entries();
        let (xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.duplicate_spine_itemrefs, 1);
        assert_eq!(xml.matches("idref=\"c1\"").count(), 1);
        assert_eq!(xml.matches("idref=\"c2\"").count(), 1);
    }

    #[test]
    fn drops_empty_and_whitespace_metadata() {
        let opf = OPF_CLEAN.replace(
            "<dc:language>en</dc:language>",
            "<dc:language>en</dc:language>\n    <dc:date></dc:date>\n    <dc:publisher>   </dc:publisher>",
        );
        let zip = base_entries();
        let (xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.empty_metadata_dropped, 2);
        assert!(!xml.contains("dc:date"));
        assert!(!xml.contains("dc:publisher"));
        assert!(xml.contains("<dc:title>A Clean Book</dc:title>"));
    }

    #[test]
    fn never_drops_the_only_title_even_if_empty() {
        let opf = OPF_CLEAN.replace("<dc:title>A Clean Book</dc:title>", "<dc:title></dc:title>");
        let zip = base_entries();
        let (_xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.empty_metadata_dropped, 0);
    }

    #[test]
    fn never_drops_empty_unique_identifier_even_with_other_nonempty_identifier() {
        let opf = OPF_CLEAN.replace(
            "<dc:identifier id=\"bookid\">urn:uuid:1</dc:identifier>",
            "<dc:identifier id=\"bookid\"></dc:identifier>\n    <dc:identifier id=\"isbn\">urn:isbn:9780000000000</dc:identifier>",
        );
        let zip = base_entries();
        let (xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.empty_metadata_dropped, 0);
        assert!(xml.contains("id=\"bookid\""));
    }

    #[test]
    fn drops_dangling_manifest_item() {
        let opf = OPF_CLEAN.replace(
            "<item id=\"c2\" href=\"c2.xhtml\" media-type=\"application/xhtml+xml\"/>",
            "<item id=\"c2\" href=\"c2.xhtml\" media-type=\"application/xhtml+xml\"/>\n    <item id=\"ghost\" href=\"missing.jpg\" media-type=\"image/jpeg\"/>",
        );
        let zip = base_entries();
        let (xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.dangling_manifest_items, 1);
        assert!(!xml.contains("ghost"));
    }

    #[test]
    fn cascades_dangling_manifest_to_spine() {
        let opf = OPF_CLEAN
            .replace(
                "<item id=\"c2\" href=\"c2.xhtml\" media-type=\"application/xhtml+xml\"/>",
                "<item id=\"c2\" href=\"c2.xhtml\" media-type=\"application/xhtml+xml\"/>\n    <item id=\"ghost\" href=\"missing.jpg\" media-type=\"image/jpeg\"/>",
            )
            .replace(
                "<itemref idref=\"c2\"/>",
                "<itemref idref=\"c2\"/>\n    <itemref idref=\"ghost\"/>",
            );
        let zip = base_entries();
        let (xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.dangling_manifest_items, 1);
        assert_eq!(report.dangling_spine_itemrefs, 1);
        assert!(!xml.contains("idref=\"ghost\""));
    }

    #[test]
    fn drops_spine_itemref_never_in_manifest() {
        let opf = OPF_CLEAN.replace(
            "<itemref idref=\"c2\"/>",
            "<itemref idref=\"c2\"/>\n    <itemref idref=\"typo-id\"/>",
        );
        let zip = base_entries();
        let (xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.dangling_spine_itemrefs, 1);
        assert_eq!(report.dangling_manifest_items, 0);
        assert!(!xml.contains("typo-id"));
    }

    #[test]
    fn resolves_dotdot_traversal() {
        assert_eq!(
            resolve_opf_relative("OEBPS", "../shared/cover.png"),
            Some("shared/cover.png".to_string())
        );
        assert_eq!(
            resolve_opf_relative("OEBPS", "images/a.png"),
            Some("OEBPS/images/a.png".to_string())
        );
        assert_eq!(resolve_opf_relative("", "a.png"), Some("a.png".to_string()));
    }

    #[test]
    fn traversal_href_resolving_to_existing_file_is_not_dangling() {
        let opf = OPF_CLEAN.replace(
            "<item id=\"c2\" href=\"c2.xhtml\" media-type=\"application/xhtml+xml\"/>",
            "<item id=\"c2\" href=\"c2.xhtml\" media-type=\"application/xhtml+xml\"/>\n    <item id=\"cov\" href=\"../shared/cover.png\" media-type=\"image/png\"/>",
        );
        let mut zip = base_entries();
        zip.insert("shared/cover.png".to_string());
        let (_xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.dangling_manifest_items, 0);
    }

    #[test]
    fn remote_and_data_hrefs_never_flagged() {
        let opf = OPF_CLEAN.replace(
            "<item id=\"c2\" href=\"c2.xhtml\" media-type=\"application/xhtml+xml\"/>",
            "<item id=\"c2\" href=\"c2.xhtml\" media-type=\"application/xhtml+xml\"/>\n    <item id=\"remote\" href=\"https://example.com/font.ttf\" media-type=\"font/ttf\"/>",
        );
        let zip = base_entries();
        let (xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.dangling_manifest_items, 0);
        assert!(xml.contains("remote"));
    }

    #[test]
    fn drops_refines_meta_of_dropped_empty_dc_element() {
        let opf = OPF_CLEAN.replace(
            "<dc:language>en</dc:language>",
            "<dc:language>en</dc:language>\n    <dc:title id=\"t2\"></dc:title>\n    <meta refines=\"#t2\" property=\"title-type\">subtitle</meta>",
        );
        let zip = base_entries();
        let (xml, report) = repair_opf(&opf, "OEBPS/content.opf", &zip).unwrap();
        assert_eq!(report.empty_metadata_dropped, 2);
        assert!(!xml.contains("t2"));
        assert!(xml.contains("<dc:title>A Clean Book</dc:title>"));
    }
}
