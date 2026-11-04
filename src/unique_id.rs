//! The package's unique identifier: which `dc:identifier` it is, and making a
//! new one.
//!
//! A new identifier is a UUID. With an ISBN it is name-based (v5), so the same
//! ISBN always gives the same UUID and reprocessing a book never makes a
//! duplicate in a reader's library. Without one it is random (v4).
//!
//! Font obfuscation derives its key from the unique identifier (EPUB 3.3
//! §4.4.3), so a book that uses it keeps the identifier it has.

use anyhow::{Context, Result, bail};
use percent_encoding::percent_decode_str;
use uuid::Uuid;

use crate::meta::Metadata;
use crate::util::{parse_xml, parse_xml_with_dtd};

const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
const NCX_MEDIA_TYPE: &str = "application/x-dtbncx+xml";
/// The id a new or id-less identifier gets (a number follows if it is taken).
const NEW_ID: &str = "epublift-uid";
/// The namespace of the name-based UUIDs: v5 of this URL in the URL namespace.
const NAMESPACE_URL: &str = "https://github.com/ePubLift/epublift";
/// The IDPF and Adobe font-obfuscation algorithms; both key on the identifier.
const OBFUSCATION: [&str; 2] = [
    "http://www.idpf.org/2008/embedding",
    "http://ns.adobe.com/pdf/enc#RC",
];

/// What to make the package's unique identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UniqueIdentifier {
    /// One of the book's existing `dc:identifier` values.
    Existing(String),
    /// A new UUID: v5 from the ISBN, else v4.
    New,
}

/// `value` as ISBN-13 digits (ISBN-10 converted, hyphens and spaces dropped), or
/// `None` when it is not an ISBN with a correct check digit.
pub fn isbn13(value: &str) -> Option<String> {
    let v = value.trim();
    let v = match v.get(..9) {
        Some(p) if p.eq_ignore_ascii_case("urn:isbn:") => &v[9..],
        _ => v,
    };
    let c: Vec<char> = v
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let digit = |c: &char| c.to_digit(10);
    let check13 = |d: &[u32]| {
        let sum: u32 = d
            .iter()
            .enumerate()
            .map(|(i, n)| if i % 2 == 0 { *n } else { n * 3 })
            .sum();
        (10 - sum % 10) % 10
    };
    match c.len() {
        13 => {
            let d: Vec<u32> = c.iter().filter_map(digit).collect();
            (d.len() == 13 && check13(&d[..12]) == d[12]).then(|| c.iter().collect())
        }
        10 => {
            let d: Vec<u32> = c[..9].iter().filter_map(digit).collect();
            let last = if c[9] == 'X' {
                Some(10)
            } else {
                c[9].to_digit(10)
            }?;
            let sum: u32 = d.iter().enumerate().map(|(i, n)| (10 - i as u32) * n).sum();
            if d.len() != 9 || !(sum + last).is_multiple_of(11) {
                return None;
            }
            let mut d13 = vec![9, 7, 8];
            d13.extend(&d);
            let check = check13(&d13);
            d13.push(check);
            Some(d13.iter().map(u32::to_string).collect())
        }
        _ => None,
    }
}

/// The value of a new identifier: `urn:uuid:` and a v5 UUID from `isbn` (ISBN-13
/// digits), or a v4 one without it.
pub fn new_value(isbn: Option<&str>) -> String {
    let uuid = match isbn {
        Some(isbn) => {
            let namespace = Uuid::new_v5(&Uuid::NAMESPACE_URL, NAMESPACE_URL.as_bytes());
            Uuid::new_v5(&namespace, isbn.as_bytes())
        }
        None => Uuid::new_v4(),
    };
    format!("urn:uuid:{uuid}")
}

/// The book's ISBN as ISBN-13 digits. Several different ones are an error: the
/// book holds no evidence which is the edition at hand, and we never guess.
fn book_isbn(md: &Metadata) -> Result<Option<String>> {
    let mut found: Vec<String> = md
        .identifiers
        .iter()
        .filter(|i| {
            i.value.starts_with("urn:isbn:")
                || i.scheme
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
        })
        .filter_map(|i| isbn13(&i.value))
        .collect();
    found.sort();
    found.dedup();
    if found.len() > 1 {
        bail!(
            "the book has several ISBNs ({}), so a new identifier can't be chosen for you; \
             name one with --isbn",
            found.join(", ")
        );
    }
    Ok(found.pop())
}

/// The value `choice` stands for in this book. `isbn` is one given with the same
/// edit, which wins over the book's own.
pub(crate) fn resolve(
    choice: &UniqueIdentifier,
    md: &Metadata,
    isbn: Option<&str>,
) -> Result<String> {
    match choice {
        UniqueIdentifier::Existing(value) => {
            let value = value.trim();
            if md.identifiers.iter().any(|i| i.value.trim() == value) {
                Ok(value.to_string())
            } else {
                bail!(
                    "'{value}' is not one of this book's identifiers: {}",
                    identifier_list(md)
                )
            }
        }
        UniqueIdentifier::New => {
            let isbn = match isbn {
                Some(s) => Some(isbn13(s).with_context(|| format!("'{s}' is not a valid ISBN"))?),
                None => book_isbn(md)?,
            };
            Ok(new_value(isbn.as_deref()))
        }
    }
}

/// The book's identifiers, for a message.
pub fn identifier_list(md: &Metadata) -> String {
    if md.identifiers.is_empty() {
        return "it has none".to_string();
    }
    md.identifiers
        .iter()
        .map(|i| i.value.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether this `META-INF/encryption.xml` obfuscates anything with a key derived
/// from the unique identifier.
pub(crate) fn obfuscates(encryption_xml: &str) -> bool {
    OBFUSCATION.iter().any(|a| encryption_xml.contains(a))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Replace byte ranges; the ranges do not overlap.
fn apply_edits(text: &str, mut edits: Vec<(std::ops::Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
    let mut out = text.to_string();
    for (range, new) in edits {
        out.replace_range(range, &new);
    }
    out
}

/// The byte just after an element's tag name, where an attribute can go.
fn after_tag_name(text: &str, node: &roxmltree::Node) -> usize {
    let start = node.range().start + 1;
    start
        + text[start..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(0)
}

/// `node`'s range, with the whitespace before it, so removing it leaves no gap.
fn with_leading_space(text: &str, node: &roxmltree::Node) -> std::ops::Range<usize> {
    let r = node.range();
    let start = text[..r.start].trim_end().len();
    start..r.end
}

/// Make the identifier holding `value` the package's unique identifier, adding
/// the element when the book doesn't have it. Empty `dc:identifier`s go, with
/// what refines them: they are invalid and hold nothing.
pub(crate) fn apply_to_opf(opf: &str, value: &str) -> Result<String> {
    let doc = parse_xml(opf).context("Failed to parse the OPF")?;
    let pkg = doc.root_element();
    let metadata = pkg
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")
        .context("OPF has no <metadata> element to edit")?;
    let identifiers: Vec<_> = metadata
        .children()
        .filter(|n| {
            n.is_element()
                && n.tag_name().name() == "identifier"
                && n.tag_name().namespace() == Some(DC_NS)
        })
        .collect();
    let text_of = |n: &roxmltree::Node| n.text().map(str::trim).unwrap_or("").to_string();

    let mut edits = Vec::new();
    let free_id = || {
        let taken = |id: &str| doc.descendants().any(|n| n.attribute("id") == Some(id));
        std::iter::once(NEW_ID.to_string())
            .chain((2..).map(|n| format!("{NEW_ID}-{n}")))
            .find(|id| !taken(id))
            .expect("an unused id exists")
    };

    let target = identifiers.iter().find(|n| text_of(n) == value);
    let id = match target {
        Some(n) => match n.attribute("id") {
            Some(id) => id.to_string(),
            None => {
                let id = free_id();
                let at = after_tag_name(opf, n);
                edits.push((at..at, format!(" id=\"{id}\"")));
                id
            }
        },
        None => {
            let id = free_id();
            let end = metadata.range().end;
            let at = opf[..end]
                .rfind("</")
                .filter(|at| *at > metadata.range().start)
                .context("OPF has an empty <metadata> element")?;
            let (name, decl) = match metadata.lookup_prefix(DC_NS).filter(|p| !p.is_empty()) {
                Some(p) => (format!("{p}:identifier"), String::new()),
                None => (
                    "dc:identifier".to_string(),
                    format!(" xmlns:dc=\"{DC_NS}\""),
                ),
            };
            let indent = identifiers.first().map_or("\n    ".to_string(), |n| {
                let start = n.range().start;
                opf[opf[..start].trim_end().len()..start].to_string()
            });
            edits.push((
                at..at,
                format!(
                    "{indent}<{name} id=\"{id}\"{decl}>{}</{name}>\n  ",
                    escape(value)
                ),
            ));
            id
        }
    };

    match pkg
        .attributes()
        .find(|a| a.name() == "unique-identifier" && a.namespace().is_none())
    {
        Some(a) if a.value() == id => {}
        Some(a) => edits.push((a.range_value(), escape(&id))),
        None => {
            let at = after_tag_name(opf, &pkg);
            edits.push((at..at, format!(" unique-identifier=\"{}\"", escape(&id))));
        }
    }

    for n in identifiers.iter().filter(|n| text_of(n).is_empty()) {
        edits.push((with_leading_space(opf, n), String::new()));
        if let Some(own) = n.attribute("id") {
            let refines = format!("#{own}");
            for m in metadata
                .children()
                .filter(|m| m.is_element() && m.attribute("refines") == Some(refines.as_str()))
            {
                edits.push((with_leading_space(opf, &m), String::new()));
            }
        }
    }
    Ok(apply_edits(opf, edits))
}

/// The archive name of the book's NCX, from the manifest.
pub(crate) fn ncx_entry_name(opf: &str, opf_name: &str) -> Option<String> {
    let doc = parse_xml(opf).ok()?;
    let href = doc
        .descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "item"
                && n.attribute("media-type") == Some(NCX_MEDIA_TYPE)
        })?
        .attribute("href")?;
    let href = percent_decode_str(href).decode_utf8().ok()?;
    let mut parts: Vec<&str> = opf_name.split('/').collect();
    parts.pop();
    for part in href.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// The NCX with its `dtb:uid` set to `value`, or `None` when it already is, or
/// the NCX can't be read or has no `<head>` to put it in.
pub(crate) fn sync_ncx(ncx: &str, value: &str) -> Option<String> {
    // Many NCX files carry the DTBook DOCTYPE.
    let doc = parse_xml_with_dtd(ncx).ok()?;
    let head = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "head")?;
    let uid = head.children().find(|n| {
        n.is_element() && n.tag_name().name() == "meta" && n.attribute("name") == Some("dtb:uid")
    });
    let edit = match uid {
        Some(meta) => match meta.attributes().find(|a| a.name() == "content") {
            Some(a) if a.value() == value => return None,
            Some(a) => (a.range_value(), escape(value)),
            None => {
                let at = after_tag_name(ncx, &meta);
                (at..at, format!(" content=\"{}\"", escape(value)))
            }
        },
        None => {
            let start = head.range().start;
            let open_end = start + ncx[start..].find('>')?;
            if ncx[..open_end].ends_with('/') {
                return None;
            }
            let at = open_end + 1;
            let prefix = ncx[start + 1..after_tag_name(ncx, &head)]
                .rsplit_once(':')
                .map_or(String::new(), |(p, _)| format!("{p}:"));
            (
                at..at,
                format!(
                    "<{prefix}meta name=\"dtb:uid\" content=\"{}\"/>",
                    escape(value)
                ),
            )
        }
    };
    Some(apply_edits(ncx, vec![edit]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::parse_metadata;

    fn opf(ids: &str, extra: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:opf="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>T</dc:title>
    {ids}{extra}
  </metadata>
  <manifest/>
  <spine/>
</package>"#
        )
    }

    fn unique(xml: &str) -> Option<String> {
        parse_metadata(xml)
            .unwrap()
            .identifiers
            .into_iter()
            .find(|i| i.unique)
            .map(|i| i.value)
    }

    #[test]
    fn isbn_is_normalized_and_checked() {
        assert_eq!(
            isbn13("urn:isbn:978-0-14-032872-1").as_deref(),
            Some("9780140328721")
        );
        // The same book as ISBN-10 gives the same ISBN-13.
        assert_eq!(isbn13("0-14-032872-X"), None);
        assert_eq!(isbn13("0140328726").as_deref(), Some("9780140328721"));
        assert_eq!(isbn13("080442957X").as_deref(), Some("9780804429573"));
        assert_eq!(isbn13("9780140328722"), None);
        assert_eq!(isbn13("urn:uuid:12345"), None);
    }

    #[test]
    fn the_same_isbn_gives_the_same_uuid_and_none_gives_a_random_one() {
        let v = new_value(Some("9780140328721"));
        assert_eq!(v, new_value(Some("9780140328721")));
        // Fixed for good: a changed value would make duplicates in libraries.
        assert_eq!(v, "urn:uuid:71f90c32-46da-593e-8441-cba14452e693");
        let id = Uuid::parse_str(v.trim_start_matches("urn:uuid:")).unwrap();
        assert_eq!(id.get_version(), Some(uuid::Version::Sha1));
        assert_ne!(new_value(None), new_value(None));
    }

    #[test]
    fn an_existing_identifier_becomes_the_unique_one_and_the_empty_one_goes() {
        let xml = opf(
            r#"<dc:identifier id="bookid"/>
    <dc:identifier id="c" opf:scheme="calibre">abc</dc:identifier>
    <dc:identifier opf:scheme="ISBN">9780140328721</dc:identifier>"#,
            "",
        );
        let out = apply_to_opf(&xml, "9780140328721").unwrap();
        assert_eq!(unique(&out).as_deref(), Some("9780140328721"));
        assert!(!out.contains(r#"id="bookid""#));
        assert!(out.contains(">abc<"));
        assert!(out.contains(r#"unique-identifier="epublift-uid""#));
    }

    #[test]
    fn a_new_value_is_added_and_a_taken_id_is_not_reused() {
        let xml = opf(
            r#"<dc:identifier id="bookid">old</dc:identifier>
    <dc:identifier id="epublift-uid">other</dc:identifier>"#,
            "",
        );
        let out = apply_to_opf(&xml, "urn:uuid:a&b").unwrap();
        assert_eq!(unique(&out).as_deref(), Some("urn:uuid:a&b"));
        assert!(out.contains(r#"id="epublift-uid-2""#));
        assert!(out.contains(">old<"), "the old identifier stays");
    }

    #[test]
    fn what_refines_an_empty_identifier_goes_with_it() {
        let xml = opf(
            r#"<dc:identifier id="bookid"></dc:identifier>
    <dc:identifier id="k">keep</dc:identifier>"#,
            r##"
    <meta refines="#bookid" property="identifier-type">x</meta>
    <meta refines="#k" property="identifier-type">y</meta>"##,
        );
        let out = apply_to_opf(&xml, "keep").unwrap();
        assert_eq!(unique(&out).as_deref(), Some("keep"));
        assert!(!out.contains("#bookid"));
        assert!(out.contains("#k"));
        parse_metadata(&out).unwrap();
    }

    #[test]
    fn asking_for_the_current_one_changes_nothing() {
        let xml = opf(r#"<dc:identifier id="bookid">same</dc:identifier>"#, "");
        assert_eq!(apply_to_opf(&xml, "same").unwrap(), xml);
    }

    #[test]
    fn resolve_refuses_an_unknown_value_and_two_isbns() {
        let xml = opf(
            r#"<dc:identifier id="bookid" opf:scheme="ISBN">9780140328721</dc:identifier>
    <dc:identifier opf:scheme="ISBN">9780804429573</dc:identifier>"#,
            "",
        );
        let md = parse_metadata(&xml).unwrap();
        let e = resolve(&UniqueIdentifier::Existing("nope".into()), &md, None).unwrap_err();
        assert!(e.to_string().contains("9780140328721"));
        let e = resolve(&UniqueIdentifier::New, &md, None).unwrap_err();
        assert!(e.to_string().contains("--isbn"));
        // An ISBN given with the edit settles it.
        let v = resolve(&UniqueIdentifier::New, &md, Some("978-0-14-032872-1")).unwrap();
        assert_eq!(v, new_value(Some("9780140328721")));
        assert!(resolve(&UniqueIdentifier::New, &md, Some("123")).is_err());
    }

    #[test]
    fn ncx_uid_is_set_filled_and_added() {
        let set = r#"<ncx><head><meta name="dtb:uid" content="old"/></head></ncx>"#;
        assert_eq!(
            sync_ncx(set, "new").unwrap(),
            r#"<ncx><head><meta name="dtb:uid" content="new"/></head></ncx>"#
        );
        assert_eq!(
            sync_ncx(
                r#"<ncx><head><meta name="dtb:uid" content="a"/></head></ncx>"#,
                "a"
            ),
            None
        );
        assert_eq!(
            sync_ncx("<ncx><head></head></ncx>", "a&b").unwrap(),
            r#"<ncx><head><meta name="dtb:uid" content="a&amp;b"/></head></ncx>"#
        );
        assert_eq!(sync_ncx("<ncx><head/></ncx>", "a"), None);
    }

    #[test]
    fn ncx_with_a_doctype_is_read() {
        let ncx = r#"<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd"><ncx><head><meta content="old" name="dtb:uid"/></head></ncx>"#;
        assert!(sync_ncx(ncx, "new").unwrap().contains(r#"content="new""#));
    }

    #[test]
    fn ncx_name_is_relative_to_the_opf() {
        let o = r#"<package><manifest><item href="../t%20oc.ncx" media-type="application/x-dtbncx+xml"/></manifest></package>"#;
        assert_eq!(
            ncx_entry_name(o, "OEBPS/content/book.opf").as_deref(),
            Some("OEBPS/t oc.ncx")
        );
    }

    #[test]
    fn both_obfuscation_algorithms_count() {
        assert!(obfuscates(
            r#"<EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/>"#
        ));
        assert!(obfuscates(
            r#"<EncryptionMethod Algorithm="http://ns.adobe.com/pdf/enc#RC"/>"#
        ));
        assert!(!obfuscates(
            r#"<EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/>"#
        ));
    }
}
