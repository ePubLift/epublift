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

    Ok(OpfInfo {
        items,
        cover_id,
        nav_exists,
        ncx_href,
        has_guide,
        guide_refs,
    })
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
        None => transform_start(e, &[], &[]),
    }
}
