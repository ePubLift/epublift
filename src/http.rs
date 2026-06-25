//! A small, **100% pure-Rust** HTTPS client for the `metadata` feature — no C
//! toolchain, no `ring`/`aws-lc`. TLS is `rustls` driven by the **RustCrypto**
//! crypto provider, trusting the Mozilla roots from `webpki-roots`, over a
//! hand-rolled HTTP/1.1 GET (with redirect following and chunked decoding).
//!
//! This is the project's first outbound network path; it exists only behind the
//! opt-in `metadata` feature so the default build stays offline and C-free.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

#[cfg(feature = "metadata")]
use crate::enrich::Http;

const USER_AGENT: &str = concat!(
    "epublift/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/ePubLift/epublift)"
);
const MAX_REDIRECTS: u8 = 5;
#[cfg(feature = "metadata")]
const MAX_BODY: usize = 8 * 1024 * 1024;

/// A reusable HTTPS client backed by a pure-Rust rustls config.
pub struct RustlsHttp {
    config: Arc<rustls::ClientConfig>,
}

impl RustlsHttp {
    /// Build the client (loads the trust roots and the RustCrypto provider).
    pub fn new() -> Result<Self> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config =
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls_rustcrypto::provider()))
                .with_safe_default_protocol_versions()
                .context("rustls: could not configure TLS protocol versions")?
                .with_root_certificates(roots)
                .with_no_client_auth();

        Ok(Self {
            config: Arc::new(config),
        })
    }

    /// Perform a single GET (no redirect handling), returning the raw response
    /// bytes (status line + headers + body).
    fn fetch_once(&self, host: &str, path: &str, max_body: usize) -> Result<Vec<u8>> {
        let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
            .with_context(|| format!("invalid hostname: {host}"))?;
        let mut conn = rustls::ClientConnection::new(self.config.clone(), server_name)
            .context("rustls: could not start TLS session")?;
        let mut sock = TcpStream::connect((host, 443))
            .with_context(|| format!("could not connect to {host}:443"))?;
        let mut tls = rustls::Stream::new(&mut conn, &mut sock);

        let request = format!(
            "GET {path} HTTP/1.1\r\n\
             Host: {host}\r\n\
             User-Agent: {USER_AGENT}\r\n\
             Accept: application/json\r\n\
             Accept-Encoding: identity\r\n\
             Connection: close\r\n\r\n"
        );
        tls.write_all(request.as_bytes())
            .context("failed to send HTTPS request")?;
        let _ = tls.flush();

        let mut resp = Vec::new();
        let mut buf = [0u8; 16384];
        loop {
            match tls.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    resp.extend_from_slice(&buf[..n]);
                    if resp.len() > max_body {
                        bail!("response from {host} exceeds the {max_body}-byte limit");
                    }
                }
                // Many servers close the TCP connection without a TLS close_notify;
                // rustls surfaces that as UnexpectedEof — treat it as a clean end.
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionAborted
                    ) =>
                {
                    break;
                }
                Err(e) => return Err(e).context("error reading HTTPS response"),
            }
        }
        Ok(resp)
    }
}

#[cfg(feature = "metadata")]
impl Http for RustlsHttp {
    fn get(&self, url: &str) -> Result<String> {
        let mut current = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let (host, path) = parse_https_url(&current)?;
            let raw = self.fetch_once(&host, &path, MAX_BODY)?;
            let (status, headers, body) = split_response(&raw)?;
            match status {
                200 => return String::from_utf8(body).context("response was not valid UTF-8"),
                301 | 302 | 303 | 307 | 308 => {
                    let loc = header_value(&headers, "location")
                        .context("redirect response had no Location header")?;
                    current = resolve_redirect(&host, &loc);
                }
                404 => bail!("not found (HTTP 404)"),
                429 => bail!(
                    "rate limited (HTTP 429) — the provider's quota is exhausted; \
                     try again later or set an API key"
                ),
                403 => bail!("access denied (HTTP 403)"),
                other => bail!("unexpected HTTP status {other}"),
            }
        }
        bail!("too many redirects (>{MAX_REDIRECTS})")
    }
}

impl RustlsHttp {
    /// Binary GET (follows redirects), for downloading larger files such as the
    /// OCR models. Capped well above the JSON limit.
    #[cfg(feature = "pdf-ocr")]
    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        const MAX_FILE: usize = 64 * 1024 * 1024;
        let mut current = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let (host, path) = parse_https_url(&current)?;
            let raw = self.fetch_once(&host, &path, MAX_FILE)?;
            let (status, headers, body) = split_response(&raw)?;
            match status {
                200 => return Ok(body),
                301 | 302 | 303 | 307 | 308 => {
                    let loc = header_value(&headers, "location")
                        .context("redirect response had no Location header")?;
                    current = resolve_redirect(&host, &loc);
                }
                404 => bail!("not found (HTTP 404)"),
                other => bail!("unexpected HTTP status {other}"),
            }
        }
        bail!("too many redirects (>{MAX_REDIRECTS})")
    }
}

/// Split `https://host/path` into `(host, path)`. Only HTTPS is supported.
fn parse_https_url(url: &str) -> Result<(String, String)> {
    let rest = url
        .strip_prefix("https://")
        .with_context(|| format!("only https URLs are supported: {url}"))?;
    Ok(match rest.find('/') {
        Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    })
}

/// Resolve a `Location` header (absolute or root-relative) against the host.
fn resolve_redirect(host: &str, location: &str) -> String {
    if location.starts_with("https://") {
        location.to_string()
    } else if let Some(rest) = location.strip_prefix("http://") {
        format!("https://{rest}") // upgrade to HTTPS
    } else if location.starts_with('/') {
        format!("https://{host}{location}")
    } else {
        format!("https://{host}/{location}")
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn header_value(headers: &[(String, String)], key: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

/// A parsed HTTP response: `(status, headers, body)` with lowercased header names.
type ParsedResponse = (u16, Vec<(String, String)>, Vec<u8>);

/// Parse a raw HTTP/1.1 response into `(status, headers, body)`, decoding a
/// chunked body if needed. Header names are lowercased.
fn split_response(raw: &[u8]) -> Result<ParsedResponse> {
    let sep = find_subsequence(raw, b"\r\n\r\n")
        .context("malformed HTTP response (no header terminator)")?;
    let head = String::from_utf8_lossy(&raw[..sep]);
    let mut body = raw[sep + 4..].to_vec();

    let mut lines = head.split("\r\n");
    let status_line = lines.next().context("empty HTTP response")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .context("HTTP response had no status code")?
        .parse()
        .context("HTTP response had an invalid status code")?;

    let mut headers = Vec::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    if header_value(&headers, "transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        body = dechunk(&body)?;
    }
    Ok((status, headers, body))
}

/// Decode an HTTP/1.1 chunked transfer-encoded body.
fn dechunk(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let line_end = find_subsequence(&data[i..], b"\r\n")
            .map(|p| p + i)
            .context("malformed chunk (no size line)")?;
        let size_field = std::str::from_utf8(&data[i..line_end])
            .context("malformed chunk size")?
            .trim();
        // A chunk size may carry extensions after `;`.
        let size_hex = size_field.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).context("invalid chunk size")?;
        i = line_end + 2;
        if size == 0 {
            break;
        }
        let end = i + size;
        if end > data.len() {
            bail!("truncated chunked body");
        }
        out.extend_from_slice(&data[i..end]);
        i = end + 2; // skip the chunk's trailing CRLF
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url() {
        assert_eq!(
            parse_https_url("https://openlibrary.org/isbn/123.json").unwrap(),
            ("openlibrary.org".to_string(), "/isbn/123.json".to_string())
        );
        assert!(parse_https_url("http://x/").is_err());
    }

    #[test]
    fn resolves_redirects() {
        assert_eq!(
            resolve_redirect("a.com", "/books/OL1M.json"),
            "https://a.com/books/OL1M.json"
        );
        assert_eq!(
            resolve_redirect("a.com", "https://b.com/x"),
            "https://b.com/x"
        );
    }

    #[test]
    fn splits_and_dechunks() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        let (status, _h, body) = split_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(String::from_utf8(body).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn splits_plain_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi";
        let (status, _h, body) = split_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"hi");
    }
}
