//! Finding and loading runtime assets: the embedding model and the
//! precomputed embeddings file.
//!
//! Layout (same in the repo and in a deploy):
//!   assets/model/{config.json,tokenizer.json,model.safetensors}
//!   assets/embeddings.bin
//! The assets dir is `$BT_ASSETS`, else `<exe>/../../assets` (dist/bin/app ->
//! dist/assets), else `./assets`.

use anyhow::Result;
use bt_core::embed::{Embedder, HashEmbedder};
use bt_core::search::{compute_embeddings, embeddings_fingerprint};
use bt_core::vector::EmbeddingsFile;
use bt_core::{Bible, Engine};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn assets_dir() -> PathBuf {
    if let Ok(p) = std::env::var("BT_ASSETS") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent().and_then(Path::parent)
    {
        let cand = dir.join("assets");
        if cand.is_dir() {
            return cand;
        }
    }
    PathBuf::from("assets")
}

/// MiniLM if the model files are present, else the (non-semantic) HashEmbedder.
pub fn load_embedder(assets: &Path) -> Result<Arc<dyn Embedder>> {
    let model_dir = assets.join("model");
    #[cfg(feature = "model")]
    if model_dir.join("model.safetensors").is_file() {
        let t = std::time::Instant::now();
        let m = bt_core::embed::MiniLm::load(&model_dir)?;
        tracing::info!(
            "loaded MiniLM from {} in {:?}",
            model_dir.display(),
            t.elapsed()
        );
        return Ok(Arc::new(m));
    }
    tracing::warn!(
        "no model at {} (run scripts/fetch_model.sh); using HashEmbedder: keyword-ish only, NOT semantic",
        model_dir.display()
    );
    Ok(Arc::new(HashEmbedder::default()))
}

/// Load `embeddings.bin` from `assets` (or the `$DATA_DIR` cache) if its
/// fingerprint matches; otherwise compute embeddings and cache them in
/// `$DATA_DIR` (when set) so the next start is instant.
pub fn load_or_build_embeddings(
    bible: &Bible,
    embedder: &dyn Embedder,
    assets: &Path,
) -> Result<EmbeddingsFile> {
    let want = embeddings_fingerprint(bible, embedder);
    let cache = std::env::var("DATA_DIR")
        .ok()
        .map(|d| PathBuf::from(d).join("embeddings.bin"));
    for path in std::iter::once(assets.join("embeddings.bin")).chain(cache.clone()) {
        match std::fs::File::open(&path).map(std::io::BufReader::new) {
            Ok(f) => match EmbeddingsFile::read_from(f) {
                Ok(e) if e.fingerprint == want => {
                    tracing::info!("using embeddings from {}", path.display());
                    return Ok(e);
                }
                Ok(_) => tracing::warn!("{} is stale (data or model changed)", path.display()),
                Err(err) => tracing::warn!("{}: {err}", path.display()),
            },
            Err(_) => continue,
        }
    }
    tracing::info!("computing embeddings with {} ...", embedder.id());
    let t = std::time::Instant::now();
    let emb = compute_embeddings(bible, embedder, |_, _| {})?;
    tracing::info!("computed embeddings in {:?}", t.elapsed());
    if let Some(path) = cache
        && let Err(err) = write_embeddings(&emb, &path)
    {
        tracing::warn!("could not cache embeddings at {}: {err}", path.display());
    }
    Ok(emb)
}

pub fn write_embeddings(emb: &EmbeddingsFile, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    emb.write_to(std::io::BufWriter::new(std::fs::File::create(&tmp)?))?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Everything `main` needs: bible + embedder + embeddings -> Engine.
pub fn load_engine() -> Result<Engine> {
    let assets = assets_dir();
    let bible = Bible::embedded()?;
    let embedder = load_embedder(&assets)?;
    let emb = load_or_build_embeddings(&bible, embedder.as_ref(), &assets)?;
    Engine::new(bible, embedder, emb)
}
