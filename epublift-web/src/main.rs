// epublift-web - Browser front-end for epublift: upload an EPUB, convert it,
// download the result. Part of epublift; licensed under the GNU AGPL-3.0-or-later.
//
// The conversion itself is performed by the `epublift` library `convert()` API.
// This crate is only a thin, hardened HTTP/multipart wrapper around it.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path as UrlPath, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use epublift::{EpubVersion, ImageStrategy, Options};
use serde::Serialize;
use tokio::sync::Semaphore;
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

/// Hard limits for the public endpoint. The source is open, so these are the
/// real defense — not obscurity.
const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024; // 50 MiB
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a converted file waits in memory for its one download before it is
/// evicted. Files live only in RAM and are never written to disk or logged.
const DOWNLOAD_TTL: Duration = Duration::from_secs(120);

/// Token-bucket rate limit per client IP: a burst of `RL_BURST` conversions,
/// refilling at `RL_REFILL_PER_SEC`.
const RL_BURST: f64 = 6.0;
const RL_REFILL_PER_SEC: f64 = 0.2; // ~1 conversion every 5s, sustained

/// A converted/archived/restored file held in memory awaiting download.
struct Pending {
    name: String,
    bytes: Vec<u8>,
    /// MIME type for the download response — `application/epub+zip` for EPUBs,
    /// `application/octet-stream` for `.eparc` archives.
    content_type: &'static str,
    born: Instant,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Simple per-IP token-bucket limiter.
#[derive(Default)]
struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    /// Consume a token for `ip`; returns false when the bucket is empty.
    fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self.buckets.lock().unwrap();
        let b = map.entry(ip).or_insert(Bucket {
            tokens: RL_BURST,
            last: now,
        });
        let elapsed = now.duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * RL_REFILL_PER_SEC).min(RL_BURST);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Drop buckets that have been idle for a while.
    fn prune(&self) {
        let now = Instant::now();
        self.buckets
            .lock()
            .unwrap()
            .retain(|_, b| now.duration_since(b.last) < Duration::from_secs(600));
    }
}

/// Best-effort client IP: the first `X-Forwarded-For` entry (set by the trusted
/// reverse proxy) if present, else the direct peer address.
fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(first) = xff.split(',').next()
        && let Ok(ip) = first.trim().parse::<IpAddr>()
    {
        return ip;
    }
    peer.ip()
}

/// Shared service state.
struct AppState {
    /// Caps simultaneous (CPU-heavy) conversions so latency stays predictable.
    convert_slots: Semaphore,
    /// Converted files awaiting download, keyed by an unguessable token.
    pending: Mutex<HashMap<String, Pending>>,
    /// Per-IP request rate limiter.
    limiter: RateLimiter,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "epublift_web=info,tower_http=warn".into()),
        )
        .init();

    // One conversion per core, minimum two; keeps the box responsive.
    let slots = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .max(2);
    let state = Arc::new(AppState {
        convert_slots: Semaphore::new(slots),
        pending: Mutex::new(HashMap::new()),
        limiter: RateLimiter::default(),
    });

    // Sweep expired downloads and idle rate-limit buckets.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                state
                    .pending
                    .lock()
                    .unwrap()
                    .retain(|_, p| p.born.elapsed() < DOWNLOAD_TTL);
                state.limiter.prune();
            }
        });
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/i18n.js", get(i18n_js))
        .route("/healthz", get(|| async { "ok" }))
        .route("/convert", post(convert))
        .route("/archive", post(archive))
        .route("/restore", post(restore))
        .route("/download/{token}", get(download))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        // Security headers on every response.
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            header::HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("x-frame-options"),
            header::HeaderValue::from_static("DENY"),
        ))
        // Content-Security-Policy: deny everything by default, then allow only
        // what the single page actually loads. The front-end script lives at
        // its own `/app.js` so we can use `script-src 'self'` (no inline JS).
        // `'unsafe-inline'` is only granted to styles (the inline <style> block
        // and many style="..." attributes); there is no HTML-injection sink, so
        // this is low-risk. Fonts come from Google Fonts; everything else is
        // same-origin or `data:`.
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static(
                "default-src 'none'; \
                 script-src 'self'; \
                 style-src 'unsafe-inline' https://fonts.googleapis.com; \
                 font-src https://fonts.gstatic.com; \
                 img-src 'self' data:; \
                 connect-src 'self'; \
                 base-uri 'none'; \
                 form-action 'none'; \
                 frame-ancestors 'none'; \
                 object-src 'none'",
            ),
        ))
        // No cross-origin allow-list -> only the page's own origin may read
        // responses, so the endpoint can't be embedded by other sites.
        .layer(CorsLayer::new())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("epublift-web listening on http://{addr}  ({slots} convert slots)");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .unwrap();
}

/// Resolve on Ctrl-C (SIGINT) or SIGTERM (e.g. `docker stop`), so the server
/// stops cleanly on the first signal instead of being force-killed — and a
/// foreground `docker run` doesn't leave an orphaned container holding the port.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received — stopping");
}

/// Serve the single-page front-end.
async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

/// Serve the front-end script as a same-origin file (so the CSP can use
/// `script-src 'self'` instead of allowing inline scripts).
async fn app_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("text/javascript; charset=utf-8"),
        )],
        include_str!("../static/app.js"),
    )
}

/// Serve the i18n bundle (translation strings + language switcher) as a
/// same-origin script, like `/app.js`, so it loads under `script-src 'self'`.
async fn i18n_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("text/javascript; charset=utf-8"),
        )],
        include_str!("../static/i18n.js"),
    )
}

#[derive(Serialize)]
struct ImageRow {
    name: String,
    before: u64,
    after: u64,
    saved_pct: f64,
}

#[derive(Serialize)]
struct ConvertResponse {
    output_name: String,
    original_size: u64,
    final_size: u64,
    saved_pct: f64,
    images: Vec<ImageRow>,
    /// The CLI-identical text audit report (for the "Download report" link).
    report_text: String,
    /// One-time token to fetch the converted EPUB from `/download/{token}`.
    download_token: String,
}

/// A user-facing error with an HTTP status.
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

/// Convert an uploaded EPUB; return the report as JSON plus a download token.
async fn convert(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ConvertResponse>, ApiError> {
    // Rate-limit before reading the (potentially large) upload body.
    if !state.limiter.allow(client_ip(&headers, peer)) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests — please wait a moment and try again.".into(),
        ));
    }

    let (file_bytes, raw_name, fields) = read_upload(&mut multipart).await?;
    let file_name = if raw_name.trim().is_empty() {
        "book.epub".to_string()
    } else {
        raw_name
    };
    let quality: u8 = fields
        .get("quality")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(80);
    let ascii = field_on(&fields, "ascii");
    let kepub = field_on(&fields, "kepub");
    let keep_images = field_on(&fields, "keep_images");

    // Experimental EPUB 3.4: AVIF / JPEG XL images. Defaults to 3.3 / WebP.
    let target_34 = fields.get("target").map(|s| s.trim()) == Some("3.4");
    let target_version = if target_34 {
        EpubVersion::V3_4
    } else {
        EpubVersion::LATEST
    };
    let image_policy = if target_34 {
        match fields.get("image_format").map(|s| s.trim()) {
            Some("avif") => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::Avif)),
            Some("jxl") => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::Jxl)),
            _ => None,
        }
    } else {
        None
    };

    // Bound concurrent conversions.
    let _permit = state
        .convert_slots
        .acquire()
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "server busy".into()))?;

    // Everything below happens in a throwaway temp dir that is deleted when this
    // task ends — on success or error. No upload is ever persisted or logged.
    let result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<(ConvertResponse, Vec<u8>)> {
            let tmp = tempfile::Builder::new().prefix("epublift_web_").tempdir()?;
            let input_path = tmp.path().join(&file_name);
            std::fs::write(&input_path, &file_bytes)?;

            let opts = Options {
                quality,
                ascii,
                target_version,
                image_strategy: if keep_images {
                    ImageStrategy::KeepOriginal
                } else {
                    ImageStrategy::WebP
                },
                image_policy,
                kepub,
                // The hosted service only ever emits conformant EPUBs; the
                // experimental Zstd packaging is CLI/research-only.
                packaging: epublift::Packaging::Deflate,
                output: None,
            };
            let report = epublift::convert(&input_path, &opts, |_| {})?;

            let out_bytes = std::fs::read(&report.output_path)?;

            // Reuse the CLI's exact report formatting.
            let report_txt_path = tmp.path().join("report.txt");
            report.write_text_report(&report_txt_path)?;
            let report_text = std::fs::read_to_string(&report_txt_path)?;

            let images = report
                .image_metrics
                .iter()
                .map(|m| ImageRow {
                    name: m.name.clone(),
                    before: m.original_size,
                    after: m.new_size,
                    saved_pct: m.percentage,
                })
                .collect();

            let resp = ConvertResponse {
                output_name: report.output_name.clone(),
                original_size: report.original_size,
                final_size: report.final_size,
                saved_pct: report.percent_saved(),
                images,
                report_text,
                download_token: String::new(), // filled in below
            };
            // `tmp` drops here -> temp dir removed.
            Ok((resp, out_bytes))
        })
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "conversion crashed".into(),
            )
        })?;

    let (mut resp, out_bytes) = result
        // Most failures here are invalid/corrupt EPUBs -> client error.
        .map_err(|e| bad_request(format!("could not convert this EPUB: {e}")))?;

    // Stash the result in memory for a single short-lived download.
    let token = uuid::Uuid::new_v4().to_string();
    state.pending.lock().unwrap().insert(
        token.clone(),
        Pending {
            name: resp.output_name.clone(),
            bytes: out_bytes,
            content_type: "application/epub+zip",
            born: Instant::now(),
        },
    );
    resp.download_token = token;

    Ok(Json(resp))
}

/// Stash a finished file in memory for one short-lived token download.
fn stash(state: &AppState, name: String, bytes: Vec<u8>, content_type: &'static str) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    state.pending.lock().unwrap().insert(
        token.clone(),
        Pending {
            name,
            bytes,
            content_type,
            born: Instant::now(),
        },
    );
    token
}

/// Read the single uploaded `file` part (plus any extra text fields) from a
/// multipart body. Returns the file bytes, its basename, and the text fields.
async fn read_upload(
    multipart: &mut Multipart,
) -> Result<(Vec<u8>, String, HashMap<String, String>), ApiError> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name = String::new();
    let mut fields: HashMap<String, String> = HashMap::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(format!("malformed upload: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            // Keep only the basename of the (untrusted) filename.
            if let Some(fname) = field.file_name()
                && let Some(base) = Path::new(fname).file_name()
            {
                file_name = base.to_string_lossy().into_owned();
            }
            let bytes = field
                .bytes()
                .await
                .map_err(|e| bad_request(format!("could not read file: {e}")))?;
            file_bytes = Some(bytes.to_vec());
        } else {
            let v = field.text().await.unwrap_or_default();
            fields.insert(name, v);
        }
    }

    let bytes = file_bytes.ok_or_else(|| bad_request("no file was uploaded"))?;
    if bytes.is_empty() {
        return Err(bad_request("the uploaded file is empty"));
    }
    Ok((bytes, file_name, fields))
}

/// Whether a text multipart field reads as "on".
fn field_on(fields: &HashMap<String, String>, key: &str) -> bool {
    matches!(fields.get(key).map(|s| s.trim()), Some("true" | "on" | "1"))
}

#[derive(Serialize)]
struct ArchiveResponse {
    output_name: String,
    original_size: u64,
    archive_size: u64,
    saved_pct: f64,
    compressed_entries: usize,
    stored_entries: usize,
    download_token: String,
}

/// Archive an uploaded `.epub` into a compact `.eparc`; return stats + a token.
async fn archive(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ArchiveResponse>, ApiError> {
    if !state.limiter.allow(client_ip(&headers, peer)) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests — please wait a moment and try again.".into(),
        ));
    }

    let (file_bytes, file_name, fields) = read_upload(&mut multipart).await?;
    // `ascii` transliterates the archive's *download* name (Işık → Isik) for older
    // devices/filesystems. The archived bytes and the manifest's recorded source
    // name keep the original — only the user-facing .eparc filename changes.
    let ascii = field_on(&fields, "ascii");
    let out_stem = {
        let s = epublift::output_stem(Path::new(&file_name), ascii);
        if s.trim().is_empty() {
            "book".to_string()
        } else {
            s
        }
    };
    let src_name = if file_name.trim().is_empty() {
        "book.epub".to_string()
    } else {
        file_name
    };

    let _permit = state
        .convert_slots
        .acquire()
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "server busy".into()))?;

    let result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<(ArchiveResponse, Vec<u8>)> {
            let tmp = tempfile::Builder::new().prefix("epublift_arc_").tempdir()?;
            // Keep the original name on the temp input so it lands in the manifest;
            // the output .eparc carries the (possibly transliterated) out_stem.
            let input_path = tmp.path().join(&src_name);
            let out_path = tmp.path().join(format!("{out_stem}.eparc"));
            std::fs::write(&input_path, &file_bytes)?;

            let stats = epublift::eparc::archive_epub(&input_path, &out_path)?;
            let out_bytes = std::fs::read(&out_path)?;

            let resp = ArchiveResponse {
                output_name: format!("{out_stem}.eparc"),
                original_size: stats.original_size,
                archive_size: stats.archive_size,
                saved_pct: stats.percent_saved(),
                compressed_entries: stats.compressed_entries,
                stored_entries: stats.stored_entries,
                download_token: String::new(),
            };
            Ok((resp, out_bytes))
        })
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "archive crashed".into()))?;

    let (mut resp, out_bytes) =
        result.map_err(|e| bad_request(format!("could not archive this EPUB: {e}")))?;

    resp.download_token = stash(
        &state,
        resp.output_name.clone(),
        out_bytes,
        "application/octet-stream",
    );
    Ok(Json(resp))
}

#[derive(Serialize)]
struct RestoreResponse {
    output_name: String,
    output_size: u64,
    entries: usize,
    /// Whether the book was re-optimized on the way out (vs. content-exact).
    modernized: bool,
    download_token: String,
}

/// Restore an uploaded `.eparc` back to a `.epub`. Content-exact by default; with
/// `modernize` it re-runs the optimizer (mirroring the CLI `restore --target`).
async fn restore(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<RestoreResponse>, ApiError> {
    if !state.limiter.allow(client_ip(&headers, peer)) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests — please wait a moment and try again.".into(),
        ));
    }

    let (file_bytes, file_name, fields) = read_upload(&mut multipart).await?;
    let stem = file_stem_or(&file_name, "book");
    let modernize = field_on(&fields, "modernize");
    let keep_images = field_on(&fields, "keep_images");
    let kepub = field_on(&fields, "kepub");
    let quality: u8 = fields
        .get("quality")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(80);

    let _permit = state
        .convert_slots
        .acquire()
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "server busy".into()))?;

    let result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<(RestoreResponse, Vec<u8>)> {
            let tmp = tempfile::Builder::new().prefix("epublift_res_").tempdir()?;
            let input_path = tmp.path().join(format!("{stem}.eparc"));
            std::fs::write(&input_path, &file_bytes)?;

            if !modernize {
                // Content-exact: hand back the original book, byte-for-byte.
                let out_path = tmp.path().join(format!("{stem}.epub"));
                let stats = epublift::eparc::restore_eparc(&input_path, &out_path)?;
                let out_bytes = std::fs::read(&out_path)?;
                let resp = RestoreResponse {
                    output_name: format!("{stem}.epub"),
                    output_size: stats.output_size,
                    entries: stats.entries,
                    modernized: false,
                    download_token: String::new(),
                };
                return Ok((resp, out_bytes));
            }

            // Modernize: restore content-exact into a temp file, then optimize it.
            let restored = tmp.path().join("restored.epub");
            epublift::eparc::restore_eparc(&input_path, &restored)?;

            let opts = Options {
                quality,
                ascii: false,
                target_version: EpubVersion::LATEST,
                image_strategy: if keep_images {
                    ImageStrategy::KeepOriginal
                } else {
                    ImageStrategy::WebP
                },
                image_policy: None,
                kepub,
                packaging: epublift::Packaging::Deflate,
                output: None,
            };
            // Name the output the way `convert` does, but based on the archive's
            // stem rather than the temp file.
            let name_basis = tmp.path().join(format!("{stem}.epub"));
            let out_path = epublift::default_output_path(&name_basis, &opts);
            let opts = Options {
                output: Some(out_path.clone()),
                ..opts
            };
            let report = epublift::convert(&restored, &opts, |_| {})?;
            let out_bytes = std::fs::read(&report.output_path)?;
            let resp = RestoreResponse {
                output_name: report.output_name.clone(),
                output_size: report.final_size,
                entries: report.image_metrics.len(),
                modernized: true,
                download_token: String::new(),
            };
            Ok((resp, out_bytes))
        })
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "restore crashed".into()))?;

    let (mut resp, out_bytes) =
        result.map_err(|e| bad_request(format!("could not restore this .eparc: {e}")))?;

    resp.download_token = stash(
        &state,
        resp.output_name.clone(),
        out_bytes,
        "application/epub+zip",
    );
    Ok(Json(resp))
}

/// The file stem of an (untrusted) upload name, or a fallback if it has none.
fn file_stem_or(name: &str, fallback: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Stream a converted EPUB by token, then drop it from memory (one-shot).
async fn download(State(state): State<Arc<AppState>>, UrlPath(token): UrlPath<String>) -> Response {
    let pending = {
        let mut map = state.pending.lock().unwrap();
        match map.remove(&token) {
            Some(p) if p.born.elapsed() < DOWNLOAD_TTL => Some(p),
            _ => None,
        }
    };

    match pending {
        Some(p) => {
            let safe = sanitize_filename(&p.name);
            (
                [
                    (header::CONTENT_TYPE, p.content_type.to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{safe}\""),
                    ),
                ],
                p.bytes,
            )
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            "This download link has expired or was already used.",
        )
            .into_response(),
    }
}

/// ASCII-safe filename for the `Content-Disposition` header (no header
/// injection, no path separators). The browser uses the page's `download`
/// attribute for the actual saved name, so this is only a fallback.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "epublift-output.epub".to_string()
    } else {
        trimmed.to_string()
    }
}
