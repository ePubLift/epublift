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
/// AVIF encoder speed for the experimental 3.4 path. Faster than the CLI default
/// (4) so an interactive request stays well under [`REQUEST_TIMEOUT`] even on a
/// photo-heavy book — AVIF (rav1e) is slow at low speeds. Costs a little size.
const WEB_AVIF_SPEED: u8 = 6;
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
        .route("/version", get(version))
        .route("/convert", post(convert))
        .route("/import", post(import))
        .route("/archive", post(archive))
        .route("/restore", post(restore))
        .route("/meta/read", post(meta_read))
        .route("/meta/enrich", post(meta_enrich))
        .route("/meta/write", post(meta_write))
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

/// Report the running build: the `epublift-web` version and the short git commit
/// it was built from (`commit` is empty if unavailable). Cheap, machine-readable
/// deploy verification, and the source for the footer's version link.
async fn version() -> impl IntoResponse {
    let body = format!(
        r#"{{"version":"{}","commit":"{}"}}"#,
        env!("CARGO_PKG_VERSION"),
        env!("GIT_SHA"),
    );
    (
        [(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        )],
        body,
    )
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
        // Attach a stable `code` so the front-end can localize the common cases;
        // `error` keeps the English detail as a fallback / for unexpected errors.
        let code = classify_error(&self.1, self.0);
        (
            self.0,
            Json(serde_json::json!({ "error": self.1, "code": code })),
        )
            .into_response()
    }
}

/// Classify an error message + status into a stable, translatable code.
fn classify_error(msg: &str, status: StatusCode) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if status == StatusCode::TOO_MANY_REQUESTS
        || m.contains("too many requests")
        || m.contains("server busy")
    {
        "busy"
    } else if m.contains("429") || m.contains("rate limited") {
        "rate_limited"
    } else if m.contains("not found") {
        "not_found"
    } else if m.contains("enter an isbn") {
        "no_isbn"
    } else {
        "failed"
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

    // Experimental EPUB 3.4: AVIF / JPEG XL images. Defaults to 3.3 / WebP.
    let target_34 = fields.get("target").map(|s| s.trim()) == Some("3.4");
    let target_version = if target_34 {
        EpubVersion::V3_4
    } else {
        EpubVersion::LATEST
    };
    // The image-format pill drives everything: keep | webp | avif | jxl (AVIF/JXL
    // only on 3.4). `webp` is forced explicitly so 3.4 emits WebP rather than its
    // content-adaptive default.
    let image_format = fields
        .get("image_format")
        .map(|s| s.trim())
        .unwrap_or("keep");
    let keep_images = image_format == "keep";
    let image_policy = match image_format {
        "webp" => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::WebP)),
        "avif" if target_34 => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::Avif)),
        "jxl" if target_34 => Some(epublift::FormatPolicy::Fixed(epublift::ImageFormat::Jxl)),
        _ => None,
    };
    // For .kepub, a non-"keep" format opts into emitting it (needs the Kobo WebP
    // plugin); "keep" keeps originals (safe on stock Kobo).
    let kepub_webp = kepub && !keep_images;

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
                avif_speed: WEB_AVIF_SPEED,
                kepub,
                kepub_webp,
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

/// [EXPERIMENTAL] Import a PDF into a reflow EPUB (text tiers; OCR not yet).
async fn import(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ImportResponse>, ApiError> {
    if !state.limiter.allow(client_ip(&headers, peer)) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests — please wait a moment and try again.".into(),
        ));
    }

    let (file_bytes, raw_name, fields) = read_upload(&mut multipart).await?;
    let file_name = if raw_name.trim().is_empty() {
        "book.pdf".to_string()
    } else {
        raw_name
    };
    let language = fields
        .get("language")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mode = match fields.get("mode").map(|s| s.trim()) {
        Some("fixed") => epublift::pdf::Mode::Fixed,
        _ => epublift::pdf::Mode::Reflow,
    };

    // Bound concurrent jobs (shares the convert pool).
    let _permit = state
        .convert_slots
        .acquire()
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "server busy".into()))?;

    // Throwaway temp dir, deleted when the task ends. No upload is persisted.
    let result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<(ImportResponse, Vec<u8>)> {
            let tmp = tempfile::Builder::new().prefix("epublift_web_").tempdir()?;
            let input_path = tmp.path().join(&file_name);
            std::fs::write(&input_path, &file_bytes)?;
            let out_path = input_path.with_extension("epub");

            let opts = epublift::pdf::ImportOptions { mode, language };
            let summary = epublift::pdf::import(&input_path, &out_path, &opts)?;
            let out_bytes = std::fs::read(&out_path)?;

            let resp = ImportResponse {
                output_name: out_path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "book.epub".into()),
                chapters: summary.chapters,
                paragraphs: summary.paragraphs,
                final_size: out_bytes.len() as u64,
                download_token: String::new(), // filled in below
            };
            Ok((resp, out_bytes))
        })
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "import crashed".into()))?;

    // Most failures are scans needing OCR, CID-font PDFs, or non-PDFs — surface
    // the library's helpful message as a client error.
    let (mut resp, out_bytes) =
        result.map_err(|e| bad_request(format!("could not import this PDF: {e}")))?;

    resp.download_token = stash(
        &state,
        resp.output_name.clone(),
        out_bytes,
        "application/epub+zip",
    );
    Ok(Json(resp))
}

#[derive(Serialize)]
struct ImportResponse {
    output_name: String,
    chapters: usize,
    paragraphs: usize,
    final_size: u64,
    download_token: String,
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
    let kepub = field_on(&fields, "kepub");
    // Restore re-targets to 3.3, so the image-format pill is keep | webp.
    let image_format = fields
        .get("image_format")
        .map(|s| s.trim())
        .unwrap_or("keep");
    let keep_images = image_format == "keep";
    let kepub_webp = kepub && !keep_images;
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
                avif_speed: WEB_AVIF_SPEED,
                kepub,
                kepub_webp,
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

// ---------------------------------------------------------------------------
// Metadata editor (read / enrich-by-ISBN / write)
// ---------------------------------------------------------------------------

/// A trimmed multipart field, or `None` if missing/blank.
fn nonempty(fields: &HashMap<String, String>, key: &str) -> Option<String> {
    fields
        .get(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Parse a newline-separated multivalue field (authors, subjects) into a list.
fn parse_lines(fields: &HashMap<String, String>, key: &str) -> Option<Vec<String>> {
    let items: Vec<String> = fields
        .get(key)
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    (!items.is_empty()).then_some(items)
}

/// `Name` or `Name:position` → a `Series`.
fn parse_series(s: &str) -> epublift::meta::Series {
    if let Some((name, pos)) = s.rsplit_once(':')
        && !pos.is_empty()
        && pos.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return epublift::meta::Series {
            name: name.to_string(),
            position: Some(pos.to_string()),
        };
    }
    epublift::meta::Series {
        name: s.to_string(),
        position: None,
    }
}

fn too_many_requests() -> ApiError {
    ApiError(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many requests — please wait a moment and try again.".into(),
    )
}

/// The form-facing JSON view of a book's metadata.
fn metadata_json(md: &epublift::meta::Metadata) -> serde_json::Value {
    let main_title = md
        .titles
        .iter()
        .find(|t| t.title_type.as_deref() != Some("subtitle"))
        .map(|t| t.value.clone());
    let subtitle = md
        .titles
        .iter()
        .find(|t| t.title_type.as_deref() == Some("subtitle"))
        .map(|t| t.value.clone());
    let isbn = md
        .identifiers
        .iter()
        .find_map(|i| {
            if i.scheme
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
            {
                Some(i.value.clone())
            } else {
                i.value.strip_prefix("urn:isbn:").map(str::to_string)
            }
        })
        .or_else(|| {
            md.source
                .as_ref()
                .and_then(|s| s.strip_prefix("urn:isbn:").map(str::to_string))
        });
    serde_json::json!({
        "epub_version": md.epub_version,
        "title": main_title,
        "subtitle": subtitle,
        "authors": md.creators.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        "language": md.languages.first().cloned(),
        "publisher": md.publisher,
        "date": md.date,
        "description": md.description,
        "subjects": md.subjects,
        "series": md.series.as_ref().map(|s| serde_json::json!({"name": s.name, "position": s.position})),
        "isbn": isbn,
    })
}

/// Write an uploaded EPUB's bytes to a temp file and read its current metadata.
fn read_metadata_from_bytes(name: &str, bytes: &[u8]) -> anyhow::Result<epublift::meta::Metadata> {
    let tmp = tempfile::Builder::new().prefix("epublift_web_").tempdir()?;
    let safe = if name.trim().is_empty() {
        "book.epub"
    } else {
        name
    };
    let path = tmp.path().join(safe);
    std::fs::write(&path, bytes)?;
    epublift::read_metadata(&path)
}

/// Read the current metadata of an uploaded EPUB (to populate the editor form).
async fn meta_read(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !state.limiter.allow(client_ip(&headers, peer)) {
        return Err(too_many_requests());
    }
    let (file_bytes, name, _fields) = read_upload(&mut multipart).await?;
    let md = tokio::task::spawn_blocking(move || read_metadata_from_bytes(&name, &file_bytes))
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "crashed".into()))?
        .map_err(|e| bad_request(format!("could not read this EPUB: {e}")))?;
    Ok(Json(metadata_json(&md)))
}

/// Look up an ISBN on Open Library and return the language-aware suggestion.
async fn meta_enrich(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !state.limiter.allow(client_ip(&headers, peer)) {
        return Err(too_many_requests());
    }
    let (file_bytes, name, fields) = read_upload(&mut multipart).await?;
    let isbn = nonempty(&fields, "isbn").ok_or_else(|| bad_request("please enter an ISBN"))?;
    let provider = nonempty(&fields, "provider").unwrap_or_else(|| "openlibrary".to_string());
    let opts = epublift::enrich::EnrichOptions {
        lang_override: nonempty(&fields, "lang"),
        overwrite: field_on(&fields, "overwrite"),
        allow_foreign_meta: field_on(&fields, "allow_foreign_meta"),
        include_description: field_on(&fields, "include_description"),
    };

    let _permit = state
        .convert_slots
        .acquire()
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "server busy".into()))?;

    let plan =
        tokio::task::spawn_blocking(move || -> anyhow::Result<epublift::enrich::EnrichPlan> {
            let existing = read_metadata_from_bytes(&name, &file_bytes)?;
            let http = epublift::http::RustlsHttp::new()?;
            let fetched =
                epublift::enrich::fetch_isbn(&provider, &isbn, &http, opts.include_description)?;
            epublift::enrich::plan_enrich(&existing, &fetched, &opts)
        })
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "crashed".into()))?
        // `{e:#}` includes the anyhow cause chain (e.g. "… rate limited (HTTP 429) …").
        .map_err(|e| bad_request(format!("lookup failed: {e:#}")))?;

    // Structured applied/skipped/warnings so the front-end localizes the field
    // names and reasons (see app.js `applyEnrich`).
    use epublift::enrich::{SkipReason, Warning};
    let applied: Vec<_> = plan
        .applied
        .iter()
        .map(|a| serde_json::json!({"field": a.field, "value": a.value}))
        .collect();
    let skipped: Vec<_> = plan
        .skipped
        .iter()
        .map(|s| {
            let reason = match s.reason {
                SkipReason::AlreadySet => "present",
                SkipReason::LanguageMismatch => "lang",
                SkipReason::DescriptionOmitted => "omitted",
            };
            serde_json::json!({"field": s.field, "reason": reason})
        })
        .collect();
    let warnings: Vec<_> = plan
        .warnings
        .iter()
        .map(|w| match w {
            Warning::EditionLanguageMismatch { edition, book } => {
                serde_json::json!({"type": "edition_lang", "edition": edition, "book": book})
            }
        })
        .collect();

    let u = &plan.update;
    Ok(Json(serde_json::json!({
        "book_lang": plan.book_lang,
        "applied": applied,
        "skipped": skipped,
        "warnings": warnings,
        "fields": {
            "title": u.title,
            "subtitle": u.subtitle,
            "authors": u.authors,
            "publisher": u.publisher,
            "date": u.date,
            "description": u.description,
            "subjects": u.subjects,
            "series": u.series.as_ref().map(|s| serde_json::json!({"name": s.name, "position": s.position})),
            "isbn": u.isbn,
        }
    })))
}

#[derive(Serialize)]
struct MetaWriteResponse {
    output_name: String,
    metadata: serde_json::Value,
    download_token: String,
}

/// Write the edited metadata into the uploaded EPUB and stash it for download.
async fn meta_write(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<MetaWriteResponse>, ApiError> {
    if !state.limiter.allow(client_ip(&headers, peer)) {
        return Err(too_many_requests());
    }
    let (file_bytes, raw_name, fields) = read_upload(&mut multipart).await?;
    let file_name = if raw_name.trim().is_empty() {
        "book.epub".to_string()
    } else {
        raw_name
    };

    let update = epublift::meta::MetadataUpdate {
        title: nonempty(&fields, "title"),
        subtitle: nonempty(&fields, "subtitle"),
        authors: parse_lines(&fields, "authors"),
        language: nonempty(&fields, "language"),
        publisher: nonempty(&fields, "publisher"),
        date: nonempty(&fields, "date"),
        description: nonempty(&fields, "description"),
        subjects: parse_lines(&fields, "subjects"),
        series: nonempty(&fields, "series").map(|s| parse_series(&s)),
        isbn: nonempty(&fields, "isbn"),
    };
    if update.is_empty() {
        return Err(bad_request("nothing to save — fill in at least one field"));
    }

    let _permit = state
        .convert_slots
        .acquire()
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "server busy".into()))?;

    let result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<(MetaWriteResponse, Vec<u8>)> {
            let tmp = tempfile::Builder::new().prefix("epublift_web_").tempdir()?;
            let input = tmp.path().join(&file_name);
            std::fs::write(&input, &file_bytes)?;
            let output = tmp.path().join("output.epub");
            let md = epublift::write_metadata(&input, &update, &output)?;
            let bytes = std::fs::read(&output)?;
            let resp = MetaWriteResponse {
                output_name: format!("{}_meta.epub", file_stem_or(&file_name, "book")),
                metadata: metadata_json(&md),
                download_token: String::new(),
            };
            Ok((resp, bytes))
        })
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "crashed".into()))?
        .map_err(|e| bad_request(format!("could not save metadata: {e}")))?;

    let (mut resp, out_bytes) = result;
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
