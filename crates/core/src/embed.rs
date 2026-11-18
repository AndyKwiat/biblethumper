//! Text embedders.
//!
//! * `MiniLm` (feature `model`, default): sentence-transformers/all-MiniLM-L6-v2
//!   run on CPU with candle. 22M params, 384-dim output, ~90 MB RAM as f32,
//!   a few ms per short query. Weights live in `assets/model/` (see
//!   `scripts/fetch_model.sh`).
//! * `HashEmbedder`: deterministic feature hashing of words. Not semantic at
//!   all, but it lets tests and dev runs exercise the whole pipeline without the
//!   model files.

use anyhow::Result;

pub trait Embedder: Send + Sync {
    /// Identifies the model; part of the embeddings-file fingerprint.
    fn id(&self) -> String;
    fn dim(&self) -> usize;
    /// One L2-normalised vector per input text.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Hashes stemmed words and word bigrams into `dim` buckets with ±1 signs.
pub struct HashEmbedder {
    pub dim: usize,
}

impl Default for HashEmbedder {
    fn default() -> Self {
        HashEmbedder { dim: 384 }
    }
}

impl Embedder for HashEmbedder {
    fn id(&self) -> String {
        format!("hash-{}", self.dim)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.0f32; self.dim];
                let toks = crate::text::content_stems(t);
                let mut add = |s: &str, w: f32| {
                    let h = crate::vector::fingerprint([s]);
                    let sign = if h >> 63 == 1 { -1.0 } else { 1.0 };
                    v[(h % self.dim as u64) as usize] += sign * w;
                };
                for tok in &toks {
                    add(tok, 1.0);
                }
                for pair in toks.windows(2) {
                    add(&format!("{} {}", pair[0], pair[1]), 0.5);
                }
                crate::vector::normalize(&mut v);
                v
            })
            .collect())
    }
}

#[cfg(feature = "model")]
pub use minilm::{MiniLm, quantize_f16};

#[cfg(feature = "model")]
mod minilm {
    use super::Embedder;
    use anyhow::{Context, Error as E, Result};
    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::bert::{BertModel, Config};
    use std::path::Path;
    use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

    /// Max tokens per text. Gospel verses are < 100 tokens; summaries < 100.
    const MAX_TOKENS: usize = 256;

    pub struct MiniLm {
        model: BertModel,
        tokenizer: Tokenizer,
        device: Device,
        dim: usize,
    }

    impl MiniLm {
        /// Load `config.json`, `tokenizer.json`, `model.safetensors` (f16 or f32) from `dir`.
        pub fn load(dir: &Path) -> Result<Self> {
            let device = Device::Cpu;
            let config: Config = serde_json::from_str(
                &std::fs::read_to_string(dir.join("config.json"))
                    .with_context(|| format!("reading {}/config.json", dir.display()))?,
            )?;
            let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(E::msg)?;
            tokenizer.with_padding(Some(PaddingParams {
                strategy: PaddingStrategy::BatchLongest,
                ..Default::default()
            }));
            tokenizer
                .with_truncation(Some(TruncationParams {
                    max_length: MAX_TOKENS,
                    ..Default::default()
                }))
                .map_err(E::msg)?;
            let weights = std::fs::read(dir.join("model.safetensors"))
                .with_context(|| format!("reading {}/model.safetensors", dir.display()))?;
            // Weights may be stored as f16; VarBuilder converts every tensor to f32.
            let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)?;
            let model = BertModel::load(vb, &config)?;
            Ok(MiniLm {
                model,
                tokenizer,
                device,
                dim: config.hidden_size,
            })
        }
    }

    /// Convert a safetensors file to f16 (halves download/deploy size; quality loss is negligible
    /// because inference still runs in f32).
    pub fn quantize_f16(input: &Path, output: &Path) -> Result<()> {
        let tensors = candle_core::safetensors::load(input, &Device::Cpu)?;
        let mut converted = std::collections::HashMap::new();
        for (name, t) in tensors {
            let t = if t.dtype() == DType::F32 {
                t.to_dtype(DType::F16)?
            } else {
                t
            };
            converted.insert(name, t);
        }
        candle_core::safetensors::save(&converted, output)?;
        Ok(())
    }

    impl Embedder for MiniLm {
        fn id(&self) -> String {
            "all-MiniLM-L6-v2/mean".into()
        }

        fn dim(&self) -> usize {
            self.dim
        }

        fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let enc = self
                .tokenizer
                .encode_batch(texts.to_vec(), true)
                .map_err(E::msg)?;
            let ids = enc
                .iter()
                .map(|e| Tensor::new(e.get_ids(), &self.device))
                .collect::<candle_core::Result<Vec<_>>>()?;
            let mask = enc
                .iter()
                .map(|e| Tensor::new(e.get_attention_mask(), &self.device))
                .collect::<candle_core::Result<Vec<_>>>()?;
            let ids = Tensor::stack(&ids, 0)?;
            let mask = Tensor::stack(&mask, 0)?;
            let type_ids = ids.zeros_like()?;
            let hidden = self.model.forward(&ids, &type_ids, Some(&mask))?; // [batch, tokens, dim]
            // Mean pooling over real (non-padding) tokens, as sentence-transformers does.
            let m = mask.to_dtype(DType::F32)?.unsqueeze(2)?;
            let summed = hidden.broadcast_mul(&m)?.sum(1)?;
            let counts = m.sum(1)?;
            let mean = summed.broadcast_div(&counts)?;
            let mut out: Vec<Vec<f32>> = mean.to_vec2()?;
            out.iter_mut().for_each(|v| crate::vector::normalize(v));
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::dot;

    #[test]
    fn hash_embedder_is_deterministic_and_normalised() {
        let e = HashEmbedder::default();
        let a = e
            .embed(&["the eye of a needle", "the eye of a needle"])
            .unwrap();
        assert_eq!(a[0], a[1]);
        assert!((dot(&a[0], &a[0]) - 1.0).abs() < 1e-5);
        assert_eq!(a[0].len(), 384);
    }

    #[test]
    fn hash_embedder_similar_texts_score_higher() {
        let e = HashEmbedder::default();
        let v = e
            .embed(&[
                "camel through the eye of a needle",
                "eye of the needle",
                "blessed are the meek",
            ])
            .unwrap();
        assert!(dot(&v[0], &v[1]) > dot(&v[2], &v[1]));
    }

    #[test]
    fn empty_text_gives_zero_vector() {
        let e = HashEmbedder::default();
        let v = e.embed(&[""]).unwrap();
        assert!(v[0].iter().all(|x| *x == 0.0));
    }
}
