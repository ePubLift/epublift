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
use epublift::{EpubVersion, Options};
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

/// A converted EPUB held in memory awaiting download.
struct Pending {
    name: String,
    bytes: Vec<u8>,
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
        .route("/healthz", get(|| async { "ok" }))
        .route("/convert", post(convert))
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
    .await
    .unwrap();
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

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name = String::from("book.epub");
    let mut quality: u8 = 80;
    let mut ascii = false;
    let mut kepub = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(format!("malformed upload: {e}")))?
    {
        match field.name().unwrap_or_default() {
            "file" => {
                // Keep only the basename of the (untrusted) filename.
                if let Some(name) = field.file_name()
                    && let Some(base) = Path::new(name).file_name()
                {
                    file_name = base.to_string_lossy().into_owned();
                }
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| bad_request(format!("could not read file: {e}")))?;
                file_bytes = Some(bytes.to_vec());
            }
            "quality" => {
                let v = field.text().await.unwrap_or_default();
                quality = v.trim().parse().unwrap_or(80);
            }
            "ascii" => {
                let v = field.text().await.unwrap_or_default();
                ascii = matches!(v.trim(), "true" | "on" | "1");
            }
            "kepub" => {
                let v = field.text().await.unwrap_or_default();
                kepub = matches!(v.trim(), "true" | "on" | "1");
            }
            _ => { /* ignore unknown fields */ }
        }
    }

    let file_bytes = file_bytes.ok_or_else(|| bad_request("no file was uploaded"))?;
    if file_bytes.is_empty() {
        return Err(bad_request("the uploaded file is empty"));
    }

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
                target_version: EpubVersion::LATEST,
                kepub,
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
            born: Instant::now(),
        },
    );
    resp.download_token = token;

    Ok(Json(resp))
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
                    (header::CONTENT_TYPE, "application/epub+zip".to_string()),
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
