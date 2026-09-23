use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
    conn: Mutex<Connection>,
    indexing: AtomicBool,
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

pub async fn serve(db_path: PathBuf, host: String, port: u16, open_browser: bool) -> anyhow::Result<()> {
    let state: Shared = Arc::new(AppState {
        conn: Mutex::new(index::open(&db_path)?),
        db_path,
        indexing: AtomicBool::new(false),
    });

    // Serve immediately; index in the background so the UI fills in as it goes.
    spawn_reindex(state.clone());

    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/facets", get(facets))
        .route("/api/search", get(search))
        .route("/api/reindex", post(reindex))
        .fallback(static_asset)
        .layer(middleware::from_fn(local_host_only))
        .with_state(state);

    let listener = bind(&host, port).await?;
    let url = format!("http://{}", listener.local_addr()?);
    println!("Receipts running at {url}  (Ctrl+C to stop)");
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
async fn bind(host: &str, port: u16) -> anyhow::Result<tokio::net::TcpListener> {
    let mut last_err = None;
    for p in port..port.saturating_add(20) {
        match tokio::net::TcpListener::bind((host, p)).await {
            Ok(l) => return Ok(l),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.map(Into::into).unwrap_or_else(|| anyhow::anyhow!("no port available")))
}

/// Reject requests whose Host header isn't loopback. Blocks DNS-rebinding attacks
/// where a malicious web page resolves its own hostname to 127.0.0.1 to read your chats.
async fn local_host_only(req: Request, next: Next) -> Response {
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
    if matches!(hostname.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "Receipts only serves localhost").into_response()
    }
}

fn spawn_reindex(state: Shared) -> bool {
    if state.indexing.swap(true, Ordering::SeqCst) {
        return false;
    }
    tokio::task::spawn_blocking(move || {
        let result = index::open(&state.db_path).and_then(|mut c| index::reindex(&mut c));
        match result {
            Ok(r) if r.updated > 0 => eprintln!("receipts: indexed {} of {} sessions", r.updated, r.scanned),
            Ok(_) => {}
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
        "sessions": sessions,
        "db_path": s.db_path,
    })))
}

async fn reindex(State(s): State<Shared>) -> Json<serde_json::Value> {
    let started = spawn_reindex(s);
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

async fn get_session(State(s): State<Shared>, Path(id): Path<String>) -> ApiResult<Response> {
    let conn = s.conn.lock().unwrap();
    Ok(match index::get_session(&conn, &id)? {
        Some((session, messages)) => Json(json!({ "session": session, "messages": messages })).into_response(),
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

async fn static_asset(req: Request) -> Response {
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
