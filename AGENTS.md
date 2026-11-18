# AGENTS.md — working on Bible Thumper

Read this before changing anything. `CLAUDE.md` points here.

## What it is

A Rust web app to search and read the four Gospels (KJV) with a plain-English
"Coles Notes" column beside every passage. Deployed with radify at
https://biblethumper.akwiat.com (allowlisted to Andy).

## Map

```
data/gospels.json        KJV text: books > chapters > verses {text, para, segs[{t, wj, add}]}
data/sections.json       452 passages {book, chapter, start, end, title, summary} (the notes)
tools/import_kjv.py      regenerates gospels.json from eBible.org USFM (stdlib only)
tools/check_sections.py  validates sections.json (coverage, no gaps/overlaps, lengths)
crates/core/             bt-core library, no HTTP
  src/model.rs           Bible/Book/Chapter/Verse/Section; data is include_str!'d at compile time
  src/reference.rs       "John 3:16" / "mk 4" / "Luke 15:11-32" parser + book aliases
  src/text.rs            tokenizer, tiny stemmer (folds KJV -eth/-est), stopwords
  src/keyword.rs         BM25 index with phrase/proximity bonus
  src/vector.rs          flat cosine index, embeddings.bin format + fingerprint
  src/embed.rs           Embedder trait; MiniLm (candle, feature "model"); HashEmbedder (tests)
  src/search.rs          Engine: reference lookup + hybrid RRF fusion. Tuning constants at top.
crates/server/           bt-server: axum app
  src/lib.rs             router() + all handlers (routes listed at top of file)
  src/assets.rs          finds assets/, loads model + embeddings (rebuilds if stale)
  src/main.rs            binary `app` (radify entry point)
  src/bin/bt.rs          CLI `bt`: build-index | check-index | quantize | search | bench
  tests/api.rs           HTTP tests, in-process, HashEmbedder (no model needed)
  tests/perf.rs          speed guardrails (index build + search latency)
  tests/search_quality.rs golden queries against the REAL model (skips without it)
web/                     UI, no build step; compiled into the binary with include_str!
  index.html style.css   layout + casual/bright theme (CSS tokens on :root)
  app.js                 hash router + views (home, read, search)
  lib.js + lib.test.js   pure helpers, tested with `bun test web/`
scripts/                 fetch_model.sh, build.sh, test.sh, smoke.sh
deploy.sh                build + radify deploy + health
assets/ (gitignored)     model/ (MiniLM f16) + embeddings.bin, produced by scripts
```

## Commands

```sh
. ~/.cargo/env                      # rustup lives in ~/.cargo
scripts/fetch_model.sh              # once: download + f16-convert all-MiniLM-L6-v2 into assets/model
cargo run --release --bin bt -- build-index      # embed 3,779 verses + 452 sections (~90 s, 2 cores)
cargo run --release --bin app                    # http://127.0.0.1:8080 (PORT/HOST env)
cargo run --release --bin bt -- search "eye of the needle"
cargo run --release --bin bt -- bench
scripts/test.sh fast                # everything except build+smoke
scripts/test.sh                     # everything (default before deploying)
./deploy.sh                         # musl build -> dist/ -> radify deploy biblethumper dist
```

Always build and test with `--release`: candle is unusably slow unoptimised, and
release artifacts are reused between `cargo build` and `cargo test` (a debug
build recompiles everything, ~8 min on this box).

## Tests (kinds)

| kind | where | needs model | command |
|---|---|---|---|
| data validation | tools/check_sections.py, model.rs tests | no | `python3 -I tools/check_sections.py data/sections.json` |
| Rust unit | `#[cfg(test)]` in every core module | no | `cargo test --release -p bt-core` |
| HTTP/API | crates/server/tests/api.rs | no | `cargo test --release -p bt-server --test api` |
| performance | crates/server/tests/perf.rs, `bt bench` | no / yes | `cargo test --release --test perf` |
| search quality (golden) | crates/server/tests/search_quality.rs | yes | `cargo test --release --test search_quality` |
| frontend unit | web/lib.test.js | no | `bun test web/` |
| smoke (real binary) | scripts/smoke.sh | yes | `scripts/build.sh && scripts/smoke.sh` |
| lint | fmt + clippy -D warnings | no | part of `scripts/test.sh` |

## How-tos

**Change a summary/title.** Edit `data/sections.json`, run `tools/check_sections.py`,
then `bt build-index` (the embeddings fingerprint covers every summary, so a
stale index is detected: `bt check-index`; the server would also rebuild at
startup, slowly). Name well-known passages in summaries ("the Sermon on the
Mount", "the Prodigal Son") — semantic search for those names relies on it.

**Tune search.** Add the query you care about to `tests/search_quality.rs` first,
then adjust the constants at the top of `crates/core/src/search.rs`
(RRF weights, phrase boost, MIN_COSINE). Run the quality + api tests.

**Add books (e.g. Acts, or the whole Bible).** Extend `BOOKS` in
`tools/import_kjv.py` and rerun it; add aliases in `reference.rs`
(`BOOK_ALIASES`, book index = position in gospels.json); write sections for the
new chapters (same JSON shape; validate with check_sections.py); add a colour in
`web/lib.js` BOOK_COLORS and a blurb in `web/app.js`; rebuild the index. For the
whole Bible (~31k verses) the flat index is still fine (~12M mult-adds/query),
embeddings.bin grows to ~50 MB and build-index takes ~15 min here.

**Add an API route.** Handler + route in `crates/server/src/lib.rs` (keep the
route list in the module doc current), test in `tests/api.rs`.

**Change the UI.** Edit `web/*`; pure logic goes in `lib.js` with a test in
`lib.test.js`. Rebuild the binary (files are compiled in).

## Gotchas

- Musl static build (`x86_64-unknown-linux-musl`) because step1's glibc may be older
  than step2's. Keep deps pure Rust: `tokenizers` uses `fancy-regex`, not `onig`.
- The radify app has a 512 MB cap; the server uses ~110 MB RSS (model ~90 MB as f32).
- Deploy upload limit is 100 MB gzipped; dist/ is ~60 MB (f16 model 45 MB).
- `HashEmbedder` is NOT semantic. If `/api/info` says `hash-384` in production,
  the model files are missing from `dist/assets/model`.
- Summaries were written by an AI (Claude) from the KJV text; the footer says so.
