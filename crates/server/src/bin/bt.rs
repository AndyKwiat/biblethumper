//! Developer/ops CLI.
//!
//!   bt build-index [OUT]        embed all verses + sections -> assets/embeddings.bin
//!   bt check-index              exit 0 if assets/embeddings.bin matches data + model
//!   bt quantize IN OUT          convert model.safetensors to f16
//!   bt search "query" [N]       run a search and print results
//!   bt bench [ROUNDS]           time a fixed query mix (p50/p95/max)

use anyhow::{Context, Result, bail};
use bt_core::{Bible, Engine, SearchOptions};
use bt_server::assets;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build-index") => build_index(args.get(1)),
        Some("check-index") => check_index(),
        Some("quantize") => quantize(&args[1..]),
        Some("search") => search(
            args.get(1).context("usage: bt search QUERY [N]")?,
            args.get(2),
        ),
        Some("bench") => bench(args.get(1)),
        _ => {
            eprintln!(
                "usage: bt build-index [OUT] | check-index | quantize IN OUT | search QUERY [N] | bench [ROUNDS]"
            );
            std::process::exit(2);
        }
    }
}

fn build_index(out: Option<&String>) -> Result<()> {
    let dir = assets::assets_dir();
    let out = out
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("embeddings.bin"));
    let bible = Bible::embedded()?;
    let embedder = assets::load_embedder(&dir)?;
    let t = Instant::now();
    let emb = bt_core::search::compute_embeddings(&bible, embedder.as_ref(), |done, total| {
        if done % 512 < 32 || done == total {
            eprint!("\rembedding {done}/{total}");
        }
    })?;
    eprintln!();
    assets::write_embeddings(&emb, &out)?;
    println!(
        "wrote {} ({} verses, {} sections, {}) in {:.1?}",
        out.display(),
        emb.verses.len(),
        emb.sections.len(),
        embedder.id(),
        t.elapsed()
    );
    Ok(())
}

fn check_index() -> Result<()> {
    let dir = assets::assets_dir();
    let bible = Bible::embedded()?;
    let embedder = assets::load_embedder(&dir)?;
    let want = bt_core::search::embeddings_fingerprint(&bible, embedder.as_ref());
    let f = std::fs::File::open(dir.join("embeddings.bin")).context("no embeddings.bin")?;
    let got = bt_core::vector::EmbeddingsFile::read_from(std::io::BufReader::new(f))?.fingerprint;
    if got != want {
        bail!("embeddings.bin is stale (rebuild with `bt build-index`)");
    }
    println!("embeddings.bin is current ({})", embedder.id());
    Ok(())
}

fn quantize(args: &[String]) -> Result<()> {
    let [input, output] = args else {
        bail!("usage: bt quantize IN OUT")
    };
    #[cfg(feature = "model")]
    {
        bt_core::embed::quantize_f16(input.as_ref(), output.as_ref())?;
        println!("wrote {output}");
        Ok(())
    }
    #[cfg(not(feature = "model"))]
    {
        let _ = (input, output);
        bail!("built without the `model` feature")
    }
}

fn search(q: &str, n: Option<&String>) -> Result<()> {
    let engine = assets::load_engine()?;
    let limit = n.map(|s| s.parse()).transpose()?.unwrap_or(10);
    let r = engine.search(
        q,
        &SearchOptions {
            limit,
            ..Default::default()
        },
    )?;
    if let Some(h) = &r.reference {
        println!("reference: {}", h.label);
    }
    for v in &r.verses {
        let how = match (v.keyword, v.semantic) {
            (true, true) => "kw+sem",
            (true, false) => "kw",
            (false, true) => "sem",
            _ => "ref",
        };
        println!("{:.3} {:6} {:16} {}", v.score, how, v.label, v.text);
    }
    println!("-- passages");
    for s in &r.sections {
        println!("{:.3} {:22} {} — {}", s.score, s.label, s.title, s.summary);
    }
    println!("({:.2} ms, {})", r.took_ms, engine.embedder_id());
    Ok(())
}

pub const BENCH_QUERIES: &[&str] = &[
    "eye of the needle",
    "love your enemies",
    "John 3:16",
    "the prodigal son",
    "Jesus walks on water",
    "blessed are the meek",
    "faith like a mustard seed",
    "Mk 4",
    "who is my neighbour",
    "bread of life",
];

fn bench(rounds: Option<&String>) -> Result<()> {
    let t = Instant::now();
    let engine: Engine = assets::load_engine()?;
    println!("startup {:.1?} ({})", t.elapsed(), engine.embedder_id());
    let rounds: usize = rounds.map(|s| s.parse()).transpose()?.unwrap_or(20);
    // Cold = first time each query is embedded; warm = cached query vector.
    let mut cold = Vec::new();
    for q in BENCH_QUERIES {
        let t = Instant::now();
        engine.search(q, &SearchOptions::default())?;
        cold.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let mut warm = Vec::new();
    for _ in 0..rounds {
        for q in BENCH_QUERIES {
            let t = Instant::now();
            engine.search(q, &SearchOptions::default())?;
            warm.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    report("cold (embeds query)", &mut cold);
    report("warm (cached vector)", &mut warm);
    Ok(())
}

fn report(name: &str, xs: &mut [f64]) {
    xs.sort_by(f64::total_cmp);
    let p = |q: f64| xs[((xs.len() - 1) as f64 * q) as usize];
    println!(
        "{name:22} n={:4}  p50 {:7.2} ms  p95 {:7.2} ms  max {:7.2} ms",
        xs.len(),
        p(0.5),
        p(0.95),
        p(1.0)
    );
}
