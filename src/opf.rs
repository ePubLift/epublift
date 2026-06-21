//! Parsing and rewriting of the OPF package document.
//!
//! `parse_opf_info` extracts everything we need in a read-only pass, then
//! `rewrite_opf` streams the original XML through quick-xml applying the
//! EPUB 3.3 upgrades while preserving the rest of the document byte-for-byte.

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
}

/// Local (namespace-stripped) name of an XML element.
fn local(name: QName) -> String {
    let s = String::from_utf8_lossy(name.as_ref()).into_owned();
    match s.rfind(':') {
        Some(i) => s[i + 1..].to_string(),
        None => s,
    }
}

/// Read an attribute's unescaped value from a start/empty element.
fn get_attr(e: &BytesStart, key: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == key.as_bytes() {
            return Some(
                a.normalized_value(XmlVersion::Implicit1_0)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned()),
            );
        }
    }
    None
}

/// Read-only pass over the OPF gathering manifest, cover, navigation and guide
/// information.
pub fn parse_opf_info(xml: &str) -> Result<OpfInfo> {
    let doc = roxmltree::Document::parse(xml).context("Failed to parse OPF package document")?;

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
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let mut new = BytesStart::new(name);
    let mut present: HashSet<String> = HashSet::new();

    for a in e.attributes().flatten() {
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        present.insert(key.clone());
        let value = a
            .normalized_value(XmlVersion::Implicit1_0)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
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
fn write_modified_meta<W: std::io::Write>(writer: &mut Writer<W>, ts: &str) -> Result<()> {
    writer.write_event(Event::Text(BytesText::from_escaped("\n    ")))?;
    let mut m = BytesStart::new("meta");
    m.push_attribute(("property", "dcterms:modified"));
    writer.write_event(Event::Start(m))?;
    writer.write_event(Event::Text(BytesText::new(ts)))?;
    writer.write_event(Event::End(BytesEnd::new("meta")))?;
    Ok(())
}

/// Write an EPUB 3.4 `<meta property="pageBreakSource">value</meta>` element.
fn write_page_break_source_meta<W: std::io::Write>(
    writer: &mut Writer<W>,
    value: &str,
) -> Result<()> {
    writer.write_event(Event::Text(BytesText::from_escaped("\n    ")))?;
    let mut m = BytesStart::new("meta");
    m.push_attribute(("property", "pageBreakSource"));
    writer.write_event(Event::Start(m))?;
    writer.write_event(Event::Text(BytesText::new(value)))?;
    writer.write_event(Event::End(BytesEnd::new("meta")))?;
    Ok(())
}

/// Stream the OPF through, applying the EPUB 3.3 upgrades.
pub fn rewrite_opf(xml: &str, params: &RewriteParams) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());

    let mut in_metadata = false;
    let mut in_manifest = false;
    let mut metadata_seen = false;
    let mut skip_until_end_meta = false;
    let mut skipping_guide = false;
    let mut guide_depth = 0i32;

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
                    _ => writer.write_event(Event::Empty(e))?,
                }
            }

            Event::End(e) => {
                let ln = local(e.name());
                match ln.as_str() {
                    "metadata" => {
                        write_modified_meta(&mut writer, &params.modified_ts)?;
                        if let Some(src) = &params.page_break_source {
                            write_page_break_source_meta(&mut writer, src)?;
                        }
                        writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        in_metadata = false;
                        writer.write_event(Event::End(e))?;
                    }
                    "manifest" => {
                        if params.add_nav {
                            writer.write_event(Event::Text(BytesText::from_escaped("\n    ")))?;
                            let mut it = BytesStart::new("item");
                            it.push_attribute(("id", "nav"));
                            it.push_attribute(("href", "nav.xhtml"));
                            it.push_attribute(("media-type", "application/xhtml+xml"));
                            it.push_attribute(("properties", "nav"));
                            writer.write_event(Event::Empty(it))?;
                            writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        }
                        in_manifest = false;
                        writer.write_event(Event::End(e))?;
                    }
                    "package" if !metadata_seen => {
                        // Extremely rare: OPF had no <metadata>. Inject one.
                        let mut md = BytesStart::new("metadata");
                        md.push_attribute(("xmlns:dc", NS_DC));
                        md.push_attribute(("xmlns:dcterms", NS_DCTERMS));
                        writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        writer.write_event(Event::Start(md))?;
                        write_modified_meta(&mut writer, &params.modified_ts)?;
                        writer.write_event(Event::Text(BytesText::from_escaped("\n  ")))?;
                        writer.write_event(Event::End(BytesEnd::new("metadata")))?;
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
        }
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
