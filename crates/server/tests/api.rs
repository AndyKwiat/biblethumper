//! HTTP API tests: the real router, driven in-process (no sockets), with the
//! deterministic HashEmbedder so they run without model files and in ~1s.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bt_core::embed::HashEmbedder;
use bt_core::{Bible, Engine};
use bt_server::{ChapterResponse, ErrorBody, Info, router};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn engine() -> Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        Arc::new(
            Engine::build(
                Bible::embedded().unwrap(),
                Arc::new(HashEmbedder::default()),
            )
            .unwrap(),
        )
    })
    .clone()
}

async fn get(uri: &str) -> (StatusCode, String, Vec<u8>) {
    let res = router(engine())
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, ct, body)
}

async fn get_json<T: serde::de::DeserializeOwned>(uri: &str) -> (StatusCode, T) {
    let (s, ct, b) = get(uri).await;
    assert!(
        ct.starts_with("application/json"),
        "{uri}: content-type {ct}"
    );
    (
        s,
        serde_json::from_slice(&b)
            .unwrap_or_else(|e| panic!("{uri}: {e}: {}", String::from_utf8_lossy(&b))),
    )
}

#[tokio::test]
async fn healthz() {
    let (s, _, b) = get("/healthz").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, b"ok");
}

#[tokio::test]
async fn serves_the_ui() {
    for (path, ct) in [
        ("/", "text/html"),
        ("/app.js", "text/javascript"),
        ("/lib.js", "text/javascript"),
        ("/style.css", "text/css"),
        ("/favicon.svg", "image/svg+xml"),
    ] {
        let (s, got, body) = get(path).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert!(got.starts_with(ct), "{path}: {got}");
        assert!(!body.is_empty());
    }
    let (_, _, html) = get("/").await;
    assert!(String::from_utf8(html).unwrap().contains("Bible"));
}

#[tokio::test]
async fn info_lists_books() {
    let (s, info): (_, Info) = get_json("/api/info").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(info.translation, "KJV");
    let books: Vec<_> = info
        .books
        .iter()
        .map(|b| (b.id.as_str(), b.chapters))
        .collect();
    assert_eq!(books, [("MAT", 28), ("MRK", 16), ("LUK", 24), ("JHN", 21)]);
    assert_eq!(info.embedder, "hash-384");
}

#[tokio::test]
async fn chapter_has_sections_with_all_verses() {
    let (s, c): (_, ChapterResponse) = get_json("/api/chapter/JHN/3").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((c.name.as_str(), c.chapter, c.chapters), ("John", 3, 21));
    let verses: Vec<u16> = c
        .sections
        .iter()
        .flat_map(|s| s.verses.iter().map(|v| v.verse))
        .collect();
    assert_eq!(verses, (1..=36).collect::<Vec<_>>());
    assert!(
        c.sections
            .iter()
            .all(|s| !s.summary.is_empty() && !s.title.is_empty())
    );
    assert_eq!(c.prev.unwrap().chapter, 2);
    assert_eq!(c.next.unwrap().chapter, 4);
}

#[tokio::test]
async fn chapter_navigation_crosses_books() {
    let (_, c): (_, ChapterResponse) = get_json("/api/chapter/MAT/28").await;
    let next = c.next.unwrap();
    assert_eq!((next.book.as_str(), next.chapter), ("MRK", 1));
    let (_, c): (_, ChapterResponse) = get_json("/api/chapter/MRK/1").await;
    let prev = c.prev.unwrap();
    assert_eq!((prev.book.as_str(), prev.chapter), ("MAT", 28));
    let (_, c): (_, ChapterResponse) = get_json("/api/chapter/MAT/1").await;
    assert!(c.prev.is_none());
    let (_, c): (_, ChapterResponse) = get_json("/api/chapter/JHN/21").await;
    assert!(c.next.is_none());
}

#[tokio::test]
async fn chapter_accepts_names_and_rejects_bad_input() {
    let (s, c): (_, ChapterResponse) = get_json("/api/chapter/luke/15").await;
    assert_eq!((s, c.book.as_str()), (StatusCode::OK, "LUK"));
    for uri in [
        "/api/chapter/GEN/1",
        "/api/chapter/MAT/0",
        "/api/chapter/MAT/29",
    ] {
        let (s, e): (_, ErrorBody) = get_json(uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert!(!e.error.is_empty());
    }
    let (s, _, _) = get("/api/chapter/MAT/abc").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn search_eye_of_the_needle() {
    let (s, r): (_, Value) = get_json("/api/search?q=eye%20of%20the%20needle").await;
    assert_eq!(s, StatusCode::OK);
    let labels: Vec<&str> = r["verses"]
        .as_array()
        .unwrap()
        .iter()
        .take(3)
        .map(|v| v["label"].as_str().unwrap())
        .collect();
    for want in ["Matthew 19:24", "Mark 10:25", "Luke 18:25"] {
        assert!(labels.contains(&want), "{labels:?}");
    }
    assert!(r["took_ms"].as_f64().unwrap() >= 0.0);
}

#[tokio::test]
async fn search_reference_and_filters() {
    let (_, r): (_, Value) = get_json("/api/search?q=John+3:16").await;
    assert_eq!(r["reference"]["label"], "John 3:16");
    assert_eq!(
        r["verses"][0]["text"]
            .as_str()
            .unwrap()
            .split(',')
            .next()
            .unwrap(),
        "For God so loved the world"
    );

    let (_, r): (_, Value) = get_json("/api/search?q=love&book=LUK&limit=3").await;
    let verses = r["verses"].as_array().unwrap();
    assert_eq!(verses.len(), 3);
    assert!(verses.iter().all(|v| v["book"] == "LUK"));
}

#[tokio::test]
async fn search_validation() {
    let (s, r): (_, Value) = get_json("/api/search").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(r["verses"].as_array().unwrap().len(), 0);
    let (s, _): (_, ErrorBody) = get_json("/api/search?q=x&book=GEN").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let long = "a".repeat(301);
    let (s, _): (_, ErrorBody) = get_json(&format!("/api/search?q={long}")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, r): (_, Value) = get_json("/api/search?q=love&limit=100000").await;
    assert!(r["verses"].as_array().unwrap().len() <= 100);
}

#[tokio::test]
async fn unknown_routes_are_json_404() {
    let (s, e): (_, ErrorBody) = get_json("/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(e.error, "not found");
}

#[tokio::test]
async fn responses_are_gzipped_when_asked() {
    let req = Request::get("/api/chapter/MAT/5")
        .header("accept-encoding", "gzip")
        .body(Body::empty())
        .unwrap();
    let res = router(engine()).oneshot(req).await.unwrap();
    assert_eq!(res.headers().get("content-encoding").unwrap(), "gzip");
}

// ---- abuse protection + security headers -------------------------------------

use bt_server::limits::LimitConfig;
use bt_server::router_with;
use std::time::Duration;

async fn status_from(
    app: &axum::Router,
    uri: &str,
    ip: &str,
) -> (StatusCode, axum::http::HeaderMap) {
    let req = Request::get(uri)
        .header("cf-connecting-ip", ip)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    (res.status(), res.headers().clone())
}

fn tight() -> LimitConfig {
    LimitConfig {
        search_burst: 3.0,
        search_per_min: 1.0,
        api_burst: 5.0,
        api_per_min: 1.0,
        ..Default::default()
    }
}

#[tokio::test]
async fn search_is_rate_limited_per_client() {
    let app = router_with(engine(), &tight());
    for _ in 0..3 {
        assert_eq!(
            status_from(&app, "/api/search?q=love", "1.1.1.1").await.0,
            StatusCode::OK
        );
    }
    let (s, h) = status_from(&app, "/api/search?q=love", "1.1.1.1").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        h.get("retry-after")
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    // a different client is unaffected
    assert_eq!(
        status_from(&app, "/api/search?q=love", "2.2.2.2").await.0,
        StatusCode::OK
    );
    // static files and health checks are never limited
    for _ in 0..10 {
        assert_eq!(status_from(&app, "/", "1.1.1.1").await.0, StatusCode::OK);
        assert_eq!(
            status_from(&app, "/healthz", "1.1.1.1").await.0,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn other_api_routes_have_their_own_limit() {
    let app = router_with(engine(), &tight());
    for _ in 0..5 {
        assert_eq!(
            status_from(&app, "/api/chapter/MAT/1", "3.3.3.3").await.0,
            StatusCode::OK
        );
    }
    assert_eq!(
        status_from(&app, "/api/info", "3.3.3.3").await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn global_search_budget_applies_across_clients() {
    let cfg = LimitConfig {
        global_search_burst: 4.0,
        global_search_per_sec: 0.01,
        ..Default::default()
    };
    let app = router_with(engine(), &cfg);
    for i in 0..4 {
        assert_eq!(
            status_from(&app, "/api/search?q=love", &format!("10.0.0.{i}"))
                .await
                .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        status_from(&app, "/api/search?q=love", "10.0.0.99").await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn search_returns_503_when_model_slots_are_busy() {
    let cfg = LimitConfig {
        max_concurrent_searches: 0,
        queue_wait: Duration::from_millis(20),
        ..Default::default()
    };
    let app = router_with(engine(), &cfg);
    let (s, _) = status_from(&app, "/api/search?q=love", "4.4.4.4").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn security_headers_on_every_response() {
    for uri in ["/", "/app.js", "/api/info", "/api/search?q=x", "/nope"] {
        let res = router(engine())
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let h = res.headers();
        let csp = h.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(
            csp.contains("script-src 'self'") && csp.contains("frame-ancestors 'none'"),
            "{uri}"
        );
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
    }
}

#[tokio::test]
async fn robots_txt_disallows_api() {
    let (s, ct, b) = get("/robots.txt").await;
    assert_eq!(s, StatusCode::OK);
    assert!(ct.starts_with("text/plain"));
    assert!(String::from_utf8(b).unwrap().contains("Disallow: /api/"));
}

#[tokio::test]
async fn error_messages_do_not_echo_long_input() {
    let long = "x".repeat(250);
    let (s, e): (_, ErrorBody) = get_json(&format!("/api/chapter/{long}/1")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(e.error.len() < 80, "{}", e.error);
}

#[tokio::test]
async fn hostile_queries_are_handled() {
    for q in [
        "%00%01%02",
        "%0Afake%20log%20line",
        "%3Cscript%3Ealert(1)%3C%2Fscript%3E",
        "%F0%9F%98%80%F0%9F%98%80",
        "'%20OR%201%3D1--",
        "...",
        "-",
        "john%2099999999999:1",
    ] {
        let (s, _, _) = get(&format!("/api/search?q={q}")).await;
        assert_eq!(s, StatusCode::OK, "{q}");
    }
    // exactly at the limit is fine; one more byte is rejected
    let (s, _, _) = get(&format!("/api/search?q={}", "a".repeat(300))).await;
    assert_eq!(s, StatusCode::OK);
}
