//! HTTP layer for Bible Thumper. `router()` is everything the server does;
//! `main.rs` only loads assets and binds the port, so tests drive `router()`
//! in-process with `tower::ServiceExt::oneshot`.
//!
//! Routes:
//!   GET /                        web UI (web/index.html, embedded)
//!   GET /app.js /lib.js /style.css
//!   GET /healthz                 "ok" (radify health check)
//!   GET /api/info                translation, books, embedder id
//!   GET /api/chapter/{book}/{n}  verses grouped into sections with summaries
//!   GET /api/search?q=&limit=&book=   hybrid search (see bt_core::search)
//!   GET /robots.txt              keeps polite crawlers out of /api
//!
//! Abuse protection (rate limits, max concurrent searches) lives in `limits.rs`
//! and wraps every /api route. Security headers are added to every response.

pub mod assets;
pub mod limits;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bt_core::model::Seg;
use bt_core::{Engine, SearchOptions};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::compression::CompressionLayer;

pub struct App {
    pub engine: Arc<Engine>,
    pub limits: limits::Limits,
}

pub type AppState = Arc<App>;

/// The app with production limits.
pub fn router(engine: Arc<Engine>) -> Router {
    router_with(engine, &limits::LimitConfig::default())
}

pub fn router_with(engine: Arc<Engine>, cfg: &limits::LimitConfig) -> Router {
    let state: AppState = Arc::new(App {
        engine,
        limits: limits::Limits::new(cfg),
    });
    let api = Router::new()
        .route("/api/info", get(info))
        .route("/api/chapter/{book}/{chapter}", get(chapter))
        .route("/api/search", get(search))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            limits::rate_limit,
        ));
    Router::new()
        .route("/", get(index))
        .route(
            "/app.js",
            get(|| static_file("text/javascript; charset=utf-8", APP_JS)),
        )
        .route(
            "/lib.js",
            get(|| static_file("text/javascript; charset=utf-8", LIB_JS)),
        )
        .route(
            "/style.css",
            get(|| static_file("text/css; charset=utf-8", STYLE_CSS)),
        )
        .route(
            "/favicon.svg",
            get(|| static_file("image/svg+xml", FAVICON)),
        )
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/robots.txt",
            get(|| {
                static_file(
                    "text/plain; charset=utf-8",
                    "User-agent: *\nDisallow: /api/\n",
                )
            }),
        )
        .merge(api)
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorBody {
                    error: "not found".into(),
                }),
            )
        })
        .layer(CompressionLayer::new())
        .layer(axum::middleware::map_response(security_headers))
        .with_state(state)
}

/// CSP allows only our own scripts plus Google Fonts; inline `style="--c:..."`
/// attributes are the one inline thing the UI uses.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' https://fonts.googleapis.com; \
style-src-attr 'unsafe-inline'; font-src https://fonts.gstatic.com; img-src 'self' data:; \
connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'";

async fn security_headers(mut res: Response) -> Response {
    let h = res.headers_mut();
    let set = |h: &mut axum::http::HeaderMap, k: &'static str, v: &'static str| {
        h.insert(k, axum::http::HeaderValue::from_static(v));
    };
    set(h, "content-security-policy", CSP);
    set(h, "x-content-type-options", "nosniff");
    set(h, "referrer-policy", "strict-origin-when-cross-origin");
    set(h, "x-frame-options", "DENY");
    set(
        h,
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    );
    res
}

/// Echo at most this much user input back in error messages.
fn clip(s: &str) -> String {
    s.chars().take(40).collect()
}

// ---- static UI (compiled in; edit files under web/) ------------------------

const INDEX_HTML: &str = include_str!("../../../web/index.html");
const APP_JS: &str = include_str!("../../../web/app.js");
const LIB_JS: &str = include_str!("../../../web/lib.js");
const STYLE_CSS: &str = include_str!("../../../web/style.css");
const FAVICON: &str = include_str!("../../../web/favicon.svg");

async fn index() -> Response {
    static_file("text/html; charset=utf-8", INDEX_HTML).await
}

async fn static_file(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

// ---- API --------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

fn error(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ErrorBody { error: msg.into() })).into_response()
}

#[derive(Serialize, Deserialize)]
pub struct BookInfo {
    pub id: String,
    pub name: String,
    pub chapters: usize,
}

#[derive(Serialize, Deserialize)]
pub struct Info {
    pub translation: String,
    pub embedder: String,
    pub books: Vec<BookInfo>,
}

async fn info(State(app): State<AppState>) -> Json<Info> {
    let e = &app.engine;
    Json(Info {
        translation: e.bible.translation.clone(),
        embedder: e.embedder_id(),
        books: e
            .bible
            .books
            .iter()
            .map(|b| BookInfo {
                id: b.id.clone(),
                name: b.name.clone(),
                chapters: b.chapters.len(),
            })
            .collect(),
    })
}

#[derive(Serialize, Deserialize)]
pub struct ChapterVerse {
    pub verse: u16,
    pub para: bool,
    pub segs: Vec<Seg>,
}

#[derive(Serialize, Deserialize)]
pub struct ChapterSection {
    pub title: String,
    pub summary: String,
    pub start: u16,
    pub end: u16,
    pub verses: Vec<ChapterVerse>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct ChapterRef {
    pub book: String,
    pub chapter: u16,
}

#[derive(Serialize, Deserialize)]
pub struct ChapterResponse {
    pub book: String,
    pub name: String,
    pub chapter: u16,
    pub chapters: usize,
    pub sections: Vec<ChapterSection>,
    pub prev: Option<ChapterRef>,
    pub next: Option<ChapterRef>,
}

async fn chapter(
    State(app): State<AppState>,
    Path((book, chapter)): Path<(String, u16)>,
) -> Response {
    let b = &app.engine.bible;
    let book = clip(&book);
    let Some(bi) = b
        .book_index(&book)
        .or_else(|| bt_core::reference::book_from_name(&book))
    else {
        return error(StatusCode::NOT_FOUND, format!("unknown book {book}"));
    };
    let Some(ch) = b.chapter(bi, chapter) else {
        return error(
            StatusCode::NOT_FOUND,
            format!("{book} has no chapter {chapter}"),
        );
    };
    let bk = &b.books[bi as usize];
    let sections = (ch.first_section..ch.first_section + ch.section_count as u32)
        .map(|sid| {
            let s = &b.sections[sid as usize];
            ChapterSection {
                title: s.title.clone(),
                summary: s.summary.clone(),
                start: s.start,
                end: s.end,
                verses: (s.first_verse..=s.first_verse + (s.end - s.start) as u32)
                    .map(|vid| {
                        let v = &b.verses[vid as usize];
                        ChapterVerse {
                            verse: v.verse,
                            para: v.para,
                            segs: v.segs.clone(),
                        }
                    })
                    .collect(),
            }
        })
        .collect();
    let at = |book: usize, chapter: usize| ChapterRef {
        book: b.books[book].id.clone(),
        chapter: chapter as u16,
    };
    let (bi, ci) = (bi as usize, chapter as usize);
    let prev = if ci > 1 {
        Some(at(bi, ci - 1))
    } else if bi > 0 {
        Some(at(bi - 1, b.books[bi - 1].chapters.len()))
    } else {
        None
    };
    let next = if ci < bk.chapters.len() {
        Some(at(bi, ci + 1))
    } else if bi + 1 < b.books.len() {
        Some(at(bi + 1, 1))
    } else {
        None
    };
    Json(ChapterResponse {
        book: bk.id.clone(),
        name: bk.name.clone(),
        chapter,
        chapters: bk.chapters.len(),
        sections,
        prev,
        next,
    })
    .into_response()
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    pub limit: Option<usize>,
    pub book: Option<String>,
}

/// Max query length in bytes. The model truncates at 256 tokens anyway.
pub const MAX_QUERY_BYTES: usize = 300;

async fn search(State(app): State<AppState>, Query(p): Query<SearchParams>) -> Response {
    let q = p.q.unwrap_or_default();
    if q.len() > MAX_QUERY_BYTES {
        return error(
            StatusCode::BAD_REQUEST,
            format!("query too long (max {MAX_QUERY_BYTES} chars)"),
        );
    }
    let book = match p.book.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(id) => match app.engine.bible.book_index(id) {
            Some(i) => Some(i),
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    format!("unknown book {}", clip(id)),
                );
            }
        },
    };
    let opts = SearchOptions {
        limit: p.limit.unwrap_or(20).clamp(1, 100),
        section_limit: 5,
        book,
    };
    // At most `max_concurrent_searches` model runs at once; others queue briefly, then 503.
    let permit = match tokio::time::timeout(
        app.limits.queue_wait,
        app.limits.inference.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(p)) => p,
        _ => {
            tracing::warn!("search queue full");
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Busy right now, try again in a moment.",
            );
        }
    };
    // Embedding the query is CPU work (~25 ms); keep it off the async workers.
    let engine = app.engine.clone();
    let res = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        engine.search(&q, &opts)
    })
    .await;
    match res {
        Ok(Ok(r)) => {
            // Debug formatting escapes newlines/control chars (no log injection).
            let shown: String = r.query.chars().take(80).collect();
            tracing::info!(query = ?shown, ms = r.took_ms, hits = r.verses.len(), "search");
            Json(r).into_response()
        }
        Ok(Err(err)) => {
            tracing::error!("search failed: {err:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "search failed")
        }
        Err(err) => {
            tracing::error!("search task panicked: {err}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "search failed")
        }
    }
}
