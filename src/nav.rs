//! Generation of an EPUB 3 XHTML Navigation Document (`nav.xhtml`) from a
//! legacy `toc.ncx`, including a `landmarks` block built from the OPF `<guide>`.

use crate::opf::GuideRef;
use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::Path;

/// Remove a `<!DOCTYPE ...>` declaration so the strict, DTD-rejecting XML parser
/// (roxmltree) will accept the document. NCX files commonly carry a DOCTYPE.
fn strip_doctype(xml: &str) -> String {
    let re = Regex::new(r"(?is)<!DOCTYPE[^>]*>").unwrap();
    re.replace(xml, "").into_owned()
}

/// A node in the navigation tree parsed from `toc.ncx`.
struct NavPoint {
    title: String,
    href: String,
    children: Vec<NavPoint>,
}

/// Parse `toc.ncx` and write a valid EPUB 3 Navigation Document to `out_path`.
pub fn generate_nav_xhtml(ncx_path: &Path, out_path: &Path, guide_refs: &[GuideRef]) -> Result<()> {
    let raw = fs::read_to_string(ncx_path).context("Failed to read toc.ncx")?;
    let xml = strip_doctype(&raw);
    let doc = roxmltree::Document::parse(&xml).context("Failed to parse toc.ncx")?;

    // Document title.
    let title_text = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "docTitle")
        .and_then(|d| {
            d.children()
                .find(|n| n.is_element() && n.tag_name().name() == "text")
        })
        .and_then(|t| t.text())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Table of Contents".to_string());

    // Navigation tree.
    let toc_items = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "navMap")
        .map(|nav_map| parse_nav_points(nav_map))
        .unwrap_or_default();

    let mut out: Vec<String> = Vec::new();
    out.push(r#"<?xml version="1.0" encoding="utf-8"?>"#.to_string());
    out.push("<!DOCTYPE html>".to_string());
    out.push(r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="en" xml:lang="en">"#.to_string());
    out.push("<head>".to_string());
    out.push(format!("  <title>{}</title>", title_text));
    out.push(r#"  <meta charset="utf-8" />"#.to_string());
    out.push("  <style>".to_string());
    out.push("    body { font-family: sans-serif; margin: 2em; }".to_string());
    out.push("    nav ol { list-style-type: none; padding-left: 1.5em; }".to_string());
    out.push("    nav ol li { margin: 0.5em 0; }".to_string());
    out.push("    a { text-decoration: none; color: #1a73e8; }".to_string());
    out.push("    a:hover { text-decoration: underline; }".to_string());
    out.push("    h1 { color: #333333; }".to_string());
    out.push("  </style>".to_string());
    out.push("</head>".to_string());
    out.push("<body>".to_string());
    out.push(r#"  <nav epub:type="toc" id="toc">"#.to_string());
    out.push(format!("    <h1>{}</h1>", title_text));

    if toc_items.is_empty() {
        out.push("    <p>No table of contents available.</p>".to_string());
    } else {
        render_ol(&toc_items, 2, &mut out);
    }

    out.push("  </nav>".to_string());

    // Landmarks derived from the legacy <guide>.
    if !guide_refs.is_empty() {
        out.push("\n  <nav epub:type=\"landmarks\" id=\"landmarks\" hidden=\"\">".to_string());
        out.push("    <h2>Guide Landmarks</h2>".to_string());
        out.push("    <ol>".to_string());
        for r in guide_refs {
            let href = r.href.replace('\\', "/");
            let mapped = map_landmark_type(&r.ref_type);
            out.push(format!(
                "      <li><a epub:type=\"{}\" href=\"{}\">{}</a></li>",
                mapped, href, r.title
            ));
        }
        out.push("    </ol>".to_string());
        out.push("  </nav>".to_string());
    }

    out.push("</body>".to_string());
    out.push("</html>".to_string());

    fs::write(out_path, out.join("\n"))?;
    Ok(())
}

/// Recursively collect `<navPoint>` children of `parent`.
fn parse_nav_points(parent: roxmltree::Node) -> Vec<NavPoint> {
    let mut items = Vec::new();
    for p in parent
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "navPoint")
    {
        let title = p
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "navLabel")
            .and_then(|label| {
                label
                    .children()
                    .find(|n| n.is_element() && n.tag_name().name() == "text")
            })
            .and_then(|t| t.text())
            .unwrap_or("Untitled")
            .to_string();

        let href = p
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "content")
            .and_then(|c| c.attribute("src"))
            .unwrap_or("")
            .to_string();

        let children = parse_nav_points(p);
        items.push(NavPoint {
            title,
            href,
            children,
        });
    }
    items
}

/// Render a nested ordered list mirroring the navigation tree.
fn render_ol(items: &[NavPoint], level: usize, out: &mut Vec<String>) {
    let indent = "  ".repeat(level);
    out.push(format!("{}<ol>", indent));
    for item in items {
        let href = item.href.replace('\\', "/");
        out.push(format!("{}  <li>", indent));
        if href.is_empty() {
            out.push(format!("{}    <span>{}</span>", indent, item.title));
        } else {
            out.push(format!(
                "{}    <a href=\"{}\">{}</a>",
                indent, href, item.title
            ));
        }
        if !item.children.is_empty() {
            render_ol(&item.children, level + 2, out);
        }
        out.push(format!("{}  </li>", indent));
    }
    out.push(format!("{}</ol>", indent));
}

/// Map EPUB 2 guide types to EPUB 3 landmark semantics.
fn map_landmark_type(ref_type: &str) -> String {
    match ref_type {
        "text" => "bodymatter",
        "title-page" => "titlepage",
        "acknowledgements" => "acknowledgments",
        "cover" => "cover",
        "toc" => "toc",
        other => other,
    }
    .to_string()
}
