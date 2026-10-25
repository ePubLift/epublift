//! Presentational attributes that XHTML 1.1 (EPUB 2) allowed and HTML (EPUB 3)
//! does not, written as the CSS the HTML Standard maps them to.
//!
//! HTML renders these attributes as *presentational hints*: author-level,
//! zero-specificity declarations (HTML §15.2), with the mappings of §15.3.8
//! (tables) and §15.4.3 (images). The upgrade writes each one as a class rule
//! in one stylesheet of its own, `epublift-presentational.css`, so a book that
//! was valid as EPUB 2 stays valid as EPUB 3 and looks the same. A class
//! rather than a `style` attribute keeps the declaration a low-precedence
//! rule, as the hint was, and old reading systems read it (`:where()` would be
//! closer still, but Kobo's engine drops the whole rule).
//!
//! An attribute is converted only when nothing can change because of it:
//!
//! - styloria's cascade says **No**, no author declaration sets the property,
//!   for every element the hint applies to. `Yes` (the book's CSS already
//!   decides) and `Unknown` (it may) leave the attribute as written, counted
//!   in the [`Summary`];
//! - the book's own selectors match exactly as before: every element of the
//!   document has the same cascade with the attributes removed and the classes
//!   added (`img[width]`, `td[class="c"]` would notice). Otherwise the
//!   document is left as it is;
//! - every declaration written is a valid one (`check_declaration`).
//!
//! Obsolete elements are renamed to their HTML counterparts the same way
//! ([`plan_element`]): `strike` → `s`, `big` and `font` → `span`, `center` →
//! `div`, with a class rule for what the old element's rendering added.
//!
//! A value HTML reads the same way in a valid form is rewritten instead
//! (`width="60px"` → `"60"`, `border="yes"` → `"1"`), and one that gives no
//! hint at all (`width="auto"`, `border="0"`) is removed: neither needs CSS.

use crate::util;
use anyhow::Result;
use roxmltree::{Node, NodeId};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use styloria::{
    BlockItem, Cascade, Element, ImportRule, Rule, Sheet, Stylesheet, Token, Tokenizer, ValueCheck,
    check_declaration, decode_stylesheet, parse_block_contents, parse_import_rule,
    parse_stylesheet,
};
use walkdir::WalkDir;

const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// The stylesheet the upgrade writes, beside the package document.
pub const STYLESHEET_NAME: &str = "epublift-presentational.css";

/// What the conversion did to one book.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Attributes written as CSS, rewritten or removed.
    pub converted: usize,
    /// Obsolete elements (`font`, `big`, `center`, `strike`) renamed to their
    /// HTML counterparts, with CSS where the rendering needs it.
    pub elements: usize,
    /// Content documents changed.
    pub documents: usize,
    /// Attributes left as written: the book's CSS sets the property.
    pub left_css_sets: usize,
    /// Attributes left as written: the book's CSS may set the property
    /// (a rule under `@media`, a selector styloria can't settle, …).
    pub left_css_may_set: usize,
    /// Attributes left as written: the book's selectors read the attribute
    /// or the class, so the document was left as it is.
    pub left_selectors: usize,
    /// The stylesheet written, relative to the package document's directory;
    /// `None` when no attribute needed CSS.
    pub stylesheet: Option<String>,
}

impl Summary {
    /// Attributes left as written, for any reason.
    pub fn left(&self) -> usize {
        self.left_css_sets + self.left_css_may_set + self.left_selectors
    }
}

/// Convert the presentational attributes of every content document under
/// `root` (the unpacked book). The stylesheet, when one is needed, is written
/// into `package_dir` under [`STYLESHEET_NAME`] (or a free variant of it) and
/// linked from each document that uses it; the caller adds it to the manifest.
pub fn convert_book(root: &Path, package_dir: &Path, progress: &dyn Fn(&str)) -> Result<Summary> {
    let mut summary = Summary::default();
    let css_path = free_name(package_dir);
    let mut rules: BTreeMap<String, String> = BTreeMap::new();

    let mut docs: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(e.to_ascii_lowercase().as_str(), "xhtml" | "html" | "htm")
            })
        })
        .collect();
    docs.sort();

    for doc_path in docs {
        let Ok(text) = fs::read_to_string(&doc_path) else {
            continue;
        };
        let Some(out) =
            convert_document(root, &doc_path, &css_path, &text, &mut summary, &mut rules)
        else {
            continue;
        };
        fs::write(&doc_path, out)?;
        summary.documents += 1;
    }

    if !rules.is_empty() {
        let mut css = String::from(
            "/* Written by epublift: presentational attributes that EPUB 2 allowed and\n   \
             EPUB 3 does not, as the CSS the HTML Standard maps them to. */\n",
        );
        for (class, decls) in &rules {
            css.push_str(&format!(".{class} {{ {decls} }}\n"));
        }
        // Each rule passed `rule_clean` on its own; the sheet must too.
        debug_assert!(
            rule_clean(&css),
            "the written stylesheet has findings:\n{css}"
        );
        fs::write(&css_path, css)?;
        summary.stylesheet = css_path
            .file_name()
            .map(|n| util::quote(&n.to_string_lossy()));
    }
    if summary.converted + summary.elements > 0 {
        progress(&format!(
            "  [+] Wrote {} presentational attribute(s) and {} element(s) in {} document(s) as \
             HTML and CSS.",
            summary.converted, summary.elements, summary.documents
        ));
    }
    if summary.left() > 0 {
        progress(&format!(
            "  [!] Left {} presentational attribute(s) as written: the book's CSS sets ({}) or may \
             set ({}) the property, or its selectors read them ({}).",
            summary.left(),
            summary.left_css_sets,
            summary.left_css_may_set,
            summary.left_selectors
        ));
    }
    Ok(summary)
}

/// `epublift-presentational.css` in `dir`, or `-2`, `-3`, … if taken.
fn free_name(dir: &Path) -> PathBuf {
    let first = dir.join(STYLESHEET_NAME);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("epublift-presentational-{n}.css")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

// ---- one document -----------------------------------------------------------

/// What happens to the attribute itself, or to the element.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    Remove,
    Rewrite(String),
    /// An obsolete element renamed (`big` → `span`), losing these attributes.
    Element {
        rename: &'static str,
        remove: Vec<&'static str>,
    },
}

/// One class rule on one element: its declarations and the properties that
/// must be unset by the book's CSS there.
#[derive(Debug, Clone)]
struct Decl {
    class: String,
    css: String,
    props: &'static [&'static str],
}

/// One attribute's (or element's) conversion: its fate, and the rules it needs.
#[derive(Debug, Clone)]
struct Conversion {
    owner: NodeId,
    /// The attribute, or the element's name for [`Action::Element`].
    attr: &'static str,
    action: Action,
    targets: Vec<(NodeId, Decl)>,
    /// Further elements on which the book's CSS must not set these
    /// properties (a `center`'s content must not be made a block).
    guards: Vec<(NodeId, &'static [&'static str])>,
}

impl Conversion {
    fn is_element(&self) -> bool {
        matches!(self.action, Action::Element { .. })
    }
}

/// Returns the document's new text, or `None` when nothing changed.
fn convert_document(
    root: &Path,
    doc_path: &Path,
    css_path: &Path,
    text: &str,
    summary: &mut Summary,
    rules: &mut BTreeMap<String, String>,
) -> Option<String> {
    let doc = util::parse_xml_with_dtd(text).ok()?;
    // A declaration a browser would drop is a bug in the mapping, never shipped;
    // nor is a rule epubveri would report in the stylesheet.
    let candidates: Vec<Conversion> = plan(&doc)
        .into_iter()
        .filter(|c| {
            c.targets.iter().all(|(_, d)| {
                let ok = declarations_valid(&d.css)
                    && rule_clean(&format!(".{} {{ {} }}", d.class, d.css));
                debug_assert!(ok, "invalid declaration written: {}", d.css);
                ok
            })
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let head = doc
        .descendants()
        .find(|n| n.is_element() && is_html(n, "head"))?;

    let sources = load_sheets(root, doc_path, &doc);
    let parsed: Vec<Stylesheet<'_>> = sources
        .iter()
        .map(|s| parse_stylesheet(&s.text).0)
        .collect();
    let top: Vec<Sheet<'_, '_>> = sources
        .iter()
        .zip(&parsed)
        .filter(|(s, _)| s.top_level)
        .map(|(s, p)| Sheet {
            stylesheet: p,
            conditional: s.conditional,
        })
        .collect();
    let cascade = Cascade::with_imports(&top, |from, rule: &ImportRule<'_>| {
        let i = parsed.iter().position(|p| std::ptr::eq(p, from))?;
        let target = resolve(root, &sources[i].base, &rule.url)?;
        let j = sources
            .iter()
            .position(|s| s.path.as_deref() == Some(target.as_path()))?;
        Some(&parsed[j])
    });

    // Every target must be unset by the book's CSS, for every property.
    let mut accepted = Vec::new();
    for c in candidates {
        let mut status = styloria::Match::No;
        for (id, decl) in &c.targets {
            let node = doc.get_node(*id)?;
            let el = El {
                node,
                overlay: None,
            };
            let inline_text = node.attribute("style").unwrap_or("");
            let inline = parse_block_contents(inline_text).0;
            let style = cascade.style_for(&el, Some(inline.as_slice()));
            for p in decl.props {
                status = status.or(style.property(p).status);
            }
        }
        for (id, props) in &c.guards {
            let node = doc.get_node(*id)?;
            let inline = parse_block_contents(node.attribute("style").unwrap_or("")).0;
            let style = cascade.style_for(
                &El {
                    node,
                    overlay: None,
                },
                Some(inline.as_slice()),
            );
            for p in *props {
                status = status.or(style.property(p).status);
            }
        }
        match status {
            styloria::Match::No => accepted.push(c),
            styloria::Match::Yes => summary.left_css_sets += 1,
            styloria::Match::Unknown => summary.left_css_may_set += 1,
        }
    }
    if accepted.is_empty() {
        return None;
    }

    // A class name already taken by another rule (two font faces with one
    // slug) gets a suffix.
    let mut pending: BTreeMap<String, String> = BTreeMap::new();
    for c in accepted.iter_mut() {
        for (_, d) in c.targets.iter_mut() {
            let base = d.class.clone();
            let mut n = 2;
            while rules
                .get(&d.class)
                .or(pending.get(&d.class))
                .is_some_and(|css| *css != d.css)
            {
                d.class = format!("{base}-{n}");
                n += 1;
            }
            pending.insert(d.class.clone(), d.css.clone());
        }
    }

    // The book's selectors must match as before.
    let overlay = overlay(&doc, &accepted);
    let elements: Vec<Node<'_, '_>> = doc.descendants().filter(|n| n.is_element()).collect();
    for node in &elements {
        let inline = parse_block_contents(node.attribute("style").unwrap_or("")).0;
        let before = signature(
            &cascade,
            &El {
                node: *node,
                overlay: None,
            },
            &inline,
        );
        let after = signature(
            &cascade,
            &El {
                node: *node,
                overlay: Some(&overlay),
            },
            &inline,
        );
        if before != after {
            summary.left_selectors += accepted.len();
            return None;
        }
    }

    // Edits, applied from the end of the text backwards.
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for (id, change) in &overlay {
        let node = doc.get_node(*id)?;
        for name in &change.removed {
            let a = node
                .attributes()
                .find(|a| a.namespace().is_none() && a.name() == *name)?;
            let r = a.range();
            let start = text[..r.start]
                .trim_end_matches([' ', '\t', '\n', '\r'])
                .len();
            edits.push((start..r.end, String::new()));
        }
        for (name, value) in &change.replaced {
            let a = node
                .attributes()
                .find(|a| a.namespace().is_none() && a.name() == *name)?;
            edits.push((a.range_value(), value.clone()));
        }
        if let Some(new) = change.rename {
            let name_start = node.range().start + 1;
            let name_end = tag_name_end(text, node);
            let old = &text[name_start..name_end];
            if old.contains(':') {
                return None;
            }
            edits.push((name_start..name_end, new.to_string()));
            let whole = &text[node.range()];
            if !whole.trim_end().ends_with("/>") {
                let close = node.range().start + whole.rfind("</")? + 2;
                if !text[close..].starts_with(old) {
                    return None;
                }
                edits.push((close..close + old.len(), new.to_string()));
            }
        }
        if !change.added.is_empty() {
            let added = change.added.join(" ");
            match node
                .attributes()
                .find(|a| a.namespace().is_none() && a.name() == "class")
            {
                Some(a) => {
                    let r = a.range_value();
                    let sep = if a.value().trim().is_empty() { "" } else { " " };
                    edits.push((r.end..r.end, format!("{sep}{added}")));
                }
                None => {
                    let at = tag_name_end(text, node);
                    edits.push((at..at, format!(" class=\"{added}\"")));
                }
            }
        }
    }
    // The stylesheet, linked first in `<head>` (a hint precedes the author's
    // CSS), when a class rule is used here.
    if accepted.iter().any(|c| !c.targets.is_empty()) {
        let head_qname = &text[head.range().start + 1..tag_name_end(text, head)];
        if head_qname.contains(':') || text[head.range()].trim_end().ends_with("/>") {
            return None;
        }
        let head_open = text[head.range().start..].find('>')? + head.range().start + 1;
        let href = relative_href(doc_path.parent()?, css_path)?;
        edits.push((
            head_open..head_open,
            format!("\n<link href=\"{href}\" rel=\"stylesheet\" type=\"text/css\"/>"),
        ));
    }
    edits.sort_by(|a, b| b.0.start.cmp(&a.0.start).then(b.0.end.cmp(&a.0.end)));

    let mut out = text.to_string();
    for (r, s) in edits {
        out.replace_range(r, &s);
    }
    rules.extend(pending);
    let elements = accepted.iter().filter(|c| c.is_element()).count();
    summary.elements += elements;
    summary.converted += accepted.len() - elements;
    Some(out)
}

/// The end of an element's tag name in its start tag (`<img|`).
fn tag_name_end(text: &str, node: Node<'_, '_>) -> usize {
    let start = node.range().start + 1;
    start
        + text[start..]
            .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
            .unwrap_or(0)
}

/// Every conversion the document's markup asks for, before any CSS is read.
fn plan(doc: &roxmltree::Document<'_>) -> Vec<Conversion> {
    let mut out = Vec::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        if node.tag_name().namespace() != Some(XHTML_NS) {
            continue;
        }
        let name = node.tag_name().name();
        if let Some(c) = plan_element(node, name) {
            out.push(c);
        }
        for attr in node.attributes().filter(|a| a.namespace().is_none()) {
            if let Some(c) = plan_attribute(node, name, attr.name(), attr.value()) {
                out.push(c);
            }
        }
    }
    out
}

fn plan_attribute(
    node: Node<'_, '_>,
    element: &str,
    attr: &str,
    value: &str,
) -> Option<Conversion> {
    let id = node.id();
    let own = |attr: &'static str, decl: Option<Decl>| -> Conversion {
        match decl {
            Some(d) => Conversion {
                owner: id,
                attr,
                action: Action::Remove,
                targets: vec![(id, d)],
                guards: Vec::new(),
            },
            None => Conversion {
                owner: id,
                attr,
                action: Action::Remove,
                targets: Vec::new(),
                guards: Vec::new(),
            },
        }
    };
    let rewrite = |attr: &'static str, v: String| Conversion {
        owner: id,
        attr,
        action: Action::Rewrite(v),
        targets: Vec::new(),
        guards: Vec::new(),
    };
    match (element, attr) {
        // §15.4.3: maps to the dimension properties; the attributes must be
        // valid non-negative integers (§4.8.17).
        ("img", "width" | "height") => {
            if is_valid_integer(value) {
                return None;
            }
            let attr = if attr == "width" { "width" } else { "height" };
            Some(match parse_dimension(value, false) {
                None => own(attr, None),
                Some(Dim::Length(n)) if !n.contains('.') => rewrite(attr, n),
                Some(d) => own(attr, Some(dimension_decl(attr, &d))),
            })
        }
        // §15.3.8: pixel length property `border-spacing`.
        ("table", "cellspacing") => Some(match parse_non_negative_integer(value) {
            None => own("cellspacing", None),
            Some(n) => own("cellspacing", Some(px_decl("border-spacing", n))),
        }),
        // §15.3.8: pixel length `padding-*` on the table's own cells.
        ("table", "cellpadding") => {
            let targets = match parse_non_negative_integer(value) {
                None => Vec::new(),
                Some(n) => own_cells(node)
                    .into_iter()
                    .map(|c| (c, px_decl("padding", n)))
                    .collect(),
            };
            Some(Conversion {
                owner: id,
                attr: "cellpadding",
                action: Action::Remove,
                targets,
                guards: Vec::new(),
            })
        }
        // §15.3.8: `border` → border widths on the table, and the UA rules
        // keyed on `table[border]` (outset table, 1px inset cells) unless it
        // is equivalent to zero. Conforming values are "" and "1".
        ("table", "border") => {
            if value.is_empty() || value == "1" {
                return None;
            }
            Some(match parse_non_negative_integer(value) {
                None => rewrite("border", "1".to_string()),
                Some(0) => own("border", None),
                Some(n) => {
                    let mut targets = vec![(
                        id,
                        Decl {
                            class: format!("epublift-table-border-{n}px"),
                            css: format!("border-width: {n}px; border-style: outset"),
                            props: &["border-width", "border-style"],
                        },
                    )];
                    targets.extend(own_cells(node).into_iter().map(|c| {
                        (
                            c,
                            Decl {
                                class: "epublift-cell-border".to_string(),
                                css: "border-width: 1px; border-style: inset".to_string(),
                                props: &["border-width", "border-style"],
                            },
                        )
                    }));
                    Conversion {
                        owner: id,
                        attr: "border",
                        action: Action::Remove,
                        targets,
                        guards: Vec::new(),
                    }
                }
            })
        }
        // §15.3.8: table `width` ignores zero, `height` does not; cells
        // ignore zero for both; rows and row groups map `height`.
        ("table", "width") | ("td" | "th", "width" | "height") => {
            let attr = if attr == "width" { "width" } else { "height" };
            Some(own(
                attr,
                parse_dimension(value, true).map(|d| dimension_decl(attr, &d)),
            ))
        }
        ("table" | "tr" | "thead" | "tbody" | "tfoot", "height") => Some(own(
            "height",
            parse_dimension(value, false).map(|d| dimension_decl("height", &d)),
        )),
        // §15.3.8: `valign` → `vertical-align` for these four keywords.
        ("td" | "th" | "tr" | "thead" | "tbody" | "tfoot", "valign") => {
            let v = value.to_ascii_lowercase();
            let decl =
                matches!(v.as_str(), "top" | "middle" | "bottom" | "baseline").then(|| Decl {
                    class: format!("epublift-vertical-align-{v}"),
                    css: format!("vertical-align: {v}"),
                    props: &["vertical-align"],
                });
            Some(own("valign", decl))
        }
        // §15.3.4: `br[clear]` → `clear`; `all` and `both` clear both sides,
        // any other value gives no hint.
        ("br", "clear") => {
            let v = match value.to_ascii_lowercase().as_str() {
                "left" => Some("left"),
                "right" => Some("right"),
                "all" | "both" => Some("both"),
                _ => None,
            };
            let decl = v.map(|v| Decl {
                class: format!("epublift-clear-{v}"),
                css: format!("clear: {v}"),
                props: &["clear"],
            });
            Some(own("clear", decl))
        }
        _ => None,
    }
}

/// HTML's counterpart of an obsolete element (§15.3.3, §15.3.4): `strike` is
/// `s` (the same UA rule draws both), `big` a `span` at `font-size: larger`,
/// `center` a `div` at `text-align: center`, and `font` a `span` with its
/// `face` as `font-family` and its `size` as the legacy keyword. Declined:
/// an attribute this does not know (`font color`, for now), `size="7"`
/// (`xxx-large` is too new for old readers), and a `center` holding anything
/// but phrasing content, which its legacy centering of blocks would treat
/// differently from `text-align`.
fn plan_element(node: Node<'_, '_>, element: &str) -> Option<Conversion> {
    const GLOBAL: &[&str] = &["id", "class", "style", "title", "lang", "dir"];
    let id = node.id();
    let unknown = |allowed: &[&str]| {
        node.attributes().any(|a| {
            a.namespace().is_none() && !GLOBAL.contains(&a.name()) && !allowed.contains(&a.name())
        })
    };
    let conversion = |attr, rename, remove: Vec<&'static str>, targets: Vec<Decl>| Conversion {
        owner: id,
        attr,
        action: Action::Element { rename, remove },
        targets: targets.into_iter().map(|d| (id, d)).collect(),
        guards: Vec::new(),
    };
    match element {
        "strike" if !unknown(&[]) => Some(conversion("strike", "s", Vec::new(), Vec::new())),
        "big" if !unknown(&[]) => Some(conversion(
            "big",
            "span",
            Vec::new(),
            vec![Decl {
                class: "epublift-big".to_string(),
                css: "font-size: larger".to_string(),
                props: &["font-size"],
            }],
        )),
        "center" if !unknown(&[]) => {
            const PHRASING: &[&str] = &[
                "a", "abbr", "b", "bdi", "bdo", "big", "br", "cite", "code", "data", "dfn", "em",
                "font", "i", "img", "kbd", "mark", "q", "s", "samp", "small", "span", "strike",
                "strong", "sub", "sup", "time", "tt", "u", "var", "wbr",
            ];
            let inner: Vec<Node<'_, '_>> = node
                .descendants()
                .skip(1)
                .filter(|n| n.is_element())
                .collect();
            if inner.iter().any(|n| {
                n.tag_name().namespace() != Some(XHTML_NS)
                    || !PHRASING.contains(&n.tag_name().name())
            }) {
                return None;
            }
            let mut c = conversion(
                "center",
                "div",
                Vec::new(),
                vec![Decl {
                    class: "epublift-center".to_string(),
                    css: "text-align: center".to_string(),
                    props: &["text-align"],
                }],
            );
            c.guards = inner.iter().map(|n| (n.id(), &["display"][..])).collect();
            Some(c)
        }
        "font" if !unknown(&["face", "size"]) => {
            let mut targets = Vec::new();
            if let Some(face) = node
                .attribute("face")
                .map(str::trim)
                .filter(|f| !f.is_empty())
            {
                targets.push(Decl {
                    class: format!("epublift-font-family-{}", slug(face)),
                    css: format!("font-family: {}", font_family_value(face)?),
                    props: &["font-family"],
                });
            }
            if let Some(size) = node.attribute("size") {
                match parse_legacy_font_size(size) {
                    Some(7) => return None,
                    Some(n) => {
                        let kw = ["x-small", "small", "medium", "large", "x-large", "xx-large"]
                            [n as usize - 1];
                        targets.push(Decl {
                            class: format!("epublift-font-size-{kw}"),
                            css: format!("font-size: {kw}"),
                            props: &["font-size"],
                        });
                    }
                    None => {}
                }
            }
            let present = ["face", "size"]
                .into_iter()
                .filter(|a| node.attribute(*a).is_some())
                .collect();
            Some(conversion("font", "span", present, targets))
        }
        _ => None,
    }
}

/// `face` as a `font-family` value that is safe to write: a comma-separated
/// list of family names, each one CSS string or a run of identifiers, written
/// back with styloria's serializer so it means what it meant and nothing more.
/// `None` for anything else — a comment, a `;` or a brace, an unterminated
/// string, a CSS-wide keyword — and the `font` element is left as it is.
fn font_family_value(face: &str) -> Option<String> {
    use styloria::serialize::{serialize_ident, serialize_string};
    const RESERVED: &[&str] = &[
        "inherit",
        "initial",
        "unset",
        "revert",
        "revert-layer",
        "default",
    ];

    let mut families: Vec<Vec<Token<'_>>> = vec![Vec::new()];
    let mut end = 0;
    for t in Tokenizer::new(face).spanned() {
        // The tokenizer skips comments, which leaves a gap between spans.
        if t.span.start != end {
            return None;
        }
        end = t.span.end;
        match t.node {
            Token::Whitespace => {}
            Token::Comma => families.push(Vec::new()),
            Token::Ident(_) => families.last_mut()?.push(t.node),
            Token::String(_) if string_closed(t.span.slice(face)) => {
                families.last_mut()?.push(t.node)
            }
            _ => return None,
        }
    }
    if end != face.len() {
        return None;
    }
    let mut out = Vec::new();
    for family in &families {
        let mut name = String::new();
        match family.as_slice() {
            [Token::String(v)] => serialize_string(v, &mut name),
            [] => return None,
            idents => {
                for t in idents {
                    let Token::Ident(v) = t else { return None };
                    if RESERVED.iter().any(|r| v.eq_ignore_ascii_case(r)) {
                        return None;
                    }
                    if !name.is_empty() {
                        name.push(' ');
                    }
                    serialize_ident(v, &mut name);
                }
            }
        }
        out.push(name);
    }
    Some(out.join(", "))
}

/// Whether a string token's source ends with its own closing quote; an
/// unterminated one runs to the end of the value instead.
fn string_closed(src: &str) -> bool {
    let Some(q) = src.chars().next() else {
        return false;
    };
    let body = &src[q.len_utf8()..];
    body.strip_suffix(q)
        .is_some_and(|inner| inner.chars().rev().take_while(|c| *c == '\\').count() % 2 == 0)
}

/// Whether epubveri finds nothing to report in `css`, as the stylesheet the
/// upgrade writes would hold it.
fn rule_clean(css: &str) -> bool {
    epubveri::check_style_text(css, STYLESHEET_NAME, &epubveri::Options::default())
        .messages
        .is_empty()
}

/// The rules for parsing a legacy font size (§15.3.4): `+N` and `-N` are
/// relative to 3, the result clamped to 1–7; `None` gives no hint.
fn parse_legacy_font_size(v: &str) -> Option<u8> {
    let s = v.trim_start_matches(is_html_space);
    let (mode, s) = match s.as_bytes().first() {
        Some(b'+') => (1, &s[1..]),
        Some(b'-') => (-1, &s[1..]),
        _ => (0, s),
    };
    let digits = &s[..s.bytes().take_while(u8::is_ascii_digit).count()];
    if digits.is_empty() {
        return None;
    }
    let n: i64 = digits.parse().unwrap_or(i64::MAX / 2);
    let n = match mode {
        1 => n.saturating_add(3),
        -1 => 3 - n,
        _ => n,
    };
    Some(n.clamp(1, 7) as u8)
}

/// A class-name fragment from a font family list (`Times New Roman, serif` →
/// `times-new-roman-serif`).
fn slug(v: &str) -> String {
    let mut out = String::new();
    for c in v.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "x".to_string() } else { out }
}

/// The `td` and `th` elements of a table's own rows (HTML's cell positions
/// `table > tr > td` and `table > thead|tbody|tfoot > tr > td`), not those of
/// a nested table.
fn own_cells(table: Node<'_, '_>) -> Vec<NodeId> {
    let rows = table.children().filter(|n| n.is_element()).flat_map(|c| {
        if is_html(&c, "tr") {
            vec![c]
        } else if ["thead", "tbody", "tfoot"].iter().any(|g| is_html(&c, g)) {
            c.children().filter(|r| is_html(r, "tr")).collect()
        } else {
            Vec::new()
        }
    });
    rows.flat_map(|r| {
        r.children()
            .filter(|c| is_html(c, "td") || is_html(c, "th"))
            .map(|c| c.id())
            .collect::<Vec<_>>()
    })
    .collect()
}

fn is_html(node: &Node<'_, '_>, name: &str) -> bool {
    node.is_element()
        && node.tag_name().namespace() == Some(XHTML_NS)
        && node.tag_name().name() == name
}

fn px_decl(prop: &'static str, n: u32) -> Decl {
    let props: &'static [&'static str] = match prop {
        "border-spacing" => &["border-spacing"],
        _ => &["padding"],
    };
    Decl {
        class: format!("epublift-{prop}-{n}px"),
        css: format!("{prop}: {n}px"),
        props,
    }
}

fn dimension_decl(prop: &'static str, d: &Dim) -> Decl {
    let props: &'static [&'static str] = if prop == "width" {
        &["width"]
    } else {
        &["height"]
    };
    let (value, slug) = match d {
        Dim::Length(n) => (format!("{n}px"), format!("{}px", n.replace('.', "_"))),
        Dim::Percent(n) => (format!("{n}%"), format!("{}pct", n.replace('.', "_"))),
    };
    Decl {
        class: format!("epublift-{prop}-{slug}"),
        css: format!("{prop}: {value}"),
        props,
    }
}

// ---- HTML microsyntaxes (§2.3.4) --------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Dim {
    Length(String),
    Percent(String),
}

/// A valid non-negative integer: one or more ASCII digits, nothing else.
fn is_valid_integer(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())
}

/// The rules for parsing non-negative integers: leading whitespace, an
/// optional `+`, digits; trailing characters are ignored.
fn parse_non_negative_integer(v: &str) -> Option<u32> {
    let s = v.trim_start_matches(is_html_space);
    let s = s.strip_prefix('+').unwrap_or(s);
    let digits: &str = &s[..s.bytes().take_while(u8::is_ascii_digit).count()];
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// The rules for parsing dimension values (or nonzero ones): a number, a
/// percentage if `%` follows it at once, a length otherwise; trailing
/// characters are ignored. The number comes back normalised (`007.50` →
/// `7.5`).
fn parse_dimension(v: &str, nonzero: bool) -> Option<Dim> {
    let s = v.trim_start_matches(is_html_space);
    let int_len = s.bytes().take_while(u8::is_ascii_digit).count();
    if int_len == 0 {
        return None;
    }
    let mut rest = &s[int_len..];
    let int = s[..int_len].trim_start_matches('0');
    let mut frac = "";
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.bytes().take_while(u8::is_ascii_digit).count();
        frac = r[..n].trim_end_matches('0');
        rest = &r[n..];
    }
    let int = if int.is_empty() { "0" } else { int };
    if nonzero && int == "0" && frac.is_empty() {
        return None;
    }
    let n = if frac.is_empty() {
        int.to_string()
    } else {
        format!("{int}.{frac}")
    };
    Some(if rest.starts_with('%') {
        Dim::Percent(n)
    } else {
        Dim::Length(n)
    })
}

fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{c}' | '\r')
}

// ---- the overlay: the document as it will be ---------------------------------

#[derive(Debug, Default)]
struct Change {
    /// The element's new name.
    rename: Option<&'static str>,
    removed: Vec<&'static str>,
    replaced: Vec<(&'static str, String)>,
    added: Vec<String>,
    /// The new `class` value, when classes are added.
    class: Option<String>,
}

fn overlay(doc: &roxmltree::Document<'_>, accepted: &[Conversion]) -> HashMap<NodeId, Change> {
    let mut map: HashMap<NodeId, Change> = HashMap::new();
    for c in accepted {
        let ch = map.entry(c.owner).or_default();
        match &c.action {
            Action::Remove => ch.removed.push(c.attr),
            Action::Rewrite(v) => ch.replaced.push((c.attr, v.clone())),
            Action::Element { rename, remove } => {
                ch.rename = Some(rename);
                ch.removed.extend(remove);
            }
        }
        for (id, d) in &c.targets {
            let ch = map.entry(*id).or_default();
            if !ch.added.contains(&d.class) {
                ch.added.push(d.class.clone());
            }
        }
    }
    for (id, ch) in map.iter_mut() {
        if ch.added.is_empty() {
            continue;
        }
        let old = doc
            .get_node(*id)
            .and_then(|n| n.attribute("class"))
            .unwrap_or("")
            .trim();
        let added = ch.added.join(" ");
        ch.class = Some(if old.is_empty() {
            added
        } else {
            format!("{old} {added}")
        });
    }
    map
}

/// An element as the cascade sees it, optionally with the planned changes.
#[derive(Clone)]
struct El<'d, 'i, 'o> {
    node: Node<'d, 'i>,
    overlay: Option<&'o HashMap<NodeId, Change>>,
}

impl<'d, 'i, 'o> El<'d, 'i, 'o> {
    fn with(&self, node: Node<'d, 'i>) -> Self {
        El {
            node,
            overlay: self.overlay,
        }
    }
}

impl Element for El<'_, '_, '_> {
    fn parent(&self) -> Option<Self> {
        self.node.parent_element().map(|n| self.with(n))
    }
    fn prev_sibling(&self) -> Option<Self> {
        self.node.prev_sibling_element().map(|n| self.with(n))
    }
    fn next_sibling(&self) -> Option<Self> {
        self.node.next_sibling_element().map(|n| self.with(n))
    }
    fn has_children(&self) -> bool {
        self.node
            .children()
            .any(|c| c.is_element() || (c.is_text() && !c.text().unwrap_or("").is_empty()))
    }
    fn local_name(&self) -> &str {
        self.overlay
            .and_then(|o| o.get(&self.node.id()))
            .and_then(|ch| ch.rename)
            .unwrap_or(self.node.tag_name().name())
    }
    fn namespace(&self) -> Option<&str> {
        self.node.tag_name().namespace()
    }
    fn attribute(&self, local_name: &str) -> Option<&str> {
        if let Some(ch) = self.overlay.and_then(|o| o.get(&self.node.id())) {
            if local_name == "class"
                && let Some(c) = &ch.class
            {
                return Some(c);
            }
            if ch.removed.contains(&local_name) {
                return None;
            }
            if let Some((_, v)) = ch.replaced.iter().find(|(n, _)| *n == local_name) {
                return Some(v);
            }
        }
        self.node.attribute(local_name)
    }
    fn attribute_ns(&self, namespace_uri: &str, local_name: &str) -> Option<&str> {
        self.node.attribute((namespace_uri, local_name))
    }
    fn is_html_document(&self) -> bool {
        false
    }
}

/// Which declarations reach an element, in order: what must not change.
fn signature(
    cascade: &Cascade<'_, '_>,
    el: &El<'_, '_, '_>,
    inline: &[BlockItem<'_>],
) -> Vec<usize> {
    cascade
        .style_for(el, Some(inline))
        .candidates
        .iter()
        .map(|c| c.declaration as *const _ as usize)
        .collect()
}

// ---- the document's stylesheets ----------------------------------------------

struct Source {
    /// The file, for a linked or imported sheet; `None` for a `<style>`.
    path: Option<PathBuf>,
    /// The directory its relative URLs resolve against.
    base: PathBuf,
    text: String,
    conditional: bool,
    /// Linked or embedded by the document (not reached through `@import`).
    top_level: bool,
}

/// The document's stylesheets in document order (`<link rel=stylesheet>`,
/// `<style>`), then every sheet they import.
fn load_sheets(root: &Path, doc_path: &Path, doc: &roxmltree::Document<'_>) -> Vec<Source> {
    let doc_dir = doc_path.parent().unwrap_or(root).to_path_buf();
    let mut out: Vec<Source> = Vec::new();
    for n in doc.descendants().filter(|n| n.is_element()) {
        let conditional = n.attribute("media").is_some_and(|m| {
            let m = m.trim().to_ascii_lowercase();
            !(m.is_empty() || m == "all")
        });
        if is_html(&n, "link") {
            let rel = n.attribute("rel").unwrap_or("").to_ascii_lowercase();
            let words: Vec<&str> = rel.split_ascii_whitespace().collect();
            if !words.contains(&"stylesheet") {
                continue;
            }
            let Some(path) = n.attribute("href").and_then(|h| resolve(root, &doc_dir, h)) else {
                continue;
            };
            if let Some(text) = read_sheet(&path) {
                out.push(Source {
                    base: path.parent().unwrap_or(root).to_path_buf(),
                    path: Some(path),
                    text,
                    conditional: conditional || words.contains(&"alternate"),
                    top_level: true,
                });
            }
        } else if is_html(&n, "style") {
            if n.attribute("type")
                .is_some_and(|t| !t.trim().eq_ignore_ascii_case("text/css"))
            {
                continue;
            }
            let text: String = n.children().filter_map(|c| c.text()).collect();
            out.push(Source {
                path: None,
                base: doc_dir.clone(),
                text,
                conditional,
                top_level: true,
            });
        }
    }
    // Imports, breadth first; each file once.
    let mut i = 0;
    while i < out.len() && out.len() < 256 {
        let urls: Vec<String> = parse_stylesheet(&out[i].text)
            .0
            .rules
            .iter()
            .filter_map(|r| match &r.node {
                Rule::At(a) if a.name.eq_ignore_ascii_case("import") => {
                    parse_import_rule(&a.prelude).map(|ir| ir.url.into_owned())
                }
                _ => None,
            })
            .collect();
        let base = out[i].base.clone();
        for u in urls {
            let Some(path) = resolve(root, &base, &u) else {
                continue;
            };
            if out
                .iter()
                .any(|s| s.path.as_deref() == Some(path.as_path()))
            {
                continue;
            }
            if let Some(text) = read_sheet(&path) {
                out.push(Source {
                    base: path.parent().unwrap_or(root).to_path_buf(),
                    path: Some(path),
                    text,
                    conditional: false,
                    top_level: false,
                });
            }
        }
        i += 1;
    }
    out
}

fn read_sheet(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let decoded = decode_stylesheet(&bytes, None, Some("utf-8")).ok()?;
    Some(decoded.text.into_owned())
}

/// A relative URL in the book, as a path under `root`; `None` for anything
/// else (a scheme, an absolute path, a step out of the book).
fn resolve(root: &Path, base: &Path, url: &str) -> Option<PathBuf> {
    let url = url.split(['#', '?']).next().unwrap_or("");
    if url.is_empty() || url.starts_with('/') || url.contains(':') {
        return None;
    }
    let mut path = PathBuf::new();
    for c in base.join(util::unquote(url)).components() {
        match c {
            Component::ParentDir => {
                path.pop();
            }
            Component::CurDir => {}
            other => path.push(other),
        }
    }
    (path.starts_with(root) && path.is_file()).then_some(path)
}

/// `to` as a URL relative to the directory `from` (both under the book).
fn relative_href(from: &Path, to: &Path) -> Option<String> {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_string(); from.len() - common];
    for c in &to[common..] {
        parts.push(util::quote(&c.as_os_str().to_string_lossy()));
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Whether every declaration of a rule body is valid CSS (`check_declaration`).
fn declarations_valid(css: &str) -> bool {
    let items = parse_block_contents(css).0;
    !items.is_empty()
        && items.iter().all(|i| match i {
            BlockItem::Declaration(d) => check_declaration(&d.node) == ValueCheck::Valid,
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
        <html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>t</title>";

    /// A book with one document and one stylesheet; returns (root, summary,
    /// the document's new text, the written CSS).
    fn run(
        css: &str,
        head_extra: &str,
        body: &str,
    ) -> (tempfile::TempDir, Summary, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("OEBPS");
        fs::create_dir_all(pkg.join("Text")).unwrap();
        fs::create_dir_all(pkg.join("Styles")).unwrap();
        fs::write(pkg.join("Styles/s.css"), css).unwrap();
        let doc = format!(
            "{HEAD}<link href=\"../Styles/s.css\" rel=\"stylesheet\" type=\"text/css\"/>{head_extra}</head><body>{body}</body></html>"
        );
        fs::write(pkg.join("Text/a.xhtml"), doc).unwrap();
        let summary = convert_book(dir.path(), &pkg, &|_| {}).unwrap();
        let text = fs::read_to_string(pkg.join("Text/a.xhtml")).unwrap();
        let out_css = fs::read_to_string(pkg.join(STYLESHEET_NAME)).unwrap_or_default();
        (dir, summary, text, out_css)
    }

    #[test]
    fn percentage_image_width_becomes_a_class_rule() {
        let (_d, s, text, css) = run(
            "p { margin: 0 }",
            "",
            "<p><img src=\"x.jpg\" alt=\"\" width=\"40%\"/></p>",
        );
        assert_eq!(s.converted, 1);
        assert_eq!(s.stylesheet.as_deref(), Some(STYLESHEET_NAME));
        assert!(
            text.contains("<img class=\"epublift-width-40pct\" src=\"x.jpg\" alt=\"\"/>"),
            "{text}"
        );
        assert!(
            text.contains(
                "<head>\n<link href=\"../epublift-presentational.css\" rel=\"stylesheet\""
            ),
            "{text}"
        );
        assert!(
            css.contains(".epublift-width-40pct { width: 40% }"),
            "{css}"
        );
        crate::util::parse_xml_with_dtd(&text).unwrap();
    }

    #[test]
    fn existing_class_is_extended() {
        let (_d, _s, text, _) = run(
            "",
            "",
            "<p><img class=\"pic\" src=\"x.jpg\" alt=\"\" height=\"97%\"/></p>",
        );
        assert!(
            text.contains("class=\"pic epublift-height-97pct\""),
            "{text}"
        );
        assert!(!text.contains("height="), "{text}");
    }

    #[test]
    fn css_that_sets_the_property_leaves_the_attribute() {
        let (_d, s, text, css) = run(
            "img { width: 100% }",
            "",
            "<p><img src=\"x.jpg\" alt=\"\" width=\"40%\"/></p>",
        );
        assert_eq!((s.converted, s.left_css_sets), (0, 1));
        assert!(text.contains("width=\"40%\""), "{text}");
        assert!(css.is_empty());
    }

    #[test]
    fn css_under_media_counts_as_may_set() {
        let (_d, s, _, _) = run(
            "@media screen { img { width: 50% } }",
            "",
            "<p><img src=\"x.jpg\" alt=\"\" width=\"40%\"/></p>",
        );
        assert_eq!((s.converted, s.left_css_may_set), (0, 1));
    }

    #[test]
    fn inline_style_and_style_element_count() {
        let (_d, s, _, _) = run(
            "",
            "",
            "<p><img style=\"width: 3em\" src=\"x.jpg\" alt=\"\" width=\"40%\"/></p>",
        );
        assert_eq!(s.left_css_sets, 1);
        let (_d, s, _, _) = run(
            "",
            "<style type=\"text/css\">img { height: auto }</style>",
            "<p><img src=\"x.jpg\" alt=\"\" height=\"40%\"/></p>",
        );
        assert_eq!(s.left_css_sets, 1);
    }

    #[test]
    fn imported_sheet_counts() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("OEBPS");
        fs::create_dir_all(pkg.join("Styles")).unwrap();
        fs::write(pkg.join("Styles/s.css"), "@import url(\"more.css\");").unwrap();
        fs::write(pkg.join("Styles/more.css"), "img { width: auto }").unwrap();
        fs::write(
            pkg.join("a.xhtml"),
            format!("{HEAD}<link href=\"Styles/s.css\" rel=\"stylesheet\"/></head><body><img src=\"x\" alt=\"\" width=\"40%\"/></body></html>"),
        )
        .unwrap();
        let s = convert_book(dir.path(), &pkg, &|_| {}).unwrap();
        assert_eq!((s.converted, s.left_css_sets), (0, 1));
    }

    #[test]
    fn selector_reading_the_attribute_leaves_the_document() {
        let (_d, s, text, _) = run(
            "img[width] { border: 0 }",
            "",
            "<p><img src=\"x.jpg\" alt=\"\" width=\"40%\"/></p>",
        );
        assert_eq!((s.converted, s.left_selectors), (0, 1));
        assert!(text.contains("width=\"40%\""), "{text}");
        let (_d, s, _, _) = run(
            "img[class] { border: 0 }",
            "",
            "<p><img src=\"x.jpg\" alt=\"\" width=\"40%\"/></p>",
        );
        assert_eq!(s.left_selectors, 1);
    }

    #[test]
    fn values_html_reads_validly_are_rewritten_or_removed() {
        let (_d, s, text, css) = run(
            "",
            "",
            "<p><img src=\"a\" alt=\"\" width=\"60px\" height=\"45\"/><img src=\"b\" alt=\"\" width=\"auto\"/><img src=\"c\" alt=\"\" width=\"\"/></p>",
        );
        assert_eq!(s.converted, 3);
        assert!(
            text.contains("<img src=\"a\" alt=\"\" width=\"60\" height=\"45\"/>"),
            "{text}"
        );
        assert!(
            text.contains("<img src=\"b\" alt=\"\"/>")
                && text.contains("<img src=\"c\" alt=\"\"/>"),
            "{text}"
        );
        // Nothing needed CSS, so no stylesheet is written or linked.
        assert!(css.is_empty() && s.stylesheet.is_none());
        assert!(!text.contains(STYLESHEET_NAME), "{text}");
    }

    #[test]
    fn valid_integer_dimensions_are_untouched() {
        let (_d, s, text, _) = run(
            "",
            "",
            "<p><img src=\"x\" alt=\"\" width=\"300\" height=\"200\"/></p>",
        );
        assert_eq!(s, Summary::default());
        assert!(text.contains("width=\"300\" height=\"200\""), "{text}");
    }

    #[test]
    fn table_attributes() {
        let (_d, s, text, css) = run(
            "",
            "",
            "<table cellspacing=\"2\" cellpadding=\"3\" border=\"3\" width=\"80%\"><tbody><tr>\
             <td valign=\"Top\" width=\"50%\">a</td><th height=\"20\">b</th></tr></tbody></table>",
        );
        assert_eq!(s.converted, 7, "{text}");
        assert!(
            text.contains(
                "<table class=\"epublift-border-spacing-2px epublift-table-border-3px epublift-width-80pct\">"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "<td class=\"epublift-padding-3px epublift-cell-border epublift-vertical-align-top epublift-width-50pct\">"
            ),
            "{text}"
        );
        for rule in [
            ".epublift-border-spacing-2px { border-spacing: 2px }",
            ".epublift-cell-border { border-width: 1px; border-style: inset }",
            ".epublift-padding-3px { padding: 3px }",
            ".epublift-table-border-3px { border-width: 3px; border-style: outset }",
            ".epublift-vertical-align-top { vertical-align: top }",
            ".epublift-width-50pct { width: 50% }",
            ".epublift-height-20px { height: 20px }",
        ] {
            assert!(css.contains(rule), "{rule}\n{css}");
        }
        // The cells carry the padding and border classes, not the table.
        assert!(
            text.contains("epublift-padding-3px epublift-cell-border"),
            "{text}"
        );
        for a in [
            "cellspacing",
            "cellpadding",
            "border=",
            "valign",
            "width=",
            "height=",
        ] {
            assert!(!text.contains(a), "{a}: {text}");
        }
        crate::util::parse_xml_with_dtd(&text).unwrap();
    }

    #[test]
    fn table_border_values() {
        let (_d, s, text, css) = run("", "", "<table border=\"0\"><tr><td>a</td></tr></table>");
        assert_eq!(s.converted, 1);
        assert!(
            text.contains("<table><tr><td>a</td></tr></table>") && css.is_empty(),
            "{text}"
        );
        let (_d, _s, text, _) = run("", "", "<table border=\"yes\"><tr><td>a</td></tr></table>");
        assert!(text.contains("<table border=\"1\">"), "{text}");
        let (_d, s, _, _) = run("", "", "<table border=\"1\"><tr><td>a</td></tr></table>");
        assert_eq!(s, Summary::default());
        // A border on the cells set by the book's CSS keeps the attribute.
        let (_d, s, _, _) = run(
            "td { border: 1px solid }",
            "",
            "<table border=\"2\"><tr><td>a</td></tr></table>",
        );
        assert_eq!(s.left_css_sets, 1);
    }

    #[test]
    fn nested_table_cells_belong_to_their_own_table() {
        let (_d, _s, text, _) = run(
            "",
            "",
            "<table cellpadding=\"4\"><tr><td><table><tr><td>in</td></tr></table></td></tr></table>",
        );
        assert_eq!(text.matches("epublift-padding-4px").count(), 1, "{text}");
    }

    #[test]
    fn br_clear() {
        let (_d, s, text, css) = run(
            "",
            "",
            "<p>a<br clear=\"ALL\"/>b<br clear=\"left\"/>c<br clear=\"none\"/>d</p>",
        );
        assert_eq!(s.converted, 3);
        assert!(
            text.contains(
                "a<br class=\"epublift-clear-both\"/>b<br class=\"epublift-clear-left\"/>c<br/>d"
            ),
            "{text}"
        );
        assert!(
            css.contains(".epublift-clear-both { clear: both }")
                && css.contains(".epublift-clear-left { clear: left }"),
            "{css}"
        );
        assert!(declarations_valid("clear: right"));
    }

    #[test]
    fn obsolete_elements() {
        let (_d, s, text, css) = run(
            "",
            "",
            "<p><big>B</big> <strike class=\"x\">S</strike> <font face=\"Times New Roman, serif\" size=\"+1\">F</font> \
             <font>plain</font></p><center><img src=\"c.jpg\" alt=\"\"/></center>",
        );
        assert_eq!((s.elements, s.converted), (5, 0), "{text}");
        assert!(
            text.contains("<span class=\"epublift-big\">B</span>"),
            "{text}"
        );
        assert!(text.contains("<s class=\"x\">S</s>"), "{text}");
        assert!(
            text.contains("<span class=\"epublift-font-family-times-new-roman-serif epublift-font-size-large\">F</span>"),
            "{text}"
        );
        assert!(text.contains("<span>plain</span>"), "{text}");
        assert!(
            text.contains("<div class=\"epublift-center\"><img src=\"c.jpg\" alt=\"\"/></div>"),
            "{text}"
        );
        for rule in [
            ".epublift-big { font-size: larger }",
            ".epublift-center { text-align: center }",
            ".epublift-font-family-times-new-roman-serif { font-family: Times New Roman, serif }",
            ".epublift-font-size-large { font-size: large }",
        ] {
            assert!(css.contains(rule), "{rule}\n{css}");
        }
        crate::util::parse_xml_with_dtd(&text).unwrap();
    }

    #[test]
    fn obsolete_elements_declined() {
        // A type selector for the old name, or for the new one, keeps the document.
        let (_d, s, text, _) = run(
            "font { color: red }",
            "",
            "<p><font face=\"Arial\">F</font></p>",
        );
        assert_eq!((s.elements, s.left_selectors), (0, 1));
        assert!(text.contains("<font face=\"Arial\">"), "{text}");
        let (_d, s, _, _) = run(
            "div { margin: 0 }",
            "",
            "<center><img src=\"c\" alt=\"\"/></center>",
        );
        assert_eq!(s.left_selectors, 1);
        // CSS that already decides the property.
        let (_d, s, _, _) = run("p big { font-size: 2em }", "", "<p><big>B</big></p>");
        assert_eq!(s.left_css_sets, 1);
        // A center around blocks, an unknown attribute, size 7: not planned.
        let (_d, s, _, _) = run(
            "",
            "",
            "<center><p>x</p></center><p><font color=\"red\">c</font><font size=\"7\">7</font></p>",
        );
        assert_eq!(s, Summary::default());
        // A center whose image the book's CSS makes a block.
        let (_d, s, _, _) = run(
            "img { display: block }",
            "",
            "<center><img src=\"c\" alt=\"\"/></center>",
        );
        assert_eq!((s.elements, s.left_css_sets), (0, 1));
    }

    #[test]
    fn font_face_is_reserialized() {
        for (face, want) in [
            ("Times New Roman, serif", "Times New Roman, serif"),
            ("Arial", "Arial"),
            ("\"Book Antiqua\", serif", "\"Book Antiqua\", serif"),
            (
                "'Palatino Linotype',Georgia",
                "\"Palatino Linotype\", Georgia",
            ),
            ("  Verdana ,  sans-serif ", "Verdana, sans-serif"),
            ("'It\\'s'", "\"It's\""),
        ] {
            assert_eq!(font_family_value(face).as_deref(), Some(want), "{face}");
        }
    }

    #[test]
    fn font_face_that_is_not_a_family_list_is_declined() {
        for face in [
            // Unterminated strings: valid tokens, but parse errors.
            "\"Book Antiqua",
            "'Times",
            "'a\\'",
            // A comment would swallow what follows it in the stylesheet.
            "a /* b",
            "a /* b */ c",
            // Breaking out of the declaration or the rule.
            "x; color: red",
            "a } body { display: none",
            "a { b",
            // Not a family name.
            "inherit",
            "Default Font",
            "3D Font",
            "Arial,",
            ", Arial",
            "\"a\" b",
            "url(x)",
        ] {
            assert_eq!(font_family_value(face), None, "{face}");
        }
    }

    #[test]
    fn hostile_font_face_leaves_the_element() {
        // The `font` stays as written: the face is not CSS we can write safely.
        for face in [
            "&quot;Book Antiqua",
            "a } body { display: none",
            "x; color: red",
            "a /* b",
        ] {
            let (_d, s, text, css) = run(
                "",
                "",
                &format!("<p><font face=\"{face}\" size=\"+1\">F</font></p>"),
            );
            assert_eq!(s.elements, 0, "{face}");
            assert!(text.contains("<font face="), "{face}: {text}");
            assert!(!css.contains("font-family"), "{face}: {css}");
        }
        // A quoted family goes through, quoted as styloria writes it.
        let (_d, s, _, css) = run(
            "",
            "",
            "<p><font face=\"'Book Antiqua', serif\">F</font></p>",
        );
        assert_eq!(s.elements, 1);
        assert!(
            css.contains("{ font-family: \"Book Antiqua\", serif }"),
            "{css}"
        );
        assert!(rule_clean(&css), "{css}");
    }

    #[test]
    fn legacy_font_size() {
        assert_eq!(parse_legacy_font_size("2"), Some(2));
        assert_eq!(parse_legacy_font_size(" +1"), Some(4));
        assert_eq!(parse_legacy_font_size("-1"), Some(2));
        assert_eq!(parse_legacy_font_size("-9"), Some(1));
        assert_eq!(parse_legacy_font_size("+9"), Some(7));
        assert_eq!(parse_legacy_font_size("0"), Some(1));
        assert_eq!(parse_legacy_font_size("big"), None);
        assert_eq!(slug("Garamond Premr Pro, "), "garamond-premr-pro");
    }

    #[test]
    fn microsyntaxes() {
        assert_eq!(
            parse_dimension("40%", false),
            Some(Dim::Percent("40".into()))
        );
        assert_eq!(
            parse_dimension(" 12.50%", false),
            Some(Dim::Percent("12.5".into()))
        );
        assert_eq!(
            parse_dimension("60px", false),
            Some(Dim::Length("60".into()))
        );
        assert_eq!(
            parse_dimension("007.0", false),
            Some(Dim::Length("7".into()))
        );
        assert_eq!(
            parse_dimension("40 %", false),
            Some(Dim::Length("40".into()))
        );
        assert_eq!(parse_dimension("auto", false), None);
        assert_eq!(parse_dimension("", false), None);
        assert_eq!(parse_dimension("0", true), None);
        assert_eq!(parse_dimension("0", false), Some(Dim::Length("0".into())));
        assert_eq!(parse_non_negative_integer("+3px"), Some(3));
        assert_eq!(parse_non_negative_integer("-1"), None);
        assert!(is_valid_integer("300") && !is_valid_integer(" 300") && !is_valid_integer(""));
    }

    #[test]
    fn every_declaration_written_is_valid() {
        for d in [
            dimension_decl("width", &Dim::Percent("12.5".into())),
            dimension_decl("height", &Dim::Length("60.5".into())),
            px_decl("border-spacing", 0),
            px_decl("padding", 7),
        ] {
            assert!(declarations_valid(&d.css), "{}", d.css);
        }
        assert!(declarations_valid(
            "border-width: 3px; border-style: outset"
        ));
        assert!(declarations_valid("vertical-align: baseline"));
        assert!(!declarations_valid("width: banana"));
    }

    #[test]
    fn stylesheet_name_is_free() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(STYLESHEET_NAME), "").unwrap();
        assert_eq!(
            free_name(dir.path()),
            dir.path().join("epublift-presentational-2.css")
        );
    }
}
