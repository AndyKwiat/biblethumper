# Bible Thumper 👍📖

Search and read Matthew, Mark, Luke and John (King James Version) with a
plain-English note beside every passage.

- **Search by words or meaning.** "eye of the needle" finds Matthew 19:24,
  Mark 10:25 and Luke 18:25; "the prodigal son" finds Luke 15 even though the
  KJV never uses the word. References work too: `John 3:16`, `mk 4`, `Luke 15:11-32`.
- **Read with notes.** Each chapter is split into passages; a "Coles Notes"
  column explains each one in plain English (stacks above the text on phones).
  Words of Jesus are in red.
- **Fast and small.** Rust (axum) server, one static binary. Hybrid search =
  BM25 keywords + all-MiniLM-L6-v2 sentence embeddings (run on CPU with candle),
  fused with reciprocal-rank fusion. ~25 ms for a new query, ~2 ms cached,
  ~110 MB RAM, 0.4 s startup. Rate-limited so bots can't swamp the model.

Live: https://biblethumper.akwiat.com

## Quick start

```sh
scripts/fetch_model.sh                          # model -> assets/model (45 MB, f16)
cargo run --release --bin bt -- build-index     # embeddings -> assets/embeddings.bin
cargo run --release --bin app                   # http://127.0.0.1:8080
scripts/test.sh                                 # all tests
./deploy.sh                                     # deploy (uses radify, my personal deploy tool)
```

Developer and agent guide (layout, tests, how-tos): [AGENTS.md](AGENTS.md).

## Engineering notes

- **Workspace:** `crates/core` is the search library (reference parser, BM25 index, vector index, RRF
  fusion) with no HTTP; `crates/server` is the axum app plus a `bt` CLI for building and benchmarking the
  index. The web UI (no build step) is compiled into the binary.
- **Tests:** Rust unit tests in every core module, in-process HTTP tests, performance guardrails,
  golden search-quality queries against the real model, frontend unit tests (`bun test`), a smoke test
  of the release binary, plus `fmt` and `clippy -D warnings`. `scripts/test.sh` runs them all.
- **Built to be public:** per-IP and global token-bucket rate limits, a cap on concurrent model runs,
  query length limits, strict CSP and security headers, no internal errors returned to clients.
  Load-tested at 100 concurrent clients across 300 IPs.

## Sources

- KJV text: eBible.org `eng-kjv_usfm` (public domain), imported by `tools/import_kjv.py`.
- Embedding model: sentence-transformers/all-MiniLM-L6-v2 (Apache-2.0).
- Passage titles and summaries: written by Claude from the KJV text; not scripture.
