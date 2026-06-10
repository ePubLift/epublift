// epublift-web - Browser front-end for epublift: upload an EPUB, convert it,
// download the result. Part of epublift; licensed under the GNU AGPL-3.0-or-later.
//
// The conversion itself is performed by the `epublift` library `convert()` API.
// This crate is only a thin, hardened HTTP/multipart wrapper around it.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use base64::Engine;
use epublift::{EpubVersion, Options};
use serde::Serialize;
use tokio::sync::Semaphore;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

/// Hard limits for the public endpoint. The source is open, so these are the
/// real defense — not obscurity.
const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024; // 50 MiB
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared service state.
struct AppState {
    /// Caps simultaneous (CPU-heavy) conversions so latency stays predictable.
    convert_slots: Semaphore,
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
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/healthz", get(|| async { "ok" }))
        .route("/convert", post(convert))
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
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("epublift-web listening on http://{addr}  ({slots} convert slots)");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Serve the single-page front-end.
async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
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
    /// The converted EPUB, base64-encoded. The client rebuilds it as a Blob and
    /// offers it for download; nothing is ever stored server-side.
    file_base64: String,
}

/// A user-facing error with an HTTP status.
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

/// Convert an uploaded EPUB and return the result + report as JSON.
async fn convert(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<ConvertResponse>, ApiError> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name = String::from("book.epub");
    let mut quality: u8 = 80;
    let mut ascii = false;

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
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<ConvertResponse> {
        let tmp = tempfile::Builder::new().prefix("epublift_web_").tempdir()?;
        let input_path = tmp.path().join(&file_name);
        std::fs::write(&input_path, &file_bytes)?;

        let opts = Options {
            quality,
            ascii,
            target_version: EpubVersion::LATEST,
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
            file_base64: base64::engine::general_purpose::STANDARD.encode(&out_bytes),
        };
        // `tmp` drops here -> temp dir removed.
        Ok(resp)
    })
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "conversion crashed".into(),
        )
    })?;

    match result {
        Ok(resp) => Ok(Json(resp)),
        // Most failures here are invalid/corrupt EPUBs -> client error.
        Err(e) => Err(bad_request(format!("could not convert this EPUB: {e}"))),
    }
}
