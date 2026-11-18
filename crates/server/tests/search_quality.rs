//! Golden-query tests against the REAL embedding model.
//!
//! These check that search finds what a person would expect. They need
//! `assets/model` (run `scripts/fetch_model.sh`) and are skipped with a
//! message otherwise. Run: `cargo test --release -p bt-server --test search_quality`.
//!
//! When tuning search (bt_core::search constants), add queries here first.

use bt_core::{Bible, Engine, SearchOptions};
use std::path::Path;
use std::sync::{Arc, OnceLock};

fn engine() -> Option<&'static Engine> {
    static E: OnceLock<Option<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        if !assets.join("model/model.safetensors").is_file() {
            eprintln!(
                "SKIPPED: no model in {} (run scripts/fetch_model.sh)",
                assets.display()
            );
            return None;
        }
        let bible = Bible::embedded().unwrap();
        let embedder = bt_server::assets::load_embedder(&assets).unwrap();
        assert!(
            embedder.id().contains("MiniLM"),
            "expected the real model, got {}",
            embedder.id()
        );
        let emb = bt_server::assets::load_or_build_embeddings(&bible, embedder.as_ref(), &assets)
            .unwrap();
        Some(Engine::new(bible, Arc::clone(&embedder), emb).unwrap())
    })
    .as_ref()
}

/// (query, verse(s) that must appear in the top `n` verses; "A|B" = either is fine)
const VERSE_CASES: &[(&str, &str, usize)] = &[
    ("eye of the needle", "Matthew 19:24", 3),
    ("eye of the needle", "Mark 10:25", 3),
    ("eye of the needle", "Luke 18:25", 3),
    ("camel needle rich man", "Matthew 19:24", 3),
    ("for god so loved the world", "John 3:16", 1),
    ("love your enemies", "Matthew 5:44", 3),
    ("turn the other cheek", "Matthew 5:39", 5),
    ("Jesus wept", "John 11:35", 1),
    ("I am the way the truth and the life", "John 14:6", 1),
    ("I am the bread of life", "John 6:35", 3),
    ("blessed are the meek", "Matthew 5:5", 1),
    ("the Lord's prayer our father in heaven", "Matthew 6:9", 5),
    ("faith as small as a mustard seed", "Matthew 17:20", 5),
    (
        "render unto caesar",
        "Matthew 22:21|Mark 12:17|Luke 20:25",
        3,
    ),
    (
        "do unto others as you would have them do to you",
        "Luke 6:31",
        5,
    ),
    (
        "let him who is without sin cast the first stone",
        "John 8:7",
        3,
    ),
    ("in the beginning was the word", "John 1:1", 1),
];

/// (query, "BOOK CH|BOOK CH") — a section from one of those chapters must be in the top 3 passages.
const SECTION_CASES: &[(&str, &str)] = &[
    ("the prodigal son", "LUK 15"),
    ("Jesus walks on water", "MAT 14|MRK 6|JHN 6"),
    ("feeding of the five thousand", "JHN 6|MAT 14|MRK 6|LUK 9"),
    ("Lazarus raised from the dead", "JHN 11"),
    ("the good samaritan", "LUK 10"),
    ("Jesus calms a storm", "MRK 4|MAT 8|LUK 8"),
    ("water turned into wine at a wedding", "JHN 2"),
    ("the sermon on the mount", "MAT 5"),
    (
        "Peter denies Jesus three times",
        "LUK 22|MAT 26|MRK 14|JHN 18",
    ),
    ("the birth of Jesus in Bethlehem", "LUK 2"),
];

#[test]
fn golden_verses() {
    let Some(e) = engine() else { return };
    let mut failures = Vec::new();
    for (q, want, n) in VERSE_CASES {
        let r = e.search(q, &SearchOptions::default()).unwrap();
        let top: Vec<_> = r.verses.iter().take(*n).map(|v| v.label.clone()).collect();
        if !top.iter().any(|l| want.split('|').any(|w| w == l)) {
            failures.push(format!("{q:?}: want {want} in top {n}, got {top:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn golden_sections() {
    let Some(e) = engine() else { return };
    let mut failures = Vec::new();
    for (q, want) in SECTION_CASES {
        let r = e.search(q, &SearchOptions::default()).unwrap();
        let top: Vec<_> = r
            .sections
            .iter()
            .take(3)
            .map(|s| format!("{} {}", s.book, s.chapter))
            .collect();
        if !top.iter().any(|t| want.split('|').any(|w| w == t)) {
            let titles: Vec<_> = r
                .sections
                .iter()
                .take(3)
                .map(|s| format!("{} ({})", s.label, s.title))
                .collect();
            failures.push(format!(
                "{q:?}: want {want} in top 3 passages, got {titles:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn real_model_query_speed() {
    let Some(e) = engine() else { return };
    // Warm up, then time uncached queries (each one runs the model).
    e.search("warm up", &SearchOptions::default()).unwrap();
    let mut times = Vec::new();
    for (i, (q, _, _)) in VERSE_CASES.iter().enumerate() {
        let t = std::time::Instant::now();
        e.search(&format!("{q} {i}"), &SearchOptions::default())
            .unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let p50 = times[times.len() / 2];
    eprintln!(
        "uncached query p50 {p50:.1} ms, max {:.1} ms",
        times.last().unwrap()
    );
    assert!(p50 < 150.0, "search too slow: p50 {p50:.1} ms");
}
