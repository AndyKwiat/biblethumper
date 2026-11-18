//! Speed guardrails for the parts that don't depend on the model: building the
//! indexes and the keyword + vector search path. Generous limits so they only
//! fail on real regressions (e.g. accidentally quadratic code). Run with
//! `--release` for meaningful numbers; limits assume a debug build of this crate
//! with optimised dependencies (see [profile.test] in Cargo.toml).

use bt_core::embed::HashEmbedder;
use bt_core::{Bible, Engine, SearchOptions};
use std::sync::Arc;
use std::time::Instant;

#[test]
fn startup_and_search_are_fast() {
    let t = Instant::now();
    let bible = Bible::embedded().unwrap();
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let engine = Engine::build(bible, Arc::new(HashEmbedder::default())).unwrap();
    let build_ms = t.elapsed().as_secs_f64() * 1000.0;

    let queries = [
        "eye of the needle",
        "love",
        "the kingdom of heaven is like",
        "John 3:16",
        "a",
        "verily verily I say unto you",
    ];
    let mut times = Vec::new();
    for _ in 0..20 {
        for q in queries {
            let t = Instant::now();
            engine.search(q, &SearchOptions::default()).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    times.sort_by(f64::total_cmp);
    let p50 = times[times.len() / 2];
    let p99 = times[times.len() * 99 / 100];
    eprintln!(
        "parse {load_ms:.1} ms, build {build_ms:.1} ms, search p50 {p50:.3} ms p99 {p99:.3} ms"
    );
    assert!(load_ms < 1500.0, "parsing data took {load_ms} ms");
    assert!(p50 < 25.0, "search p50 {p50} ms");
    assert!(p99 < 100.0, "search p99 {p99} ms");
}
