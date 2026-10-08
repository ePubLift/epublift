//! Reading (and, later, writing) of the book's bibliographic metadata from the
//! OPF package document.
//!
//! [`parse_metadata`] does a read-only pass with `roxmltree`, resolving EPUB 3
//! `refines` meta properties (`title-type`, `role`, `file-as`, `identifier-type`,
//! `group-position`, …) back onto the Dublin Core elements they describe, and
//! falling back to legacy EPUB 2 attributes (`opf:role`, `opf:scheme`) and
//! `calibre:*` series metas. The bibliographic vocabulary is identical in EPUB
//! 3.3 and 3.4, so this is version-agnostic. See `docs/metadata.md`.

use anyhow::{Context, Result};
use quick_xml::escape::escape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};
use std::collections::{HashMap, HashSet};

/// A `dc:title`, with its EPUB 3 `title-type` refinement if present
/// (`main`, `subtitle`, `short`, `collection`, `edition`, `expanded`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Title {
    pub value: String,
    pub title_type: Option<String>,
}

/// A `dc:creator` or `dc:contributor`, with its MARC relator `role`
/// (e.g. `aut`, `trl`, `edt`, `ill`) and `file-as` (sort) refinements.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Contributor {
    pub name: String,
    pub role: Option<String>,
    pub file_as: Option<String>,
}

/// A `dc:identifier`, with its scheme (`identifier-type` refinement or legacy
/// `opf:scheme`) and whether it is the package `unique-identifier`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Identifier {
    pub value: String,
    pub scheme: Option<String>,
    pub unique: bool,
}

/// A series, from an EPUB 3 `belongs-to-collection` (with `group-position`) or a
/// legacy `calibre:series` / `calibre:series_index` pair.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Series {
    pub name: String,
    pub position: Option<String>,
}

impl Series {
    /// `Name` or `Name:position`, as the CLI's `--series` and the web form
    /// write it. The text after the last `:` is the position when it starts
    /// with a digit and has no space (`Dune:2`, `Dune:1-6`, `Dune:2.5`), so a
    /// colon inside a name (`Star Wars: Legends`) stays part of the name.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if let Some((name, pos)) = s.rsplit_once(':')
            && pos.starts_with(|c: char| c.is_ascii_digit())
            && !pos.contains(char::is_whitespace)
            && !name.trim().is_empty()
        {
            return Self {
                name: name.trim().to_string(),
                position: Some(pos.to_string()),
            };
        }
        Self {
            name: s.to_string(),
            position: None,
        }
    }
}

/// Everything we read out of the OPF `<metadata>` element.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// The `<package version="…">` attribute (e.g. `"2.0"`, `"3.0"`).
    pub epub_version: Option<String>,
    pub titles: Vec<Title>,
    pub creators: Vec<Contributor>,
    pub contributors: Vec<Contributor>,
    pub languages: Vec<String>,
    pub identifiers: Vec<Identifier>,
    /// `dc:source` (e.g. the print edition's `urn:isbn:…`).
    pub source: Option<String>,
    pub publisher: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub subjects: Vec<String>,
    pub series: Option<Series>,
    pub rights: Option<String>,
    /// `dcterms:modified`.
    pub modified: Option<String>,
}

/// Concatenated, trimmed text content of a node. `None` if empty.
fn text_of(n: roxmltree::Node) -> Option<String> {
    let t: String = n
        .children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|c| c.text())
        .collect();
    let t = t.trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// An attribute matched by local name, ignoring any namespace prefix — so this
/// catches both `file-as` and the legacy `opf:file-as` / `opf:role` / `opf:scheme`.
fn attr_local(n: roxmltree::Node, name: &str) -> Option<String> {
    n.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value().to_string())
}

/// Read all bibliographic metadata from an OPF package document.
pub fn parse_metadata(xml: &str) -> Result<Metadata> {
    let doc = crate::util::parse_xml(xml).context("Failed to parse OPF package document")?;
    let pkg = doc.root_element();

    let mut md = Metadata {
        epub_version: pkg.attribute("version").map(str::to_string),
        ..Metadata::default()
    };
    let unique_id = pkg.attribute("unique-identifier").unwrap_or("");

    let Some(metadata) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")
    else {
        return Ok(md);
    };

    let metas = || {
        metadata
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "meta")
    };
    let dc = |name: &'static str| {
        metadata
            .children()
            .filter(move |n| n.is_element() && n.tag_name().name() == name)
    };

    // Map an element id to its `refines` (property, value) pairs, e.g.
    // "#titleid" -> [("title-type","main")].
    let mut refines: HashMap<String, Vec<(String, String)>> = HashMap::new();
    // Legacy EPUB 2 `<meta name="…" content="…">` pairs (calibre series, etc.).
    let mut legacy: HashMap<String, String> = HashMap::new();
    for n in metas() {
        if let (Some(name), Some(content)) = (n.attribute("name"), n.attribute("content")) {
            legacy.insert(name.to_string(), content.to_string());
        }
        if let (Some(r), Some(prop)) = (n.attribute("refines"), n.attribute("property"))
            && let Some(val) = text_of(n)
        {
            refines
                .entry(r.trim_start_matches('#').to_string())
                .or_default()
                .push((prop.to_string(), val));
        }
    }
    let refined = |id: Option<&str>, prop: &str| -> Option<String> {
        let id = id?;
        refines
            .get(id)?
            .iter()
            .find(|(p, _)| p == prop)
            .map(|(_, v)| v.clone())
    };

    // Titles.
    for n in dc("title") {
        if let Some(value) = text_of(n) {
            md.titles.push(Title {
                value,
                title_type: refined(n.attribute("id"), "title-type"),
            });
        }
    }

    // Creators / contributors.
    let contributor = |n: roxmltree::Node| -> Option<Contributor> {
        let name = text_of(n)?;
        let id = n.attribute("id");
        Some(Contributor {
            name,
            role: refined(id, "role").or_else(|| attr_local(n, "role")),
            file_as: refined(id, "file-as").or_else(|| attr_local(n, "file-as")),
        })
    };
    md.creators = dc("creator").filter_map(contributor).collect();
    md.contributors = dc("contributor").filter_map(contributor).collect();

    // Languages, identifiers, simple single-valued fields.
    md.languages = dc("language").filter_map(text_of).collect();
    for n in dc("identifier") {
        if let Some(value) = text_of(n) {
            let id = n.attribute("id");
            md.identifiers.push(Identifier {
                value,
                scheme: refined(id, "identifier-type").or_else(|| attr_local(n, "scheme")),
                unique: !unique_id.is_empty() && id == Some(unique_id),
            });
        }
    }
    md.source = dc("source").find_map(text_of);
    md.publisher = dc("publisher").find_map(text_of);
    md.date = dc("date").find_map(text_of);
    md.description = dc("description").find_map(text_of);
    md.rights = dc("rights").find_map(text_of);
    md.subjects = dc("subject").filter_map(text_of).collect();

    // Modified: a non-refining `<meta property="dcterms:modified">`.
    md.modified = metas()
        .filter(|n| {
            n.attribute("refines").is_none() && n.attribute("property") == Some("dcterms:modified")
        })
        .find_map(text_of);

    // Series: prefer EPUB 3 belongs-to-collection (skip `set`-typed collections),
    // else fall back to legacy calibre:series.
    md.series = metas()
        .filter(|n| n.attribute("property") == Some("belongs-to-collection"))
        .find_map(|n| {
            let name = text_of(n)?;
            let id = n.attribute("id");
            if refined(id, "collection-type").as_deref() == Some("set") {
                return None;
            }
            Some(Series {
                name,
                position: refined(id, "group-position"),
            })
        })
        .or_else(|| {
            legacy.get("calibre:series").map(|name| Series {
                name: name.clone(),
                position: legacy.get("calibre:series_index").cloned(),
            })
        });

    Ok(md)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn field(out: &mut String, label: &str, value: &str) {
    out.push_str(label);
    for _ in label.len()..14 {
        out.push(' ');
    }
    out.push_str(value);
    out.push('\n');
}

fn render_contributor(c: &Contributor) -> String {
    let mut extra = Vec::new();
    if let Some(r) = &c.role {
        extra.push(r.clone());
    }
    if let Some(f) = &c.file_as {
        extra.push(format!("file-as: {f}"));
    }
    if extra.is_empty() {
        c.name.clone()
    } else {
        format!("{}  [{}]", c.name, extra.join("; "))
    }
}

impl Metadata {
    /// Human-readable table of the metadata that is present (empty fields are
    /// omitted).
    pub fn to_text(&self) -> String {
        let mut o = String::new();
        if let Some(v) = &self.epub_version {
            field(&mut o, "EPUB version:", v);
        }
        for t in &self.titles {
            let label = match t.title_type.as_deref() {
                Some("subtitle") => "Subtitle:",
                Some("main") | None => "Title:",
                Some(other) => return_title_label(other),
            };
            field(&mut o, label, &t.value);
        }
        for (i, c) in self.creators.iter().enumerate() {
            field(
                &mut o,
                if i == 0 { "Author(s):" } else { "" },
                &render_contributor(c),
            );
        }
        for (i, c) in self.contributors.iter().enumerate() {
            field(
                &mut o,
                if i == 0 { "Contributors:" } else { "" },
                &render_contributor(c),
            );
        }
        if !self.languages.is_empty() {
            field(&mut o, "Language:", &self.languages.join(", "));
        }
        for (i, id) in self.identifiers.iter().enumerate() {
            let mut tags = Vec::new();
            if let Some(s) = &id.scheme {
                tags.push(s.clone());
            }
            if id.unique {
                tags.push("unique".to_string());
            }
            let v = if tags.is_empty() {
                id.value.clone()
            } else {
                format!("{}  [{}]", id.value, tags.join("; "))
            };
            field(&mut o, if i == 0 { "Identifier:" } else { "" }, &v);
        }
        if let Some(s) = &self.source {
            field(&mut o, "Source:", s);
        }
        if let Some(p) = &self.publisher {
            field(&mut o, "Publisher:", p);
        }
        if let Some(d) = &self.date {
            field(&mut o, "Date:", d);
        }
        if let Some(s) = &self.series {
            let v = match &s.position {
                Some(p) => format!("{} (#{p})", s.name),
                None => s.name.clone(),
            };
            field(&mut o, "Series:", &v);
        }
        if !self.subjects.is_empty() {
            field(&mut o, "Subjects:", &self.subjects.join(", "));
        }
        if let Some(d) = &self.description {
            // Collapse whitespace and truncate for the table; full text is in --json.
            let flat = d.split_whitespace().collect::<Vec<_>>().join(" ");
            let shown = if flat.chars().count() > 200 {
                let mut s: String = flat.chars().take(200).collect();
                s.push('…');
                s
            } else {
                flat
            };
            field(&mut o, "Description:", &shown);
        }
        if let Some(r) = &self.rights {
            field(&mut o, "Rights:", r);
        }
        if let Some(m) = &self.modified {
            field(&mut o, "Modified:", m);
        }
        if o.is_empty() {
            o.push_str("(no metadata found)\n");
        }
        o
    }

    /// Machine-readable JSON (dependency-free; valid UTF-8 JSON).
    pub fn to_json(&self) -> String {
        let mut o = String::from("{\n");
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.epub_version {
            parts.push(format!("  \"epub_version\": {}", jstr(v)));
        }
        parts.push(format!(
            "  \"titles\": [{}]",
            self.titles
                .iter()
                .map(|t| format!(
                    "{{\"value\": {}, \"type\": {}}}",
                    jstr(&t.value),
                    jopt(t.title_type.as_deref())
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let contribs = |cs: &[Contributor]| {
            cs.iter()
                .map(|c| {
                    format!(
                        "{{\"name\": {}, \"role\": {}, \"file_as\": {}}}",
                        jstr(&c.name),
                        jopt(c.role.as_deref()),
                        jopt(c.file_as.as_deref())
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        parts.push(format!("  \"creators\": [{}]", contribs(&self.creators)));
        parts.push(format!(
            "  \"contributors\": [{}]",
            contribs(&self.contributors)
        ));
        parts.push(format!("  \"languages\": [{}]", jarr(&self.languages)));
        parts.push(format!(
            "  \"identifiers\": [{}]",
            self.identifiers
                .iter()
                .map(|id| format!(
                    "{{\"value\": {}, \"scheme\": {}, \"unique\": {}}}",
                    jstr(&id.value),
                    jopt(id.scheme.as_deref()),
                    id.unique
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        parts.push(format!("  \"source\": {}", jopt(self.source.as_deref())));
        parts.push(format!(
            "  \"publisher\": {}",
            jopt(self.publisher.as_deref())
        ));
        parts.push(format!("  \"date\": {}", jopt(self.date.as_deref())));
        parts.push(format!(
            "  \"description\": {}",
            jopt(self.description.as_deref())
        ));
        parts.push(format!("  \"subjects\": [{}]", jarr(&self.subjects)));
        parts.push(format!(
            "  \"series\": {}",
            match &self.series {
                Some(s) => format!(
                    "{{\"name\": {}, \"position\": {}}}",
                    jstr(&s.name),
                    jopt(s.position.as_deref())
                ),
                None => "null".to_string(),
            }
        ));
        parts.push(format!("  \"rights\": {}", jopt(self.rights.as_deref())));
        parts.push(format!(
            "  \"modified\": {}",
            jopt(self.modified.as_deref())
        ));
        o.push_str(&parts.join(",\n"));
        o.push_str("\n}");
        o
    }
}

/// Map a non-standard `title-type` to a stable display label.
fn return_title_label(t: &str) -> &'static str {
    match t {
        "short" => "Short title:",
        "collection" => "Collection:",
        "edition" => "Edition:",
        "expanded" => "Full title:",
        _ => "Title:",
    }
}

/// JSON-escape a string and wrap it in quotes.
fn jstr(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `jstr` of a value, or `null`.
fn jopt(s: Option<&str>) -> String {
    s.map_or_else(|| "null".to_string(), jstr)
}

/// A JSON array body (comma-joined `jstr`s) from a slice of strings.
fn jarr(items: &[String]) -> String {
    items.iter().map(|s| jstr(s)).collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// A set of metadata fields to write into an EPUB's OPF. Every field is
/// optional: `None` leaves that field untouched; `Some` **replaces** it (the
/// existing element(s) for that field, plus their `refines` metas, are removed
/// and re-emitted from this value). In EPUB 3, `dcterms:modified` is always
/// refreshed.
///
/// Each field is written in the syntax of the book's own version: EPUB 3
/// refinements (`refines`, `property`) in an EPUB 3 package, the OPF 2
/// attributes (`opf:role`, `opf:file-as`, `opf:scheme`) and Calibre's series
/// metas in an EPUB 2 one, which has no `refines` and no `dcterms:modified`.
///
/// Multi-valued fields (`authors`, `subjects`) replace the whole set when
/// `Some`. Everything not modelled here (cover meta, rendition properties,
/// accessibility metadata, unknown vocab, the package `unique-identifier`, …) is
/// preserved verbatim.
#[derive(Debug, Default, Clone)]
pub struct MetadataUpdate {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub authors: Option<Vec<String>>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub subjects: Option<Vec<String>>,
    pub series: Option<Series>,
    /// Written as a `dc:identifier` `urn:isbn:<value>` (id `epublift-isbn`) — the
    /// form Calibre / Apple Books recognize as the book's ISBN. An additional
    /// identifier, not the package `unique-identifier`.
    pub isbn: Option<String>,
}

impl MetadataUpdate {
    /// `true` when no field is set (nothing to write).
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.subtitle.is_none()
            && self.authors.is_none()
            && self.language.is_none()
            && self.publisher.is_none()
            && self.date.is_none()
            && self.description.is_none()
            && self.subjects.is_none()
            && self.series.is_none()
            && self.isbn.is_none()
    }
}

/// Which existing metadata elements an update will remove, computed up front so
/// the streaming pass can drop them (and re-emit fresh values at `</metadata>`).
#[derive(Debug, Default)]
struct DropPlan {
    /// Dublin Core local tag names to drop entirely (e.g. `creator`, `subject`).
    drop_tags: HashSet<String>,
    /// Indices (among `dc:title` elements, in document order) to drop — so a
    /// `--subtitle` edit doesn't remove the main title and vice versa.
    title_drop_idx: HashSet<usize>,
    /// Indices (among `dc:identifier` elements) to drop — only our own
    /// `id="epublift-isbn"` identifier, so re-writing the ISBN is idempotent and
    /// never touches the unique-identifier or other identifiers (ASIN, etc.).
    identifier_drop_idx: HashSet<usize>,
    /// Element ids whose refining `<meta refines="#id">` should also be dropped.
    drop_ids: HashSet<String>,
    /// Drop `belongs-to-collection` metas and legacy `calibre:series*` (series set).
    drop_series_metas: bool,
    /// How the injected elements are spelled in this package.
    syntax: Syntax,
    /// The book's authors before the edit: an author who stays keeps their
    /// role and sort name (`file-as`), which a form does not carry.
    prior_creators: Vec<Contributor>,
}

const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
const OPF_NS: &str = "http://www.idpf.org/2007/opf";

/// How to spell what we inject so that it parses and validates in *this*
/// package: its version, and the prefixes it binds (Calibre writes `ns0:` for
/// the OPF namespace and declares `dc` on each element, for example).
#[derive(Debug, Default)]
struct Syntax {
    epub3: bool,
    /// The Dublin Core prefix with its colon, and a declaration to add on each
    /// element when no prefix for it is in scope at `<metadata>`.
    dc: String,
    dc_decl: String,
    /// The prefix for OPF elements (`""` when OPF is the default namespace).
    opf_el: String,
    /// The prefix for OPF attributes (attributes never take the default
    /// namespace), and a declaration when none is in scope.
    opf_attr: String,
    opf_decl: String,
}

impl Syntax {
    fn of(metadata: roxmltree::Node, epub3: bool) -> Self {
        let prefixed = |uri: &str| {
            metadata
                .namespaces()
                .find(|ns| ns.uri() == uri && ns.name().is_some())
                .and_then(|ns| ns.name())
                .map(|p| format!("{p}:"))
        };
        let (dc, dc_decl) = match prefixed(DC_NS) {
            Some(p) => (p, String::new()),
            None => ("dc:".to_string(), format!(" xmlns:dc=\"{DC_NS}\"")),
        };
        let opf_default = metadata
            .namespaces()
            .any(|ns| ns.uri() == OPF_NS && ns.name().is_none());
        let opf_prefix = prefixed(OPF_NS);
        let opf_el = if opf_default {
            String::new()
        } else {
            opf_prefix.clone().unwrap_or_default()
        };
        let (opf_attr, opf_decl) = match opf_prefix {
            Some(p) => (p, String::new()),
            None => ("opf:".to_string(), format!(" xmlns:opf=\"{OPF_NS}\"")),
        };
        Self {
            epub3,
            dc,
            dc_decl,
            opf_el,
            opf_attr,
            opf_decl,
        }
    }
}

fn build_drop_plan(xml: &str, update: &MetadataUpdate) -> Result<DropPlan> {
    let doc = crate::util::parse_xml(xml).context("Failed to parse OPF package document")?;
    let mut plan = DropPlan::default();
    let mut tag = |present: bool, name: &str| {
        if present {
            plan.drop_tags.insert(name.to_string());
        }
    };
    tag(update.authors.is_some(), "creator");
    tag(update.language.is_some(), "language");
    tag(update.publisher.is_some(), "publisher");
    tag(update.date.is_some(), "date");
    tag(update.description.is_some(), "description");
    tag(update.subjects.is_some(), "subject");

    let Some(metadata) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")
    else {
        return Ok(plan);
    };
    let epub3 = doc
        .root_element()
        .attribute("version")
        .is_some_and(|v| v.trim_start().starts_with('3'));
    plan.syntax = Syntax::of(metadata, epub3);
    if !epub3
        && update
            .subtitle
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    {
        anyhow::bail!(
            "EPUB 2 has no subtitle field: put the subtitle in the title, or upgrade the \
             book to EPUB 3 first (Optimize, or `epublift -i <book>`)"
        );
    }
    if update.authors.is_some() {
        plan.prior_creators = parse_metadata(xml)?.creators;
    }
    let children = |name: &'static str| {
        metadata
            .children()
            .filter(move |n| n.is_element() && n.tag_name().name() == name)
    };

    // id -> title-type, so each dc:title can be classified main vs subtitle.
    let mut title_type: HashMap<String, String> = HashMap::new();
    for n in children("meta") {
        if let (Some(r), Some("title-type")) = (n.attribute("refines"), n.attribute("property"))
            && let Some(v) = text_of(n)
        {
            title_type.insert(r.trim_start_matches('#').to_string(), v);
        }
    }
    for (i, t) in children("title").enumerate() {
        let ty = t
            .attribute("id")
            .and_then(|id| title_type.get(id))
            .map_or("main", String::as_str);
        let drop = if ty == "subtitle" {
            update.subtitle.is_some()
        } else {
            update.title.is_some()
        };
        if drop {
            plan.title_drop_idx.insert(i);
            if let Some(id) = t.attribute("id") {
                plan.drop_ids.insert(id.to_string());
            }
        }
    }
    if update.authors.is_some() {
        for c in children("creator") {
            if let Some(id) = c.attribute("id") {
                plan.drop_ids.insert(id.to_string());
            }
        }
    }
    if update.isbn.is_some() {
        for (i, idel) in children("identifier").enumerate() {
            if idel.attribute("id") == Some("epublift-isbn") {
                plan.identifier_drop_idx.insert(i);
            }
        }
    }
    if update.series.is_some() {
        plan.drop_series_metas = true;
        for m in children("meta") {
            if m.attribute("property") == Some("belongs-to-collection")
                && let Some(id) = m.attribute("id")
            {
                plan.drop_ids.insert(id.to_string());
            }
        }
    }
    Ok(plan)
}

/// Local (namespace-stripped) name of an XML element.
fn local_name(q: QName) -> String {
    let s: &str = q.as_ref();
    match s.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => s.to_string(),
    }
}

/// Read an attribute's raw value from a start/empty element.
fn get_attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(|a| a.value.into_owned())
}

fn should_drop_dc(
    name: &str,
    title_counter: &mut usize,
    ident_counter: &mut usize,
    plan: &DropPlan,
) -> bool {
    if name == "title" {
        let i = *title_counter;
        *title_counter += 1;
        return plan.title_drop_idx.contains(&i);
    }
    if name == "identifier" {
        let i = *ident_counter;
        *ident_counter += 1;
        return plan.identifier_drop_idx.contains(&i);
    }
    plan.drop_tags.contains(name)
}

fn should_drop_meta(e: &BytesStart, plan: &DropPlan) -> bool {
    let prop = get_attr(e, "property");
    if prop.as_deref() == Some("dcterms:modified") {
        return true;
    }
    if let Some(r) = get_attr(e, "refines")
        && plan.drop_ids.contains(r.trim_start_matches('#'))
    {
        return true;
    }
    if plan.drop_series_metas {
        if prop.as_deref() == Some("belongs-to-collection") {
            return true;
        }
        if let Some(nm) = get_attr(e, "name")
            && (nm == "calibre:series" || nm == "calibre:series_index")
        {
            return true;
        }
    }
    false
}

/// Append `\n    <tag attrs>escaped-text</tag>` to `buf`.
fn push_el(buf: &mut String, tag: &str, attrs: &str, text: &str) {
    buf.push_str("\n    <");
    buf.push_str(tag);
    if !attrs.is_empty() {
        buf.push(' ');
        buf.push_str(attrs.trim_start());
    }
    buf.push('>');
    buf.push_str(&escape(text));
    buf.push_str("</");
    buf.push_str(tag);
    buf.push('>');
}

/// The fresh metadata elements to inject just before `</metadata>`.
fn build_injected(update: &MetadataUpdate, modified_ts: &str, plan: &DropPlan) -> String {
    let s = &plan.syntax;
    let mut b = String::new();
    let dc = |name: &str| format!("{}{name}", s.dc);
    let meta = format!("{}meta", s.opf_el);
    // An EPUB 3 refinement of the element with `id`.
    let refine = |b: &mut String, id: &str, property: &str, extra: &str, value: &str| {
        b.push_str(&format!(
            "\n    <{meta} refines=\"#{id}\" property=\"{property}\"{extra}>{}</{meta}>",
            escape(value)
        ));
    };

    if let Some(t) = &update.title {
        if s.epub3 {
            push_el(
                &mut b,
                &dc("title"),
                &format!("id=\"epublift-title\"{}", s.dc_decl),
                t,
            );
            refine(&mut b, "epublift-title", "title-type", "", "main");
        } else {
            push_el(&mut b, &dc("title"), &s.dc_decl, t);
        }
    }
    if let Some(sub) = &update.subtitle
        && s.epub3
    {
        push_el(
            &mut b,
            &dc("title"),
            &format!("id=\"epublift-subtitle\"{}", s.dc_decl),
            sub,
        );
        refine(&mut b, "epublift-subtitle", "title-type", "", "subtitle");
    }
    if let Some(authors) = &update.authors {
        let mut used = vec![false; plan.prior_creators.len()];
        for (i, a) in authors.iter().enumerate() {
            // An author already in the book keeps their role and sort name; a
            // name listed twice is matched in order, entry by entry.
            let prior = plan
                .prior_creators
                .iter()
                .enumerate()
                .find(|(j, c)| !used[*j] && c.name.trim() == a.trim())
                .map(|(j, c)| {
                    used[j] = true;
                    c
                });
            let role = prior.and_then(|c| c.role.as_deref()).unwrap_or("aut");
            let file_as = prior.and_then(|c| c.file_as.as_deref());
            if s.epub3 {
                let id = format!("epublift-creator-{i}");
                push_el(
                    &mut b,
                    &dc("creator"),
                    &format!("id=\"{id}\"{}", s.dc_decl),
                    a,
                );
                refine(&mut b, &id, "role", " scheme=\"marc:relators\"", role);
                if let Some(f) = file_as {
                    refine(&mut b, &id, "file-as", "", f);
                }
                refine(&mut b, &id, "display-seq", "", &(i + 1).to_string());
            } else {
                let mut attrs = format!("{}role=\"{}\"", s.opf_attr, escape(role));
                if let Some(f) = file_as {
                    attrs.push_str(&format!(" {}file-as=\"{}\"", s.opf_attr, escape(f)));
                }
                attrs.push_str(&s.dc_decl);
                attrs.push_str(&s.opf_decl);
                push_el(&mut b, &dc("creator"), &attrs, a);
            }
        }
    }
    for (name, value) in [
        ("language", &update.language),
        ("publisher", &update.publisher),
        ("date", &update.date),
        ("description", &update.description),
    ] {
        if let Some(v) = value {
            push_el(&mut b, &dc(name), &s.dc_decl, v);
        }
    }
    if let Some(subs) = &update.subjects {
        for sub in subs {
            push_el(&mut b, &dc("subject"), &s.dc_decl, sub);
        }
    }
    if let Some(isbn) = &update.isbn {
        // A `dc:identifier` — the form Calibre, Apple Books and other readers
        // recognize as the book's ISBN (a `dc:source` is not surfaced by them).
        // An additional identifier, not the package `unique-identifier`.
        if s.epub3 {
            push_el(
                &mut b,
                &dc("identifier"),
                &format!("id=\"epublift-isbn\"{}", s.dc_decl),
                &format!("urn:isbn:{isbn}"),
            );
        } else {
            push_el(
                &mut b,
                &dc("identifier"),
                &format!(
                    "id=\"epublift-isbn\" {}scheme=\"ISBN\"{}{}",
                    s.opf_attr, s.dc_decl, s.opf_decl
                ),
                isbn,
            );
        }
    }
    if let Some(series) = &update.series {
        if s.epub3 {
            push_el(
                &mut b,
                &meta,
                "property=\"belongs-to-collection\" id=\"epublift-series\"",
                &series.name,
            );
            refine(&mut b, "epublift-series", "collection-type", "", "series");
            if let Some(pos) = &series.position {
                refine(&mut b, "epublift-series", "group-position", "", pos);
            }
        } else {
            // EPUB 2 has no collections; Calibre's metas are what readers use.
            b.push_str(&format!(
                "\n    <{meta} name=\"calibre:series\" content=\"{}\"/>",
                escape(&series.name)
            ));
            if let Some(pos) = &series.position {
                b.push_str(&format!(
                    "\n    <{meta} name=\"calibre:series_index\" content=\"{}\"/>",
                    escape(pos)
                ));
            }
        }
    }
    if s.epub3 {
        b.push_str(&format!(
            "\n    <{meta} property=\"dcterms:modified\">{modified_ts}</{meta}>"
        ));
    }
    b.push_str("\n  ");
    b
}

/// `update` minus the fields whose value the book already has. A form sends
/// every field it shows; re-writing an unchanged one would only lose what the
/// form does not carry (a second `dc:language`, the ISBN's own markup).
fn without_unchanged(update: &MetadataUpdate, md: &Metadata) -> MetadataUpdate {
    fn keep<T: Clone>(new: &Option<T>, same: impl Fn(&T) -> bool) -> Option<T> {
        new.as_ref().filter(|v| !same(v)).cloned()
    }
    let eq = |a: &str, b: Option<&str>| b.is_some_and(|b| a.trim() == b.trim());
    let main = md
        .titles
        .iter()
        .find(|t| t.title_type.as_deref() != Some("subtitle"))
        .map(|t| t.value.as_str());
    let sub = md
        .titles
        .iter()
        .find(|t| t.title_type.as_deref() == Some("subtitle"))
        .map(|t| t.value.as_str());
    let names = |v: &[String]| v.iter().map(|s| s.trim().to_string()).collect::<Vec<_>>();
    let isbn_digits = |s: &str| {
        s.trim_start_matches("urn:isbn:")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_uppercase()
    };
    let has_isbn = |isbn: &String| {
        md.identifiers.iter().any(|i| {
            let marked = i
                .scheme
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
                || i.value.starts_with("urn:isbn:");
            marked && isbn_digits(&i.value) == isbn_digits(isbn)
        })
    };
    MetadataUpdate {
        title: keep(&update.title, |v| eq(v, main)),
        subtitle: keep(&update.subtitle, |v| eq(v, sub)),
        authors: keep(&update.authors, |v| {
            names(v)
                == names(
                    &md.creators
                        .iter()
                        .map(|c| c.name.clone())
                        .collect::<Vec<_>>(),
                )
        }),
        language: keep(&update.language, |v| {
            eq(v, md.languages.first().map(String::as_str))
        }),
        publisher: keep(&update.publisher, |v| eq(v, md.publisher.as_deref())),
        date: keep(&update.date, |v| eq(v, md.date.as_deref())),
        description: keep(&update.description, |v| eq(v, md.description.as_deref())),
        subjects: keep(&update.subjects, |v| names(v) == names(&md.subjects)),
        series: keep(&update.series, |v| {
            md.series.as_ref().is_some_and(|s| {
                s.name.trim() == v.name.trim()
                    && s.position.as_deref().map(str::trim) == v.position.as_deref().map(str::trim)
            })
        }),
        isbn: keep(&update.isbn, has_isbn),
    }
}

/// Apply `update` to an OPF package document, returning the new XML.
///
/// Streams the original with `quick-xml`, dropping the elements the update
/// replaces (per [`build_drop_plan`]) and re-emitting fresh values right before
/// `</metadata>`. Everything else is preserved as-is. `modified_ts` is the new
/// `dcterms:modified` value (always refreshed in EPUB 3; EPUB 2 has none).
pub fn apply_update(xml: &str, update: &MetadataUpdate, modified_ts: &str) -> Result<String> {
    let update = &without_unchanged(update, &parse_metadata(xml)?);
    let plan = build_drop_plan(xml, update)?;
    let mut reader = Reader::from_str(xml);
    let mut writer = Writer::new(Vec::new());
    let mut in_metadata = false;
    let mut metadata_seen = false;
    let mut skip_depth: u32 = 0;
    let mut title_counter: usize = 0;
    let mut ident_counter: usize = 0;

    loop {
        match reader
            .read_event()
            .context("Failed to read OPF while editing metadata")?
        {
            Event::Start(e) => {
                if skip_depth > 0 {
                    skip_depth += 1;
                    continue;
                }
                let name = local_name(e.name());
                if name == "metadata" {
                    in_metadata = true;
                    metadata_seen = true;
                    writer.write_event(Event::Start(e))?;
                    continue;
                }
                if in_metadata {
                    let drop = if name == "meta" {
                        should_drop_meta(&e, &plan)
                    } else {
                        should_drop_dc(&name, &mut title_counter, &mut ident_counter, &plan)
                    };
                    if drop {
                        skip_depth = 1;
                        continue;
                    }
                }
                writer.write_event(Event::Start(e))?;
            }
            Event::Empty(e) => {
                if skip_depth > 0 {
                    continue;
                }
                let name = local_name(e.name());
                if in_metadata {
                    let drop = if name == "meta" {
                        should_drop_meta(&e, &plan)
                    } else {
                        should_drop_dc(&name, &mut title_counter, &mut ident_counter, &plan)
                    };
                    if drop {
                        continue;
                    }
                }
                writer.write_event(Event::Empty(e))?;
            }
            Event::End(e) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                if in_metadata && local_name(e.name()) == "metadata" {
                    let injected = build_injected(update, modified_ts, &plan);
                    writer.get_mut().extend_from_slice(injected.as_bytes());
                    in_metadata = false;
                }
                writer.write_event(Event::End(e))?;
            }
            Event::Eof => break,
            ev => {
                if skip_depth > 0 {
                    continue;
                }
                writer.write_event(ev)?;
            }
        }
    }

    if !metadata_seen {
        anyhow::bail!("OPF has no <metadata> element to edit");
    }
    String::from_utf8(writer.into_inner()).context("OPF became invalid UTF-8 after editing")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"<?xml version="1.0"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="bookid">urn:uuid:12345</dc:identifier>
    <dc:identifier id="isbn" opf:scheme="ISBN">9780140328721</dc:identifier>
    <dc:title id="t">Fantastic Mr Fox</dc:title>
    <meta refines="#t" property="title-type">main</meta>
    <dc:creator id="aut">Roald Dahl</dc:creator>
    <meta refines="#aut" property="role" scheme="marc:relators">aut</meta>
    <meta refines="#aut" property="file-as">Dahl, Roald</meta>
    <dc:contributor opf:role="ill">Tony Ross</dc:contributor>
    <dc:language>en</dc:language>
    <dc:publisher>Puffin</dc:publisher>
    <dc:date>1988</dc:date>
    <dc:subject>Foxes</dc:subject>
    <dc:subject>Fiction</dc:subject>
    <meta property="belongs-to-collection" id="c">Mr Fox Series</meta>
    <meta refines="#c" property="group-position">1</meta>
    <meta property="dcterms:modified">2020-01-01T00:00:00Z</meta>
  </metadata>
</package>"##;

    #[test]
    fn parses_core_fields() {
        let md = parse_metadata(SAMPLE).unwrap();
        assert_eq!(md.epub_version.as_deref(), Some("3.0"));
        assert_eq!(md.titles.len(), 1);
        assert_eq!(md.titles[0].value, "Fantastic Mr Fox");
        assert_eq!(md.titles[0].title_type.as_deref(), Some("main"));
        assert_eq!(md.languages, vec!["en"]);
        assert_eq!(md.publisher.as_deref(), Some("Puffin"));
        assert_eq!(md.date.as_deref(), Some("1988"));
        assert_eq!(md.subjects, vec!["Foxes", "Fiction"]);
        assert_eq!(md.modified.as_deref(), Some("2020-01-01T00:00:00Z"));
    }

    #[test]
    fn resolves_refines_and_legacy_attrs() {
        let md = parse_metadata(SAMPLE).unwrap();
        assert_eq!(md.creators.len(), 1);
        assert_eq!(md.creators[0].name, "Roald Dahl");
        assert_eq!(md.creators[0].role.as_deref(), Some("aut"));
        assert_eq!(md.creators[0].file_as.as_deref(), Some("Dahl, Roald"));
        // contributor uses legacy opf:role attribute
        assert_eq!(md.contributors[0].role.as_deref(), Some("ill"));
    }

    #[test]
    fn identifiers_and_series() {
        let md = parse_metadata(SAMPLE).unwrap();
        let unique = md.identifiers.iter().find(|i| i.unique).unwrap();
        assert_eq!(unique.value, "urn:uuid:12345");
        let isbn = md
            .identifiers
            .iter()
            .find(|i| i.scheme.as_deref() == Some("ISBN"))
            .unwrap();
        assert_eq!(isbn.value, "9780140328721");
        let series = md.series.unwrap();
        assert_eq!(series.name, "Mr Fox Series");
        assert_eq!(series.position.as_deref(), Some("1"));
    }

    #[test]
    fn json_is_wellformed_ish() {
        let md = parse_metadata(SAMPLE).unwrap();
        let j = md.to_json();
        assert!(j.starts_with('{') && j.trim_end().ends_with('}'));
        assert!(j.contains("\"creators\""));
        assert!(j.contains("Fantastic Mr Fox"));
    }

    #[test]
    fn set_replaces_fields_and_preserves_others() {
        let update = MetadataUpdate {
            title: Some("Yeni Başlık".to_string()),
            authors: Some(vec!["Ada Lovelace".to_string(), "Alan Turing".to_string()]),
            language: Some("tr".to_string()),
            subjects: Some(vec!["Bilim".to_string()]),
            ..MetadataUpdate::default()
        };
        let out = apply_update(SAMPLE, &update, "2026-06-22T10:00:00Z").unwrap();
        let md = parse_metadata(&out).unwrap();

        // Replaced fields.
        assert_eq!(md.titles.len(), 1);
        assert_eq!(md.titles[0].value, "Yeni Başlık");
        assert_eq!(md.titles[0].title_type.as_deref(), Some("main"));
        assert_eq!(
            md.creators
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Ada Lovelace", "Alan Turing"]
        );
        assert!(md.creators.iter().all(|c| c.role.as_deref() == Some("aut")));
        assert_eq!(md.languages, vec!["tr"]);
        assert_eq!(md.subjects, vec!["Bilim"]);
        // modified is refreshed.
        assert_eq!(md.modified.as_deref(), Some("2026-06-22T10:00:00Z"));

        // Untouched fields survive.
        assert_eq!(md.publisher.as_deref(), Some("Puffin"));
        assert!(
            md.identifiers
                .iter()
                .any(|i| i.unique && i.value == "urn:uuid:12345")
        );
        assert!(
            md.identifiers
                .iter()
                .any(|i| i.scheme.as_deref() == Some("ISBN"))
        );
        // The series we didn't set is preserved.
        assert_eq!(md.series.as_ref().unwrap().name, "Mr Fox Series");
    }

    #[test]
    fn set_subtitle_keeps_main_title() {
        let update = MetadataUpdate {
            subtitle: Some("Bir Alt Başlık".to_string()),
            ..MetadataUpdate::default()
        };
        let out = apply_update(SAMPLE, &update, "2026-06-22T10:00:00Z").unwrap();
        let md = parse_metadata(&out).unwrap();
        // Main title untouched, subtitle added.
        assert!(md.titles.iter().any(|t| t.value == "Fantastic Mr Fox"));
        assert!(
            md.titles
                .iter()
                .any(|t| t.value == "Bir Alt Başlık" && t.title_type.as_deref() == Some("subtitle"))
        );
    }

    #[test]
    fn set_series_replaces_collection() {
        let update = MetadataUpdate {
            series: Some(Series {
                name: "Yeni Seri".to_string(),
                position: Some("3".to_string()),
            }),
            ..MetadataUpdate::default()
        };
        let out = apply_update(SAMPLE, &update, "2026-06-22T10:00:00Z").unwrap();
        let md = parse_metadata(&out).unwrap();
        let s = md.series.unwrap();
        assert_eq!(s.name, "Yeni Seri");
        assert_eq!(s.position.as_deref(), Some("3"));
        // Old collection gone (only one belongs-to-collection remains).
        assert_eq!(out.matches("belongs-to-collection").count(), 1);
    }

    const EPUB2: &str = r##"<?xml version="1.0"?>
<package version="2.0" unique-identifier="uid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="uid" opf:scheme="uuid">1234</dc:identifier>
    <dc:title>Acımak</dc:title>
    <dc:creator opf:role="aut" opf:file-as="Güntekin, Reşat Nuri">Reşat Nuri Güntekin</dc:creator>
    <dc:creator opf:role="trl">Ayşe Çevirmen</dc:creator>
    <dc:language>tr</dc:language>
    <meta name="cover" content="cover"/>
  </metadata>
</package>"##;

    /// Calibre 0.7-era output: OPF as `ns0:`, Dublin Core declared per element.
    const EPUB2_NS0: &str = r##"<ns0:package unique-identifier="uuid_id" version="2.0" xmlns:ns0="http://www.idpf.org/2007/opf">
  <ns0:metadata>
    <ns1:title xmlns:ns1="http://purl.org/dc/elements/1.1/">Bye Bye</ns1:title>
    <ns1:creator ns0:file-as="Sinanoğlu, Oktay" ns0:role="aut" xmlns:ns1="http://purl.org/dc/elements/1.1/">Oktay Sinanoğlu</ns1:creator>
    <ns1:identifier id="uuid_id" ns0:scheme="uuid" xmlns:ns1="http://purl.org/dc/elements/1.1/">1e3e</ns1:identifier>
  </ns0:metadata>
</ns0:package>"##;

    fn full_update() -> MetadataUpdate {
        MetadataUpdate {
            title: Some("Acımak".into()),
            authors: Some(vec![
                "Reşat Nuri Güntekin".into(),
                "Ayşe Çevirmen".into(),
                "Yeni Yazar".into(),
            ]),
            language: Some("tr".into()),
            publisher: Some("İnkılap".into()),
            series: Some(Series {
                name: "Klasikler".into(),
                position: Some("4".into()),
            }),
            isbn: Some("9789751000000".into()),
            ..MetadataUpdate::default()
        }
    }

    #[test]
    fn epub2_is_written_in_epub2_syntax() {
        let out = apply_update(EPUB2, &full_update(), "2026-10-08T00:00:00Z").unwrap();
        for epub3_only in [
            "refines",
            "property=",
            "dcterms:modified",
            "belongs-to-collection",
        ] {
            assert!(!out.contains(epub3_only), "{epub3_only} in {out}");
        }
        assert!(out.contains(r#"<dc:creator opf:role="aut" opf:file-as="Güntekin, Reşat Nuri">Reşat Nuri Güntekin</dc:creator>"#), "{out}");
        assert!(
            out.contains(r#"<meta name="calibre:series" content="Klasikler"/>"#),
            "{out}"
        );
        assert!(out.contains(r#"<meta name="cover" content="cover"/>"#));
        let md = parse_metadata(&out).unwrap();
        let roles: Vec<_> = md
            .creators
            .iter()
            .map(|c| c.role.as_deref().unwrap())
            .collect();
        assert_eq!(roles, ["aut", "trl", "aut"]);
        assert_eq!(md.series.unwrap().position.as_deref(), Some("4"));
        let isbn = md
            .identifiers
            .iter()
            .find(|i| i.scheme.as_deref() == Some("ISBN"))
            .unwrap();
        assert_eq!(isbn.value, "9789751000000");
        assert!(md.identifiers.iter().any(|i| i.unique && i.value == "1234"));
    }

    #[test]
    fn epub2_with_its_own_prefixes_still_parses() {
        let update = MetadataUpdate {
            authors: Some(vec!["Oktay Sinanoğlu".into(), "Yeni Yazar".into()]),
            ..full_update()
        };
        let out = apply_update(EPUB2_NS0, &update, "2026-10-08T00:00:00Z").unwrap();
        roxmltree::Document::parse(&out).expect("well-formed");
        assert!(
            out.contains(r#"<ns0:meta name="calibre:series" content="Klasikler"/>"#),
            "{out}"
        );
        let md = parse_metadata(&out).unwrap();
        assert_eq!(md.titles[0].value, "Acımak");
        // The author who stayed keeps the sort name Calibre wrote with `ns0:`.
        assert_eq!(md.creators[0].file_as.as_deref(), Some("Sinanoğlu, Oktay"));
        assert!(out.contains(r#"ns0:file-as="Sinanoğlu, Oktay""#), "{out}");
        assert_eq!(md.publisher.as_deref(), Some("İnkılap"));
        assert_eq!(md.creators.len(), 2);
    }

    #[test]
    fn epub2_refuses_a_subtitle_it_cannot_store() {
        let update = MetadataUpdate {
            subtitle: Some("Alt".into()),
            ..MetadataUpdate::default()
        };
        assert!(apply_update(EPUB2, &update, "t").is_err());
    }

    #[test]
    fn an_author_who_stays_keeps_role_and_sort_name_in_epub3() {
        let update = MetadataUpdate {
            authors: Some(vec!["Roald Dahl".into()]),
            ..MetadataUpdate::default()
        };
        let out = apply_update(SAMPLE, &update, "2026-10-08T00:00:00Z").unwrap();
        let md = parse_metadata(&out).unwrap();
        assert_eq!(md.creators[0].file_as.as_deref(), Some("Dahl, Roald"));
        assert_eq!(md.creators[0].role.as_deref(), Some("aut"));
    }

    #[test]
    fn series_positions_may_be_ranges_and_names_may_hold_colons() {
        let s = Series::parse("Dune:1-6");
        assert_eq!(
            (s.name.as_str(), s.position.as_deref()),
            ("Dune", Some("1-6"))
        );
        let s = Series::parse("Star Wars: Legends");
        assert_eq!((s.name.as_str(), s.position), ("Star Wars: Legends", None));
        assert_eq!(Series::parse("Dune:2.5").position.as_deref(), Some("2.5"));
    }

    #[test]
    fn saving_the_same_values_changes_nothing_the_form_does_not_carry() {
        let opf = EPUB2.replace(
            "<dc:language>tr</dc:language>",
            "<dc:language>tr</dc:language><dc:language>en</dc:language>\n    <dc:identifier opf:scheme=\"ISBN\">978-975-10-0000-0</dc:identifier>",
        );
        let update = MetadataUpdate {
            title: Some("Acımak".into()),
            authors: Some(vec!["Reşat Nuri Güntekin".into(), "Ayşe Çevirmen".into()]),
            language: Some("tr".into()),
            isbn: Some("9789751000000".into()),
            ..MetadataUpdate::default()
        };
        let out = apply_update(&opf, &update, "t").unwrap();
        let md = parse_metadata(&out).unwrap();
        assert_eq!(md.languages, ["tr", "en"]);
        assert_eq!(
            md.identifiers
                .iter()
                .filter(|i| i.scheme.as_deref() == Some("ISBN"))
                .count(),
            1
        );
        assert!(out.contains(r#"opf:file-as="Güntekin, Reşat Nuri""#));
    }

    #[test]
    fn a_name_listed_twice_keeps_each_entry() {
        let opf = EPUB2.replace("Ayşe Çevirmen", "Reşat Nuri Güntekin");
        let update = MetadataUpdate {
            authors: Some(vec![
                "Reşat Nuri Güntekin".into(),
                "Reşat Nuri Güntekin".into(),
                "Yeni".into(),
            ]),
            ..MetadataUpdate::default()
        };
        let md = parse_metadata(&apply_update(&opf, &update, "t").unwrap()).unwrap();
        let roles: Vec<_> = md
            .creators
            .iter()
            .map(|c| c.role.as_deref().unwrap())
            .collect();
        assert_eq!(roles, ["aut", "trl", "aut"]);
    }
}
