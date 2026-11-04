//! Parsing and rewriting of the OPF package document.
//!
//! `parse_opf_info` extracts everything we need in a read-only pass, then
//! `rewrite_opf` streams the original XML through quick-xml applying the
//! EPUB 3.3 upgrades while preserving the rest of the document byte-for-byte.

use crate::util;
use anyhow::{Context, Result};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer, XmlVersion};
use std::collections::{HashMap, HashSet};

const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
const NS_DCTERMS: &str = "http://purl.org/dc/terms/";

/// A single `<item>` entry from the OPF manifest.
#[derive(Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
}

/// A `<reference>` entry from the legacy `<guide>` block.
#[derive(Debug, Clone)]
pub struct GuideRef {
    pub ref_type: String,
    pub href: String,
    pub title: String,
}

/// Everything we read out of the OPF in the initial read-only pass.
#[derive(Debug, Default)]
pub struct OpfInfo {
    pub items: Vec<ManifestItem>,
    pub cover_id: Option<String>,
    pub nav_exists: bool,
    pub ncx_href: Option<String>,
    pub has_guide: bool,
    pub guide_refs: Vec<GuideRef>,
    /// The value for an EPUB 3.4 `pageBreakSource` meta property, derived from a
    /// legacy `source-of`/`pagebreak` meta that refines a `dc:source`. `None` if
    /// there is no such legacy meta, or a `pageBreakSource` already exists.
    pub page_break_source: Option<String>,
    /// Human-readable descriptions of EPUB 3.4 "outdated"/deprecated features found
    /// in the package (informational — we report them, but don't strip author
    /// content). See [`detect_outdated_features`].
    pub outdated_features: Vec<String>,
}

/// Describes how a single manifest image item should be rewritten.
#[derive(Debug, Clone)]
pub struct ImageChange {
    pub new_href_encoded: String,
    /// The output media type for the rewritten item (e.g. `image/webp`,
    /// `image/avif`, `image/jxl`).
    pub media_type: String,
    pub is_cover: bool,
}

/// Inputs for the streaming OPF rewrite.
pub struct RewriteParams {
    pub modified_ts: String,
    /// Keyed by the raw (unescaped) `href` attribute as found in the manifest.
    pub manifest_changes: HashMap<String, ImageChange>,
    pub add_nav: bool,
    pub remove_guide: bool,
    /// When `Some`, inject an EPUB 3.4 `<meta property="pageBreakSource">` with
    /// this value (set only for a 3.4 target; see [`OpfInfo::page_break_source`]).
    pub page_break_source: Option<String>,
    /// When `Some`, add this stylesheet (an href relative to the package
    /// document) to the manifest: the CSS the upgrade wrote for the
    /// presentational attributes EPUB 3 no longer allows.
    pub add_stylesheet: Option<String>,
}

/// Local (namespace-stripped) name of an XML element.
fn local(name: QName) -> String {
    let s: String = name.as_ref().to_string();
    match s.rfind(':') {
        Some(i) => s[i + 1..].to_string(),
        None => s,
    }
}

/// Read an attribute's unescaped value from a start/empty element.
fn get_attr(e: &BytesStart, key: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == key {
            return Some(
                a.normalized_value(XmlVersion::Implicit1_0)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| a.value.to_string()),
            );
        }
    }
    None
}

/// Read-only pass over the OPF gathering manifest, cover, navigation and guide
/// information.
pub fn parse_opf_info(xml: &str) -> Result<OpfInfo> {
    let doc = crate::util::parse_xml(xml).context("Failed to parse OPF package document")?;

    // Cover image id: <meta name="cover" content="..."/> anywhere in metadata.
    let cover_id = doc
        .descendants()
        .find(|n| {
            n.is_element() && n.tag_name().name() == "meta" && n.attribute("name") == Some("cover")
        })
        .and_then(|n| n.attribute("content").map(String::from));

    // Manifest items.
    let mut items = Vec::new();
    let mut nav_exists = false;
    let mut ncx_href = None;
    if let Some(manifest) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "manifest")
    {
        for item in manifest
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "item")
        {
            let properties = item.attribute("properties").unwrap_or("");
            let media_type = item.attribute("media-type").unwrap_or("").to_string();
            let href = item.attribute("href").unwrap_or("").to_string();
            let id = item.attribute("id").unwrap_or("").to_string();

            if properties.split_whitespace().any(|p| p == "nav") {
                nav_exists = true;
            }
            if media_type == "application/x-dtbncx+xml" {
                ncx_href = Some(href.clone());
            }

            items.push(ManifestItem {
                id,
                href,
                media_type,
            });
        }
    }

    // Guide / landmarks.
    let mut has_guide = false;
    let mut guide_refs = Vec::new();
    if let Some(guide) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "guide")
    {
        has_guide = true;
        for r in guide
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "reference")
        {
            let ref_type = r.attribute("type").unwrap_or("").to_string();
            let href = r.attribute("href").unwrap_or("").to_string();
            if guide_target_is_not_a_document(&href, &items) {
                continue;
            }
            let title = match r.attribute("title") {
                Some(t) => t.to_string(),
                None => capitalize(&ref_type),
            };
            guide_refs.push(GuideRef {
                ref_type,
                href,
                title,
            });
        }
    }

    // EPUB 3.4 `pageBreakSource`: derive it from a legacy `source-of`/`pagebreak`
    // meta that refines a `dc:source`. Skip if a `pageBreakSource` already exists.
    let page_break_source = derive_page_break_source(&doc);

    let outdated_features = detect_outdated_features(&doc);

    Ok(OpfInfo {
        items,
        cover_id,
        nav_exists,
        ncx_href,
        has_guide,
        guide_refs,
        page_break_source,
        outdated_features,
    })
}

/// Detect EPUB 3.4 "outdated"/deprecated features present in the package document
/// (a lint — we report them, we don't strip author content). Covers the reliably
/// Whether a guide `href` names a manifest item that is no document, such as a
/// cover image: EPUB 2 reports it, and EPUB 3 forbids it as a hyperlink, so it
/// makes no landmark. `text/html` counts as a document, because the upgrade's
/// final pass declares it as XHTML. A target the manifest lacks is kept: the
/// broken link is the book's and stays reported.
fn guide_target_is_not_a_document(href: &str, items: &[ManifestItem]) -> bool {
    let path = util::unquote(href.split('#').next().unwrap_or(""));
    items
        .iter()
        .find(|item| util::unquote(&item.href) == path)
        .is_some_and(|item| {
            !matches!(
                item.media_type.as_str(),
                "application/xhtml+xml" | "image/svg+xml" | "text/html"
            )
        })
}

/// OPF-detectable signals: manifest content fallbacks, the outdated `rendition:*`
/// properties, the `collection` element, and the deprecated reserved prefixes
/// (`xsd`/`msv`/`prism`).
fn detect_outdated_features(doc: &roxmltree::Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };

    for n in doc.descendants().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            // Manifest item with a content `fallback` (outdated in 3.4).
            "item" if n.has_attribute("fallback") => {
                push("manifest content fallback (`fallback` attribute)".into());
            }
            // Outdated rendition properties on <meta property="...">.
            "meta" => {
                if let Some(p) = n.attribute("property")
                    && matches!(
                        p,
                        "rendition:flow"
                            | "rendition:orientation"
                            | "rendition:spread"
                            | "rendition:align-x-center"
                    )
                {
                    push(format!("outdated rendition property `{p}`"));
                }
            }
            // The <collection> element (obsolete but conforming).
            "collection" => push("`collection` element".into()),
            _ => {}
        }
    }

    // Deprecated reserved prefixes declared on <package prefix="name: uri ...">.
    if let Some(pkg) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "package")
        && let Some(prefix) = pkg.attribute("prefix")
    {
        for tok in prefix.split_whitespace() {
            if let Some(name) = tok.strip_suffix(':')
                && matches!(name, "xsd" | "msv" | "prism")
            {
                push(format!("deprecated reserved prefix `{name}`"));
            }
        }
    }

    out
}

/// Resolve the value for an EPUB 3.4 `pageBreakSource` meta from a legacy
/// `<meta refines="#id" property="source-of">pagebreak</meta>` whose `refines`
/// target is a `dc:source`. Returns `None` if absent, unresolvable, or if a
/// `pageBreakSource` meta is already present (don't duplicate).
fn derive_page_break_source(doc: &roxmltree::Document) -> Option<String> {
    let is_meta = |n: &roxmltree::Node| n.is_element() && n.tag_name().name() == "meta";

    // Already modernized? Then there's nothing to add.
    if doc
        .descendants()
        .any(|n| is_meta(&n) && n.attribute("property") == Some("pageBreakSource"))
    {
        return None;
    }

    // Find the source-of/pagebreak meta and its refines target id.
    let refines_id = doc.descendants().find_map(|n| {
        if is_meta(&n)
            && n.attribute("property") == Some("source-of")
            && n.text().map(|t| t.trim().eq_ignore_ascii_case("pagebreak")) == Some(true)
        {
            n.attribute("refines")
                .map(|r| r.trim_start_matches('#').to_string())
        } else {
            None
        }
    })?;

    // Resolve it to a dc:source element and read its value.
    doc.descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "source"
                && n.attribute("id") == Some(refines_id.as_str())
        })
        .and_then(|n| n.text())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Capitalize the first character and lowercase the rest (Python `str.capitalize`).
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Build a new start element from `e`, applying value overrides and adding
/// any `add_if_missing` attributes not already present. Element name (including
/// any namespace prefix) and attribute order are preserved.
fn transform_start(
    e: &BytesStart,
    overrides: &[(String, String)],
    add_if_missing: &[(String, String)],
) -> BytesStart<'static> {
    let name = e.name().as_ref().to_string();
    let mut new = BytesStart::new(name);
    let mut present: HashSet<String> = HashSet::new();

    for a in e.attributes().flatten() {
        let key = a.key.as_ref().to_string();
        present.insert(key.clone());
        let value = a
            .normalized_value(XmlVersion::Implicit1_0)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| a.value.to_string());
        let value = overrides
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or(value);
        new.push_attribute((key.as_str(), value.as_str()));
    }

    for (k, v) in overrides {
        if !present.contains(k) {
            new.push_attribute((k.as_str(), v.as_str()));
        }
    }
    for (k, v) in add_if_missing {
        if !present.contains(k) {
            new.push_attribute((k.as_str(), v.as_str()));
        }
    }
    new
}

/// Construct the `<meta property="dcterms:modified">` element pieces for `ts`.
fn write_modified_meta<W: std::io::Write>(
    writer: &mut Writer<W>,
    prefix: &str,
    ts: &str,
) -> Result<()> {
    writer.write_event(Event::Text(BytesText::from_escaped("\n    ")))?;
    let meta = format!("{prefix}meta");
    let mut m = BytesStart::new(meta.as_str());
    m.push_attribute(("property", "dcterms:modified"));
    writer.write_event(Event::Start(m))?;
    writer.write_event(Event::Text(BytesText::new(ts)))?;
    writer.write_event(Event::End(BytesEnd::new(meta)))?;
    Ok(())
}

/// Write an EPUB 3.4 `<meta property="pageBreakSource">value</meta>` element.
fn write_page_break_source_meta<W: std::io::Write>(
    writer: &mut Writer<W>,
    prefix: &str,
    value: &str,
) -> Result<()> {
    writer.write_event(Event::Text(BytesText::from_escaped("\n    ")))?;
    let meta = format!("{prefix}meta");
    let mut m = BytesStart::new(meta.as_str());
    m.push_attribute(("property", "pageBreakSource"));
    writer.write_event(Event::Start(m))?;
    writer.write_event(Event::Text(BytesText::new(value)))?;
    writer.write_event(Event::End(BytesEnd::new(meta)))?;
    Ok(())
}

/// The OPF 2 attributes on metadata elements that EPUB 3 expresses as
/// `<meta refines>` instead (`opf:role`, `opf:file-as`, `opf:scheme`,
/// `opf:event`). Returns the local name for such an attribute key.
fn legacy_attr(key: &str) -> Option<&str> {
    let (prefix, name) = key.split_once(':')?;
    if matches!(prefix, "xml" | "xmlns" | "xsi") {
        return None;
    }
    matches!(name, "role" | "file-as" | "scheme" | "event").then_some(name)
}

/// How a `dc:date` survives the move to EPUB 3, which allows exactly one.
fn date_rank(event: Option<&str>) -> u8 {
    match event.map(str::trim) {
        Some("publication" | "original-publication") => 0,
        None | Some("") => 1,
        Some("creation") => 2,
        Some("modification") => 4,
        Some(_) => 3,
    }
}

/// A metadata element with OPF 2 attributes, held back until its end tag shows
/// whether it is empty.
struct HeldElement {
    start: BytesStart<'static>,
    inner: Vec<Event<'static>>,
    depth: usize,
}

impl HeldElement {
    /// Empty as XML counts it: no content but XML whitespace (a no-break
    /// space or a character reference is content).
    fn is_blank(&self) -> bool {
        self.inner.iter().all(|e| match e {
            Event::Text(t) => t.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n')),
            Event::CData(c) => c.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n')),
            Event::GeneralRef(_) => false,
            _ => true,
        })
    }
}

/// What the EPUB 2 → 3 metadata translation must know before streaming: the
/// ids already in use, and which `dc:date` (by position) stays a `dc:date`.
struct LegacyScan {
    ids: HashSet<String>,
    primary_date: Option<usize>,
}

fn scan_legacy(xml: &str) -> Result<LegacyScan> {
    let doc = crate::util::parse_xml(xml).context("Failed to parse OPF package document")?;
    let ids = doc
        .descendants()
        .filter_map(|n| n.attribute("id"))
        .map(str::to_string)
        .collect();
    let primary_date = doc
        .descendants()
        .filter(|n| n.tag_name().namespace() == Some(NS_DC) && n.tag_name().name() == "date")
        .enumerate()
        .min_by_key(|(i, n)| {
            let event = n
                .attributes()
                .find(|a| a.name() == "event" && a.namespace().is_some())
                .map(|a| a.value());
            (date_rank(event), *i)
        })
        .map(|(i, _)| i);
    Ok(LegacyScan { ids, primary_date })
}

/// A `<meta>` the translation adds at the end of `<metadata>`.
struct AddedMeta {
    refines: Option<String>,
    property: &'static str,
    scheme: Option<&'static str>,
    value: String,
}

/// Strip the OPF 2 attributes from a metadata element and queue their EPUB 3
/// `refines` equivalents. Gives the element an id when it needs one. An empty
/// element (`blank`) only loses the attributes: a refinement would describe
/// nothing, and would keep epubsana from dropping the element.
fn translate_legacy(
    e: &BytesStart,
    blank: bool,
    ids: &mut HashSet<String>,
    next_id: &mut usize,
    added: &mut Vec<AddedMeta>,
) -> BytesStart<'static> {
    let is_identifier = local(e.name()) == "identifier";
    let mut legacy: Vec<(&'static str, String)> = Vec::new();
    let mut new = BytesStart::new(e.name().as_ref().to_string());
    let mut id = None;
    for a in e.attributes().flatten() {
        let key = a.key.as_ref().to_string();
        let value = a
            .normalized_value(XmlVersion::Implicit1_0)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| a.value.to_string());
        match legacy_attr(&key) {
            // `opf:event` is handled by the dc:date logic; elsewhere it has no
            // EPUB 3 form.
            Some("event") => {}
            Some("role") => legacy.push(("role", value)),
            Some("file-as") => legacy.push(("file-as", value)),
            Some("scheme") if is_identifier => legacy.push(("identifier-type", value)),
            Some(_) => {}
            None => {
                if key == "id" {
                    id = Some(value.clone());
                }
                new.push_attribute((key.as_str(), value.as_str()));
            }
        }
    }
    legacy.retain(|(_, v)| !v.trim().is_empty());
    if blank || legacy.is_empty() {
        return new;
    }
    let id = id.unwrap_or_else(|| {
        loop {
            *next_id += 1;
            let candidate = format!("epublift-meta-{next_id}");
            if ids.insert(candidate.clone()) {
                new.push_attribute(("id", candidate.as_str()));
                break candidate;
            }
        }
    });
    for (property, value) in legacy {
        let value = value.trim().to_string();
        let scheme = (property == "role"
            && value.len() == 3
            && value.bytes().all(|b| b.is_ascii_lowercase()))
        .then_some("marc:relators");
        added.push(AddedMeta {
            refines: Some(format!("#{id}")),
            property,
            scheme,
            value,
        });
    }
    new
}

fn write_added_meta<W: std::io::Write>(
    writer: &mut Writer<W>,
    prefix: &str,
    m: &AddedMeta,
) -> Result<()> {
    writer.write_event(Event::Text(BytesText::from_escaped("\n    ")))?;
    let meta = format!("{prefix}meta");
    let mut e = BytesStart::new(meta.as_str());
    if let Some(r) = &m.refines {
        e.push_attribute(("refines", r.as_str()));
    }
    e.push_attribute(("property", m.property));
    if let Some(s) = m.scheme {
        e.push_attribute(("scheme", s));
    }
    writer.write_event(Event::Start(e))?;
    writer.write_event(Event::Text(BytesText::new(&m.value)))?;
    writer.write_event(Event::End(BytesEnd::new(meta)))?;
    Ok(())
}

/// The EPUB 3 property for a `dc:date` that cannot stay one, or `None` when
/// it is dropped (a modification date: `dcterms:modified` replaces it).
fn secondary_date_property(event: Option<&str>) -> Option<&'static str> {
    match event.map(str::trim) {
        Some("modification") => None,
        Some("creation") => Some("dcterms:created"),
        Some("publication" | "original-publication") => Some("dcterms:issued"),
        _ => Some("dcterms:date"),
    }
}

/// The `opf:event` of a `dc:date` start tag.
fn date_event(e: &BytesStart) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let key = a.key.as_ref().to_string();
        (legacy_attr(&key) == Some("event")).then(|| a.value.to_string())
    })
}

/// Does this metadata element carry an OPF 2 attribute EPUB 3 does not allow?
fn has_legacy_attr(e: &BytesStart) -> bool {
    e.attributes()
        .flatten()
        .any(|a| legacy_attr(a.key.as_ref()).is_some())
}

/// A `dc:date` (or other DC element) start tag inside `<metadata>`.
fn is_dc(e: &BytesStart, name: &str) -> bool {
    let q = e.name();
    let q = q.as_ref();
    local(e.name()) == name && q.contains(':') && !q.starts_with("opf:")
}

/// Stream the OPF through, applying the EPUB 3.3 upgrades.
pub fn rewrite_opf(xml: &str, params: &RewriteParams) -> Result<String> {
    let LegacyScan {
        mut ids,
        primary_date,
    } = scan_legacy(xml)?;
    let mut next_id = 0usize;
    let mut added: Vec<AddedMeta> = Vec::new();
    let mut date_index = 0usize;
    // A secondary dc:date being dropped: its property (None = discard) and text.
    let mut moving_date: Option<(Option<&'static str>, String)> = None;
    let mut held: Option<HeldElement> = None;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());

    let mut in_metadata = false;
    let mut in_manifest = false;
    let mut metadata_seen = false;
    let mut skip_until_end_meta = false;
    let mut skipping_guide = false;
    let mut guide_depth = 0i32;
    // The prefix the package element uses for the OPF namespace (`ns0:` in
    // some converters' output); every element we add must carry it too.
    let mut prefix = String::new();

    loop {
        let ev = reader.read_event()?;

        // Drop an existing <meta property="dcterms:modified"> element entirely.
        if skip_until_end_meta {
            if let Event::End(e) = &ev
                && local(e.name()) == "meta"
            {
                skip_until_end_meta = false;
            }
            continue;
        }

        // A secondary dc:date: collect its text, then move it to a meta (or
        // drop it) at its end tag.
        if let Some((property, text)) = &mut moving_date {
            match &ev {
                Event::Text(t) => text.push_str(&t.xml_content(XmlVersion::Implicit1_0)),
                Event::GeneralRef(r) => {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        text.push(c);
                    } else {
                        let name: &str = r.as_ref();
                        text.push_str(match name {
                            "amp" => "&",
                            "lt" => "<",
                            "gt" => ">",
                            "quot" => "\"",
                            "apos" => "'",
                            _ => "",
                        });
                    }
                }
                Event::End(e) if local(e.name()) == "date" => {
                    if let Some(property) = property
                        && !text.trim().is_empty()
                    {
                        added.push(AddedMeta {
                            refines: None,
                            property,
                            scheme: None,
                            value: text.trim().to_string(),
                        });
                    }
                    moving_date = None;
                }
                _ => {}
            }
            continue;
        }

        // A held metadata element: collect its content, then translate it at
        // its end tag, knowing whether it is empty.
        if let Some(h) = &mut held {
            match &ev {
                Event::Start(_) => h.depth += 1,
                Event::End(_) if h.depth > 0 => h.depth -= 1,
                Event::Eof => anyhow::bail!("OPF package document ends inside <metadata>"),
                Event::End(_) => {
                    let h = held.take().expect("held element");
                    let new = translate_legacy(
                        &h.start,
                        h.is_blank(),
                        &mut ids,
                        &mut next_id,
                        &mut added,
                    );
                    writer.write_event(Event::Start(new))?;
                    for e in h.inner {
                        writer.write_event(e)?;
                    }
                    writer.write_event(ev)?;
                    continue;
                }
                _ => {}
            }
            h.inner.push(ev.into_owned());
            continue;
        }

        // Drop the legacy <guide> subtree entirely.
        if skipping_guide {
            match &ev {
                Event::Start(_) => guide_depth += 1,
                Event::End(_) => {
                    guide_depth -= 1;
                    if guide_depth == 0 {
                        skipping_guide = false;
                    }
                }
                _ => {}
            }
            continue;
        }

        match ev {
            Event::Eof => break,

            Event::Start(e) => {
                let ln = local(e.name());
                match ln.as_str() {
                    "package" => {
                        prefix = opf_prefix(&e);
                        let new = transform_start(&e, &[("version".into(), "3.0".into())], &[]);
                        writer.write_event(Event::Start(new))?;
                    }
                    "metadata" => {
                        metadata_seen = true;
                        in_metadata = true;
                        let new = transform_start(
                            &e,
                            &[],
                            &[
                                ("xmlns:dc".into(), NS_DC.into()),
                                ("xmlns:dcterms".into(), NS_DCTERMS.into()),
                            ],
                        );
                        writer.write_event(Event::Start(new))?;
                    }
                    "manifest" => {
                        in_manifest = true;
                        writer.write_event(Event::Start(e))?;
                    }
                    "guide" if params.remove_guide => {
                        skipping_guide = true;
                        guide_depth = 1;
                    }
                    "meta" if in_metadata && is_modified_meta(&e) => {
                        skip_until_end_meta = true;
                    }
                    "item" if in_manifest => {
                        let new = rewrite_item(&e, params);
                        writer.write_event(Event::Start(new))?;
                    }
                    "date" if in_metadata && is_dc(&e, "date") => {
                        let index = date_index;
                        date_index += 1;
                        if Some(index) == primary_date {
                            held = Some(HeldElement {
                                start: e.into_owned(),
                                inner: Vec::new(),
                                depth: 0,
                            });
                        } else {
                            let event = date_event(&e);
                            moving_date =
                                Some((secondary_date_property(event.as_deref()), String::new()));
                        }
                    }
                    _ if in_metadata && has_legacy_attr(&e) => {
                        held = Some(HeldElement {
                            start: e.into_owned(),
                            inner: Vec::new(),
                            depth: 0,
                        });
                    }
                    _ => writer.write_event(Event::Start(e))?,
                }
            }

            Event::Empty(e) => {
                let ln = local(e.name());
                match ln.as_str() {
                    "guide" if params.remove_guide => { /* skip self-closing guide */ }
                    "meta" if in_metadata && is_modified_meta(&e) => { /* drop */ }
                    "item" if in_manifest => {
                        let new = rewrite_item(&e, params);
                        writer.write_event(Event::Empty(new))?;
                    }
                    "date" if in_metadata && is_dc(&e, "date") => {
                        let index = date_index;
                        date_index += 1;
                        if Some(index) == primary_date {
                            let new =
                                translate_legacy(&e, true, &mut ids, &mut next_id, &mut added);
                            writer.write_event(Event::Empty(new))?;
                        }
                    }
                    _ if in_metadata && has_legacy_attr(&e) => {
                        let new = translate_legacy(&e, true, &mut ids, &mut next_id, &mut added);
                        writer.write_event(Event::Empty(new))?;
                    }
                    _ => writer.write_event(Event::Empty(e))?,
                }
            }

            Event::End(e) => {
                let ln = local(e.name());
                match ln.as_str() {
                    "metadata" => {
                        write_modified_meta(&mut writer, &prefix, &params.modified_ts)?;
                        if let Some(src) = &params.page_break_source {
                            write_page_break_source_meta(&mut writer, &prefix, src)?;
                        }
                        for m in &added {
                            write_added_meta(&mut writer, &prefix, m)?;
                        }
                        writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        in_metadata = false;
                        writer.write_event(Event::End(e))?;
                    }
                    "manifest" => {
                        if params.add_nav {
                            writer.write_event(Event::Text(BytesText::from_escaped("\n    ")))?;
                            let item = format!("{prefix}item");
                            let mut it = BytesStart::new(item.as_str());
                            it.push_attribute(("id", "nav"));
                            it.push_attribute(("href", "nav.xhtml"));
                            it.push_attribute(("media-type", "application/xhtml+xml"));
                            it.push_attribute(("properties", "nav"));
                            writer.write_event(Event::Empty(it))?;
                            writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        }
                        if let Some(href) = &params.add_stylesheet {
                            let id = (1..)
                                .map(|n| match n {
                                    1 => "epublift-presentational-css".to_string(),
                                    n => format!("epublift-presentational-css-{n}"),
                                })
                                .find(|id| !ids.contains(id))
                                .unwrap_or_default();
                            ids.insert(id.clone());
                            writer.write_event(Event::Text(BytesText::from_escaped("\n    ")))?;
                            let item = format!("{prefix}item");
                            let mut it = BytesStart::new(item.as_str());
                            it.push_attribute(("id", id.as_str()));
                            it.push_attribute(("href", href.as_str()));
                            it.push_attribute(("media-type", "text/css"));
                            writer.write_event(Event::Empty(it))?;
                            writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        }
                        in_manifest = false;
                        writer.write_event(Event::End(e))?;
                    }
                    "package" if !metadata_seen => {
                        // Extremely rare: OPF had no <metadata>. Inject one.
                        let metadata = format!("{prefix}metadata");
                        let mut md = BytesStart::new(metadata.as_str());
                        md.push_attribute(("xmlns:dc", NS_DC));
                        md.push_attribute(("xmlns:dcterms", NS_DCTERMS));
                        writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        writer.write_event(Event::Start(md))?;
                        write_modified_meta(&mut writer, &prefix, &params.modified_ts)?;
                        writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        writer.write_event(Event::End(BytesEnd::new(metadata)))?;
                        writer.write_event(Event::Text(BytesText::from_escaped("\n")))?;
                        metadata_seen = true;
                        writer.write_event(Event::End(e))?;
                    }
                    _ => writer.write_event(Event::End(e))?,
                }
            }

            other => writer.write_event(other)?,
        }
    }

    Ok(String::from_utf8(writer.into_inner())?)
}

/// The element-name prefix of `package` including its colon (`"ns0:"`), or
/// `""` when the OPF namespace is the default one.
fn opf_prefix(package: &BytesStart) -> String {
    match package.name().prefix() {
        Some(p) => format!("{}:", p.as_ref()),
        None => String::new(),
    }
}

/// Does this `<meta>` element carry `property="dcterms:modified"`?
fn is_modified_meta(e: &BytesStart) -> bool {
    get_attr(e, "property").as_deref() == Some("dcterms:modified")
}

/// Apply the WebP rewrite to a manifest `<item>` if its href is being converted.
fn rewrite_item(e: &BytesStart, params: &RewriteParams) -> BytesStart<'static> {
    let href = get_attr(e, "href").unwrap_or_default();
    match params.manifest_changes.get(&href) {
        Some(change) => {
            let mut overrides = vec![
                ("href".to_string(), change.new_href_encoded.clone()),
                ("media-type".to_string(), change.media_type.clone()),
            ];
            if change.is_cover {
                overrides.push(("properties".to_string(), "cover-image".to_string()));
            }
            transform_start(e, &overrides, &[])
        }
        None => {
            // Font media-type hygiene: a `.ttf` item carrying a legacy/non-core
            // media type (or none) gets `font/ttf` — the modern core type valid
            // in EPUB 3.3 *and* 3.4. (`application/x-font-ttf`, common in older
            // EPUBs, was non-core before 3.4; normalizing fixes 3.3 conformance
            // and is clean under 3.4.) Set both the type and, if it was missing,
            // add it.
            if href_is_ttf(&href) && needs_ttf_media_type(get_attr(e, "media-type").as_deref()) {
                let mt = ("media-type".to_string(), "font/ttf".to_string());
                transform_start(e, std::slice::from_ref(&mt), std::slice::from_ref(&mt))
            } else {
                transform_start(e, &[], &[])
            }
        }
    }
}

/// Whether an href points to a TrueType font (by `.ttf` extension).
fn href_is_ttf(href: &str) -> bool {
    href.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ttf"))
}

/// Whether a `.ttf` item's current media type should be normalized to `font/ttf`.
/// Leaves the already-core TTF types (`font/ttf`, `application/font-sfnt`) alone;
/// rewrites the known legacy/wrong/missing ones.
fn needs_ttf_media_type(media_type: Option<&str>) -> bool {
    match media_type.map(str::trim) {
        None | Some("") => true,
        Some("font/ttf") | Some("application/font-sfnt") => false,
        Some(other) => matches!(
            other,
            "application/x-font-ttf"
                | "application/x-font-truetype"
                | "application/x-truetype-font"
                | "application/octet-stream"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guide_entry_for_an_image_makes_no_landmark() {
        let info = parse_opf_info(
            r#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><manifest>
              <item id="c" href="image/cover.jpeg" media-type="image/jpeg"/>
              <item id="x" href="a%20b.xhtml" media-type="application/xhtml+xml"/>
              <item id="h" href="c.htm" media-type="text/html"/>
            </manifest><guide>
              <reference type="cover" title="Cover" href="image/cover.jpeg"/>
              <reference type="text" title="Text" href="a b.xhtml#start"/>
              <reference type="toc" title="Contents" href="c.htm"/>
              <reference type="notes" title="Notes" href="missing.xhtml"/>
            </guide></package>"#,
        )
        .unwrap();
        assert!(info.has_guide);
        let kept: Vec<&str> = info
            .guide_refs
            .iter()
            .map(|r| r.ref_type.as_str())
            .collect();
        assert_eq!(kept, ["text", "toc", "notes"]);
    }

    const OPF_PAGEBREAK: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">urn:uuid:1</dc:identifier>
    <dc:title>T</dc:title>
    <dc:language>en</dc:language>
    <dc:source id="src">urn:isbn:9780375704024</dc:source>
    <meta refines="#src" property="source-of">pagebreak</meta>
  </metadata>
  <manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="c"/></spine>
</package>"##;

    fn params(pbs: Option<String>) -> RewriteParams {
        RewriteParams {
            modified_ts: "2026-01-01T00:00:00Z".into(),
            manifest_changes: HashMap::new(),
            add_nav: false,
            remove_guide: false,
            page_break_source: pbs,
            add_stylesheet: None,
        }
    }

    const OPF_EPUB2: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uuid_id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="uuid_id" opf:scheme="uuid">urn:uuid:1</dc:identifier>
    <dc:identifier opf:scheme="ISBN">9780000000000</dc:identifier>
    <dc:title>T</dc:title>
    <dc:creator opf:role="aut" opf:file-as="Doe, Jane">Jane Doe</dc:creator>
    <dc:contributor opf:role="Çeviren">A &amp; B</dc:contributor>
    <dc:date>1999-08-02T21:00:00+00:00</dc:date>
    <dc:date opf:event="modification">2013-12-06</dc:date>
    <dc:date opf:event="creation">1990</dc:date>
    <dc:language>tr</dc:language>
  </metadata>
  <manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="c"/></spine>
</package>"##;

    #[test]
    fn translates_opf2_metadata_attributes_to_refines() {
        let out = rewrite_opf(OPF_EPUB2, &params(None)).unwrap();
        assert!(!out.contains("opf:role") && !out.contains("opf:file-as"));
        assert!(!out.contains("opf:scheme") && !out.contains("opf:event"));
        assert!(
            out.contains(r##"<meta refines="#uuid_id" property="identifier-type">uuid</meta>"##)
        );
        assert!(
            out.contains(r##"<dc:identifier id="epublift-meta-1">9780000000000</dc:identifier>"##)
        );
        assert!(out.contains(
            r##"<meta refines="#epublift-meta-1" property="identifier-type">ISBN</meta>"##
        ));
        assert!(out.contains(r##"<dc:creator id="epublift-meta-2">Jane Doe</dc:creator>"##));
        assert!(out.contains(
            r##"<meta refines="#epublift-meta-2" property="role" scheme="marc:relators">aut</meta>"##
        ));
        assert!(
            out.contains(
                r##"<meta refines="#epublift-meta-2" property="file-as">Doe, Jane</meta>"##
            )
        );
        // Not a MARC code: kept as a role, without claiming the scheme.
        assert!(
            out.contains(r##"<meta refines="#epublift-meta-3" property="role">Çeviren</meta>"##)
        );
        assert!(out.contains("A &amp; B"));
        // One dc:date stays; the modification date gives way to dcterms:modified.
        assert_eq!(out.matches("<dc:date").count(), 1);
        assert!(out.contains("<dc:date>1999-08-02T21:00:00+00:00</dc:date>"));
        assert!(!out.contains("2013-12-06"));
        assert!(out.contains(r##"<meta property="dcterms:created">1990</meta>"##));
        crate::util::parse_xml(&out).unwrap();
    }

    #[test]
    fn an_empty_element_loses_its_opf2_attributes_without_a_refinement() {
        let opf = OPF_EPUB2
            .replace(
                r##"<dc:identifier id="uuid_id" opf:scheme="uuid">urn:uuid:1</dc:identifier>"##,
                r##"<dc:identifier id="uuid_id" opf:scheme="uuid"/>
    <dc:identifier opf:scheme="UUID">urn:uuid:1</dc:identifier>"##,
            )
            .replace(
                r##"<dc:contributor opf:role="Çeviren">A &amp; B</dc:contributor>"##,
                r##"<dc:contributor opf:role="trl" opf:file-as="X"> </dc:contributor>"##,
            );
        let out = rewrite_opf(&opf, &params(None)).unwrap();
        assert!(out.contains(r##"<dc:identifier id="uuid_id"/>"##));
        assert!(!out.contains(r##"refines="#uuid_id""##));
        assert!(out.contains("<dc:contributor> </dc:contributor>"));
        assert!(!out.contains(">trl<") && !out.contains(">X<"));
        // A non-empty sibling is still translated.
        assert!(out.contains(r##"property="identifier-type">UUID</meta>"##));
        crate::util::parse_xml(&out).unwrap();
    }

    #[test]
    fn added_elements_take_the_package_namespace_prefix() {
        let opf = r##"<ns0:package unique-identifier="u" version="2.0" xmlns:ns0="http://www.idpf.org/2007/opf">
  <ns0:metadata>
    <ns1:identifier id="u" ns0:scheme="uuid" xmlns:ns1="http://purl.org/dc/elements/1.1/">urn:uuid:1</ns1:identifier>
  </ns0:metadata>
  <ns0:manifest><ns0:item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></ns0:manifest>
  <ns0:spine><ns0:itemref idref="c"/></ns0:spine>
</ns0:package>"##;
        let mut p = params(Some("urn:isbn:1".into()));
        p.add_nav = true;
        let out = rewrite_opf(opf, &p).unwrap();
        assert!(
            out.contains(r##"<ns0:meta property="dcterms:modified">"##),
            "{out}"
        );
        assert!(
            out.contains(r##"<ns0:meta property="pageBreakSource">"##),
            "{out}"
        );
        assert!(
            out.contains(r##"<ns0:meta refines="#u" property="identifier-type">uuid</ns0:meta>"##)
        );
        assert!(out.contains(r##"<ns0:item id="nav" href="nav.xhtml""##));
        assert!(!out.contains("<meta ") && !out.contains("<item "));
        crate::util::parse_xml(&out).unwrap();
    }

    #[test]
    fn a_publication_date_wins_over_an_undated_one() {
        let opf = OPF_EPUB2.replace(
            r##"<dc:date opf:event="creation">1990</dc:date>"##,
            r##"<dc:date opf:event="publication">2001</dc:date>"##,
        );
        let out = rewrite_opf(&opf, &params(None)).unwrap();
        assert!(out.contains("<dc:date>2001</dc:date>"));
        assert!(
            out.contains(r##"<meta property="dcterms:date">1999-08-02T21:00:00+00:00</meta>"##)
        );
    }

    #[test]
    fn derives_page_break_source_from_legacy_meta() {
        let info = parse_opf_info(OPF_PAGEBREAK).unwrap();
        assert_eq!(
            info.page_break_source.as_deref(),
            Some("urn:isbn:9780375704024")
        );
    }

    #[test]
    fn rewrite_injects_page_break_source_and_keeps_legacy() {
        let info = parse_opf_info(OPF_PAGEBREAK).unwrap();
        let out = rewrite_opf(OPF_PAGEBREAK, &params(info.page_break_source)).unwrap();
        assert!(out.contains(r#"property="pageBreakSource""#));
        assert!(out.contains("urn:isbn:9780375704024"));
        // Legacy source-of is kept for backward compatibility.
        assert!(out.contains(r#"property="source-of""#));
    }

    #[test]
    fn no_injection_when_target_is_not_34() {
        // Simulates a 3.3 target: page_break_source is None, so nothing is added.
        let out = rewrite_opf(OPF_PAGEBREAK, &params(None)).unwrap();
        assert!(!out.contains("pageBreakSource"));
    }

    #[test]
    fn skips_derivation_when_already_present() {
        let opf = OPF_PAGEBREAK.replace(
            "</metadata>",
            r#"  <meta property="pageBreakSource">urn:isbn:9780375704024</meta>
  </metadata>"#,
        );
        let info = parse_opf_info(&opf).unwrap();
        assert_eq!(info.page_break_source, None);
    }

    #[test]
    fn none_when_no_legacy_meta() {
        let opf = OPF_PAGEBREAK.replace(
            r##"<meta refines="#src" property="source-of">pagebreak</meta>"##,
            "",
        );
        let info = parse_opf_info(&opf).unwrap();
        assert_eq!(info.page_break_source, None);
    }

    #[test]
    fn ttf_media_type_decision() {
        assert!(needs_ttf_media_type(Some("application/x-font-ttf")));
        assert!(needs_ttf_media_type(Some("application/octet-stream")));
        assert!(needs_ttf_media_type(None));
        assert!(needs_ttf_media_type(Some("")));
        // Already-core types are left alone.
        assert!(!needs_ttf_media_type(Some("font/ttf")));
        assert!(!needs_ttf_media_type(Some("application/font-sfnt")));
    }

    #[test]
    fn href_ttf_detection() {
        assert!(href_is_ttf("fonts/Foo.ttf"));
        assert!(href_is_ttf("Foo.TTF"));
        assert!(!href_is_ttf("fonts/Foo.otf"));
        assert!(!href_is_ttf("Foo.woff2"));
    }

    #[test]
    fn rewrite_normalizes_legacy_ttf_media_type() {
        let opf = OPF_PAGEBREAK.replace(
            r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#,
            r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/><item id="f" href="fonts/F.ttf" media-type="application/x-font-ttf"/>"#,
        );
        let out = rewrite_opf(&opf, &params(None)).unwrap();
        assert!(out.contains(r#"href="fonts/F.ttf" media-type="font/ttf""#));
        assert!(!out.contains("application/x-font-ttf"));
    }

    #[test]
    fn detects_outdated_features() {
        let opf = r##"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"
         prefix="xsd: http://www.w3.org/2001/XMLSchema# foo: http://example.org/">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">x</dc:identifier>
    <dc:title>t</dc:title>
    <dc:language>en</dc:language>
    <meta property="rendition:flow">scrolled-continuous</meta>
    <meta property="rendition:layout">pre-paginated</meta>
  </metadata>
  <manifest>
    <item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>
    <item id="legacy" href="old.xml" media-type="application/x-foo" fallback="c"/>
  </manifest>
  <spine><itemref idref="c"/></spine>
  <collection role="index"><link href="i.xhtml"/></collection>
</package>"##;
        let info = parse_opf_info(opf).unwrap();
        let f = info.outdated_features.join(" | ");
        assert!(f.contains("fallback"), "fallback: {f}");
        assert!(f.contains("rendition:flow"), "rendition:flow: {f}");
        assert!(f.contains("collection"), "collection: {f}");
        assert!(f.contains("xsd"), "xsd prefix: {f}");
        // rendition:layout is NOT outdated — must not be flagged.
        assert!(!f.contains("rendition:layout"));
        // non-deprecated prefix not flagged.
        assert!(!f.contains("foo"));
    }

    #[test]
    fn no_outdated_features_in_clean_opf() {
        let info = parse_opf_info(OPF_PAGEBREAK).unwrap();
        assert!(info.outdated_features.is_empty());
    }

    #[test]
    fn rewrite_leaves_core_and_non_ttf_fonts_alone() {
        let opf = OPF_PAGEBREAK.replace(
            r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#,
            r#"<item id="t" href="F.ttf" media-type="font/ttf"/><item id="o" href="F.otf" media-type="application/x-font-otf"/>"#,
        );
        let out = rewrite_opf(&opf, &params(None)).unwrap();
        assert!(out.contains(r#"href="F.ttf" media-type="font/ttf""#));
        // OTF is out of scope — left untouched.
        assert!(out.contains(r#"href="F.otf" media-type="application/x-font-otf""#));
    }
}
