// epublift - Optimize EPUB files: convert images to WebP and upgrade to EPUB 3.3.
// Copyright (C) 2024  Baris Kayadelen
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Mistral OCR provider for Smart Import.
//!
//! Calls `POST https://api.mistral.ai/v1/ocr` with the PDF inlined as a base64
//! data URL and `include_image_base64: true`. The response is a list of pages,
//! each with `markdown` and `images` (base64); we stitch the pages together and
//! return the images so the offline Markdown importer can embed them.
//!
//! The live HTTP call can't be exercised without an API key, so the response
//! reconstruction ([`parse_ocr_response`]) is covered by a fixture test.

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde_json::Value;

/// OCR'd document: stitched Markdown plus its decoded images (`filename`, bytes)
/// where the filenames match the rewritten `![…](filename)` references.
pub(crate) struct OcrResult {
    pub markdown: String,
    pub images: Vec<(String, Vec<u8>)>,
}

const ENDPOINT: &str = "https://api.mistral.ai/v1/ocr";
/// OCR responses carry base64 page images, so allow a generous response size.
const MAX_RESPONSE: usize = 128 * 1024 * 1024;

/// Run Mistral OCR on `pdf`, returning the stitched Markdown + images.
pub(crate) fn ocr(api_key: &str, pdf: &[u8]) -> Result<OcrResult> {
    let http = crate::http::RustlsHttp::new()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(pdf);
    let request = serde_json::json!({
        "model": "mistral-ocr-latest",
        "document": {
            "type": "document_url",
            "document_url": format!("data:application/pdf;base64,{b64}"),
        },
        "include_image_base64": true,
    });
    let body = serde_json::to_vec(&request).context("failed to build OCR request")?;

    let (status, resp) = http.post_json(ENDPOINT, api_key, &body, MAX_RESPONSE)?;
    if status != 200 {
        bail!(
            "Mistral OCR API returned HTTP {status}: {}",
            error_message(&resp)
        );
    }

    let value: Value =
        serde_json::from_slice(&resp).context("Mistral OCR API returned invalid JSON")?;
    parse_ocr_response(&value)
}

/// Pull a human-readable error out of an error response body (best effort).
fn error_message(resp: &[u8]) -> String {
    let text = String::from_utf8_lossy(resp);
    if let Ok(v) = serde_json::from_str::<Value>(&text) {
        // Mistral errors look like {"message": "..."} or {"detail": "..."}.
        for key in ["message", "detail", "error"] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                return truncate(s, 300);
            }
        }
    }
    truncate(text.trim(), 300)
}

/// Reconstruct stitched Markdown + decoded images from an OCR response.
///
/// Image ids are page-scoped (`img-0.jpeg` can repeat per page), so we namespace
/// each as `p{page}_{id}` and rewrite that page's links to match before joining.
fn parse_ocr_response(value: &Value) -> Result<OcrResult> {
    let pages = value
        .get("pages")
        .and_then(|p| p.as_array())
        .context("OCR response had no 'pages' array")?;

    let mut markdown = String::new();
    let mut images: Vec<(String, Vec<u8>)> = Vec::new();

    for (pi, page) in pages.iter().enumerate() {
        let mut page_md = page
            .get("markdown")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(imgs) = page.get("images").and_then(|i| i.as_array()) {
            for img in imgs {
                let Some(id) = img
                    .get("id")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let Some(data) = img.get("image_base64").and_then(|x| x.as_str()) else {
                    continue;
                };
                let bytes = match decode_data_uri(data) {
                    Ok(b) => b,
                    Err(_) => continue, // skip an undecodable image rather than fail the book
                };
                let name = format!("p{pi}_{}", sanitize_name(id));
                // Re-point this page's links (`](id)` / `](./id)`) at the file.
                page_md = page_md
                    .replace(&format!("]({id})"), &format!("](./{name})"))
                    .replace(&format!("](./{id})"), &format!("](./{name})"));
                images.push((name, bytes));
            }
        }

        if !markdown.is_empty() {
            markdown.push_str("\n\n");
        }
        markdown.push_str(&page_md);
    }

    Ok(OcrResult { markdown, images })
}

/// Decode an image that may be a bare base64 string or a `data:…;base64,…` URI.
fn decode_data_uri(s: &str) -> Result<Vec<u8>> {
    let b64 = match s.strip_prefix("data:") {
        Some(rest) => rest
            .split_once(',')
            .map(|(_, data)| data)
            .context("malformed data URI")?,
        None => s,
    };
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .context("invalid base64 image data")
}

/// Flatten an image id into a safe, path-component-free filename.
fn sanitize_name(id: &str) -> String {
    let base = id.rsplit(['/', '\\']).next().unwrap_or(id);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "image".to_string()
    } else {
        cleaned
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1x1 transparent PNG, base64-encoded (standard alphabet).
    const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAACklEQVR4nGNgAAAAAgABc3UBGAAAAABJRU5ErkJggg==";

    #[test]
    fn decodes_plain_and_data_uri_base64() {
        let plain = decode_data_uri(PNG_B64).unwrap();
        let uri = decode_data_uri(&format!("data:image/png;base64,{PNG_B64}")).unwrap();
        assert_eq!(plain, uri);
        assert_eq!(&plain[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn sanitizes_ids() {
        assert_eq!(sanitize_name("img-0.jpeg"), "img-0.jpeg");
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name("a b/c?.png"), "c_.png");
    }

    #[test]
    fn parses_pages_and_rewrites_image_links() {
        let resp = serde_json::json!({
            "pages": [
                {
                    "index": 0,
                    "markdown": "# Title\n\n![img-0.jpeg](img-0.jpeg)\n\nHello.",
                    "images": [
                        { "id": "img-0.jpeg", "image_base64": format!("data:image/png;base64,{PNG_B64}") }
                    ]
                },
                {
                    "index": 1,
                    "markdown": "More on page two.",
                    "images": []
                }
            ]
        });
        let out = parse_ocr_response(&resp).unwrap();
        // Page link rewritten to the namespaced filename, both pages stitched.
        assert!(out.markdown.contains("](./p0_img-0.jpeg)"));
        assert!(out.markdown.contains("# Title"));
        assert!(out.markdown.contains("More on page two."));
        assert_eq!(out.images.len(), 1);
        assert_eq!(out.images[0].0, "p0_img-0.jpeg");
        assert_eq!(&out.images[0].1[..8], b"\x89PNG\r\n\x1a\n");
    }
}
