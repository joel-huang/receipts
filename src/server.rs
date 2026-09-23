use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::json;

use crate::index;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

struct AppState {
    db_path: PathBuf,
    /// Hostnames besides loopback that the server answers, in lowercase.
    allowed_hosts: Vec<String>,
    conn: Mutex<Connection>,
    indexing: AtomicBool,
    /// Counts scans that changed data. The web app reloads its views when this number changes.
    generation: AtomicU64,
    /// Unix time in milliseconds when the last scan finished. 0 means no scan has finished yet.
    last_indexed_at: AtomicU64,
}

type Shared = Arc<AppState>;

struct ApiError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": self.0.to_string() }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

pub async fn serve(
    db_path: PathBuf,
    host: String,
    port: u16,
    open_browser: bool,
    allowed_hosts: Vec<String>,
) -> anyhow::Result<()> {
    let allowed_hosts: Vec<String> = allowed_hosts
        .iter()
        .map(|h| h.trim().trim_end_matches('.').to_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    let state: Shared = Arc::new(AppState {
        allowed_hosts,
        conn: Mutex::new(index::open(&db_path)?),
        db_path,
        indexing: AtomicBool::new(false),
        generation: AtomicU64::new(0),
        last_indexed_at: AtomicU64::new(0),
    });

    // Serve immediately; index in the background so the UI fills in as it goes.
    spawn_reindex(state.clone(), true);

    let extra_hosts = state.allowed_hosts.clone();
    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/sessions/{id}/outline", get(get_outline))
        .route("/api/sessions/{id}/messages/{idx}", get(get_message))
        .route("/api/facets", get(facets))
        .route("/api/search", get(search))
        .route("/api/reindex", post(reindex))
        .fallback(static_asset)
        // Chat logs compress well, which matters over a slow link such as `receipts remote`.
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(middleware::from_fn_with_state(state.clone(), allowed_host_only))
        .with_state(state);

    let listener = bind(&host, port).await?;
    let url = format!("http://{}", listener.local_addr()?);
    println!("Receipts running at {url}  (Ctrl+C to stop)");
    for h in &extra_hosts {
        println!("Also answering requests for {h}");
    }
    if open_browser {
        let _ = open::that(&url);
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// Try the requested port, then the next few, so a second instance still starts.
pub(crate) async fn bind(host: &str, port: u16) -> anyhow::Result<tokio::net::TcpListener> {
    let mut last_err = None;
    for p in port..port.saturating_add(20) {
        match tokio::net::TcpListener::bind((host, p)).await {
            Ok(l) => return Ok(l),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.map(Into::into).unwrap_or_else(|| anyhow::anyhow!("no port available")))
}

/// Reject requests whose Host header is neither loopback nor an allowed hostname. This blocks
/// DNS-rebinding attacks, where a malicious web page resolves its own hostname to 127.0.0.1 to
/// read your chats. `--allow-host` adds names such as a Tailscale name, which such a page cannot
/// make the browser send.
async fn allowed_host_only(State(s): State<Shared>, req: Request, next: Next) -> Response {
    match host_rejection(&req, &s.allowed_hosts) {
        None => next.run(req).await,
        Some(resp) => resp,
    }
}

/// Returns a 403 response unless the Host header is loopback or one of `allowed`.
pub(crate) fn host_rejection(req: &Request, allowed: &[String]) -> Option<Response> {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let hostname = if host.starts_with('[') {
        host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or("").to_string()
    };
    let hostname = hostname.trim_end_matches('.').to_lowercase();
    if matches!(hostname.as_str(), "localhost" | "127.0.0.1" | "[::1]") || allowed.contains(&hostname) {
        None
    } else {
        let msg = format!("Receipts does not answer requests for {hostname}. Start it with --allow-host {hostname} to allow them.");
        Some((StatusCode::FORBIDDEN, msg).into_response())
    }
}

/// Scans the agent logs in the background. Only the scan at startup prints its count, because the
/// web app requests a scan every few seconds and an active chat changes between them.
fn spawn_reindex(state: Shared, report: bool) -> bool {
    if state.indexing.swap(true, Ordering::SeqCst) {
        return false;
    }
    tokio::task::spawn_blocking(move || {
        let result = index::open(&state.db_path).and_then(|mut c| index::reindex(&mut c));
        match result {
            Ok(r) => {
                if r.updated > 0 {
                    state.generation.fetch_add(1, Ordering::SeqCst);
                }
                if report {
                    eprintln!("receipts: indexed {} of {} sessions", r.updated, r.scanned);
                }
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                state.last_indexed_at.store(now, Ordering::SeqCst);
            }
            Err(e) => eprintln!("receipts: indexing failed: {e}"),
        }
        state.indexing.store(false, Ordering::SeqCst);
    });
    true
}

async fn status(State(s): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let conn = s.conn.lock().unwrap();
    let sessions: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "indexing": s.indexing.load(Ordering::SeqCst),
        "generation": s.generation.load(Ordering::SeqCst),
        "last_indexed_at": match s.last_indexed_at.load(Ordering::SeqCst) {
            0 => None,
            ms => Some(ms),
        },
        "sessions": sessions,
        "db_path": s.db_path,
    })))
}

async fn reindex(State(s): State<Shared>) -> Json<serde_json::Value> {
    let started = spawn_reindex(s, false);
    Json(json!({ "started": started }))
}

#[derive(Deserialize)]
struct ListParams {
    source: Option<String>,
    project: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_sessions(State(s): State<Shared>, Query(p): Query<ListParams>) -> ApiResult<Response> {
    let conn = s.conn.lock().unwrap();
    let rows = index::list_sessions(
        &conn,
        &index::ListFilter {
            source: p.source.as_deref().filter(|v| !v.is_empty()),
            project: p.project.as_deref().filter(|v| !v.is_empty()),
            limit: p.limit.unwrap_or(100).clamp(1, 1000),
            offset: p.offset.unwrap_or(0).max(0),
        },
    )?;
    Ok(Json(rows).into_response())
}

#[derive(Deserialize)]
struct SessionParams {
    /// Return only messages from this position on.
    from: Option<i64>,
    /// Return at most this many messages. The web app loads long chats in pages.
    limit: Option<i64>,
}

async fn get_session(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(p): Query<SessionParams>,
) -> ApiResult<Response> {
    let conn = s.conn.lock().unwrap();
    let limit = p.limit.unwrap_or(i64::MAX).max(1);
    Ok(match index::get_session(&conn, &id, p.from.unwrap_or(0), limit)? {
        Some(page) => Json(page).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
    })
}

async fn get_outline(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(p): Query<SessionParams>,
) -> ApiResult<Response> {
    let conn = s.conn.lock().unwrap();
    Ok(match index::get_outline(&conn, &id, p.from.unwrap_or(0))? {
        Some(outline) => Json(outline).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
    })
}

async fn get_message(State(s): State<Shared>, Path((id, idx)): Path<(String, i64)>) -> ApiResult<Response> {
    let conn = s.conn.lock().unwrap();
    Ok(match index::get_message(&conn, &id, idx)? {
        Some(m) => Json(m).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
    })
}

async fn facets(State(s): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let conn = s.conn.lock().unwrap();
    Ok(Json(json!({
        "sources": index::facets(&conn, "source")?,
        "projects": index::facets(&conn, "project")?,
    })))
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    source: Option<String>,
    limit: Option<i64>,
}

async fn search(State(s): State<Shared>, Query(p): Query<SearchParams>) -> ApiResult<Response> {
    let conn = s.conn.lock().unwrap();
    let hits = index::search(
        &conn,
        &p.q,
        p.source.as_deref().filter(|v| !v.is_empty()),
        p.limit.unwrap_or(100).clamp(1, 500),
    )?;
    Ok(Json(hits).into_response())
}

pub(crate) async fn static_asset(req: Request) -> Response {
    // An unknown API path is an error, not a page. The web app then knows that the server is older.
    if req.uri().path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    let path = req.uri().path().trim_start_matches('/');
    // Unknown non-asset paths fall back to index.html (client-side routing).
    let (file, name) = match Assets::get(path) {
        Some(f) if !path.is_empty() => (f, path),
        _ => match Assets::get("index.html") {
            Some(f) => (f, "index.html"),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let cache = if name.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}
