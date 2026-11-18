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

pub mod assets;

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

pub type AppState = Arc<Engine>;

pub fn router(engine: AppState) -> Router {
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
        .route("/api/info", get(info))
        .route("/api/chapter/{book}/{chapter}", get(chapter))
        .route("/api/search", get(search))
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorBody {
                    error: "not found".into(),
                }),
            )
        })
        .layer(CompressionLayer::new())
        .with_state(engine)
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

async fn info(State(e): State<AppState>) -> Json<Info> {
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
    State(e): State<AppState>,
    Path((book, chapter)): Path<(String, u16)>,
) -> Response {
    let b = &e.bible;
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

async fn search(State(e): State<AppState>, Query(p): Query<SearchParams>) -> Response {
    let q = p.q.unwrap_or_default();
    if q.len() > 300 {
        return error(StatusCode::BAD_REQUEST, "query too long (max 300 chars)");
    }
    let book = match p.book.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(id) => match e.bible.book_index(id) {
            Some(i) => Some(i),
            None => return error(StatusCode::BAD_REQUEST, format!("unknown book {id}")),
        },
    };
    let opts = SearchOptions {
        limit: p.limit.unwrap_or(20).clamp(1, 100),
        section_limit: 5,
        book,
    };
    // Embedding the query is CPU work (a few ms); keep it off the async workers.
    let engine = e.clone();
    match tokio::task::spawn_blocking(move || engine.search(&q, &opts)).await {
        Ok(Ok(r)) => {
            tracing::info!(query = %r.query, ms = r.took_ms, hits = r.verses.len(), "search");
            Json(r).into_response()
        }
        Ok(Err(err)) => error(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}
