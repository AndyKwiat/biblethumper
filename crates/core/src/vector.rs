//! Flat (brute-force) cosine-similarity index plus the on-disk embeddings file.
//!
//! ~4,200 vectors x 384 dims is ~1.6M multiply-adds per query: well under a
//! millisecond, so no ANN structure is needed. Vectors are L2-normalised when
//! inserted, so cosine similarity is a plain dot product.

use anyhow::{Result, bail};
use std::io::{Read, Write};

pub struct VectorIndex {
    dim: usize,
    data: Vec<f32>,
}

impl VectorIndex {
    pub fn new(dim: usize) -> Self {
        VectorIndex {
            dim,
            data: Vec::new(),
        }
    }

    pub fn from_flat(dim: usize, data: Vec<f32>) -> Result<Self> {
        if dim == 0 || !data.len().is_multiple_of(dim) {
            bail!(
                "flat data length {} is not a multiple of dim {dim}",
                data.len()
            );
        }
        let mut idx = VectorIndex { dim, data };
        for i in 0..idx.len() {
            normalize(&mut idx.data[i * dim..(i + 1) * dim]);
        }
        Ok(idx)
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn len(&self) -> usize {
        self.data.len() / self.dim
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn push(&mut self, v: &[f32]) {
        assert_eq!(v.len(), self.dim, "vector dim mismatch");
        let start = self.data.len();
        self.data.extend_from_slice(v);
        normalize(&mut self.data[start..]);
    }

    pub fn get(&self, i: usize) -> &[f32] {
        &self.data[i * self.dim..(i + 1) * self.dim]
    }

    pub fn as_flat(&self) -> &[f32] {
        &self.data
    }

    /// Top `k` (index, cosine) pairs, best first.
    pub fn top_k(&self, query: &[f32], k: usize) -> Vec<(u32, f32)> {
        assert_eq!(query.len(), self.dim, "query dim mismatch");
        let mut q = query.to_vec();
        normalize(&mut q);
        let mut scored: Vec<(u32, f32)> = self
            .data
            .chunks_exact(self.dim)
            .enumerate()
            .map(|(i, v)| (i as u32, dot(&q, v)))
            .collect();
        let k = k.min(scored.len());
        if k == 0 {
            return Vec::new();
        }
        let cmp = |a: &(u32, f32), b: &(u32, f32)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
        scored.select_nth_unstable_by(k - 1, cmp);
        scored.truncate(k);
        scored.sort_unstable_by(cmp);
        scored
    }
}

/// Dot product written as 8 independent accumulators so LLVM vectorises it.
#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    acc.iter().sum::<f32>() + tail
}

pub fn normalize(v: &mut [f32]) {
    let n = dot(v, v).sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

// ---- embeddings file ------------------------------------------------------
//
// Layout (little endian):
//   magic "BTEMB001" | u64 fingerprint | u32 dim | u32 n_verses | u32 n_sections
//   | f32 * dim * (n_verses + n_sections)
// The fingerprint hashes the model id and every embedded text, so a stale file
// (changed summaries, new model) is detected and rebuilt instead of silently used.

const MAGIC: &[u8; 8] = b"BTEMB001";

pub struct EmbeddingsFile {
    pub fingerprint: u64,
    pub verses: VectorIndex,
    pub sections: VectorIndex,
}

impl EmbeddingsFile {
    pub fn write_to(&self, mut w: impl Write) -> Result<()> {
        w.write_all(MAGIC)?;
        w.write_all(&self.fingerprint.to_le_bytes())?;
        w.write_all(&(self.verses.dim() as u32).to_le_bytes())?;
        w.write_all(&(self.verses.len() as u32).to_le_bytes())?;
        w.write_all(&(self.sections.len() as u32).to_le_bytes())?;
        let mut buf =
            Vec::with_capacity((self.verses.as_flat().len() + self.sections.as_flat().len()) * 4);
        for x in self.verses.as_flat().iter().chain(self.sections.as_flat()) {
            buf.extend_from_slice(&x.to_le_bytes());
        }
        w.write_all(&buf)?;
        Ok(())
    }

    pub fn read_from(mut r: impl Read) -> Result<Self> {
        let mut head = [0u8; 28];
        r.read_exact(&mut head)?;
        if &head[0..8] != MAGIC {
            bail!("not an embeddings file (bad magic)");
        }
        let fingerprint = u64::from_le_bytes(head[8..16].try_into()?);
        let dim = u32::from_le_bytes(head[16..20].try_into()?) as usize;
        let nv = u32::from_le_bytes(head[20..24].try_into()?) as usize;
        let ns = u32::from_le_bytes(head[24..28].try_into()?) as usize;
        let mut bytes = Vec::new();
        r.read_to_end(&mut bytes)?;
        if bytes.len() != (nv + ns) * dim * 4 {
            bail!(
                "embeddings file truncated: {} bytes, expected {}",
                bytes.len(),
                (nv + ns) * dim * 4
            );
        }
        let all: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let (v, s) = all.split_at(nv * dim);
        Ok(EmbeddingsFile {
            fingerprint,
            verses: VectorIndex::from_flat(dim, v.to_vec())?,
            sections: VectorIndex::from_flat(dim, s.to_vec())?,
        })
    }
}

/// FNV-1a, stable across platforms and Rust versions (unlike `DefaultHasher`).
pub fn fingerprint<'a>(parts: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes().chain([0xff]) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_matches_naive() {
        let a: Vec<f32> = (0..37).map(|i| i as f32 * 0.5).collect();
        let b: Vec<f32> = (0..37).map(|i| (i as f32).sin()).collect();
        let naive: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        assert!((dot(&a, &b) - naive).abs() < 1e-3);
    }

    #[test]
    fn top_k_orders_by_cosine() {
        let mut idx = VectorIndex::new(3);
        idx.push(&[1.0, 0.0, 0.0]);
        idx.push(&[0.0, 1.0, 0.0]);
        idx.push(&[1.0, 1.0, 0.0]);
        idx.push(&[0.0, 0.0, 5.0]);
        let top = idx.top_k(&[2.0, 0.1, 0.0], 2);
        assert_eq!(top[0].0, 0);
        assert_eq!(top[1].0, 2);
        assert!((top[0].1 - 0.9988).abs() < 1e-3);
        assert_eq!(idx.top_k(&[1.0, 0.0, 0.0], 10).len(), 4);
        assert!(idx.top_k(&[1.0, 0.0, 0.0], 0).is_empty());
    }

    #[test]
    fn file_roundtrip() {
        let mut verses = VectorIndex::new(2);
        verses.push(&[3.0, 4.0]);
        verses.push(&[1.0, 0.0]);
        let mut sections = VectorIndex::new(2);
        sections.push(&[0.0, 2.0]);
        let f = EmbeddingsFile {
            fingerprint: 42,
            verses,
            sections,
        };
        let mut buf = Vec::new();
        f.write_to(&mut buf).unwrap();
        let g = EmbeddingsFile::read_from(&buf[..]).unwrap();
        assert_eq!(g.fingerprint, 42);
        assert_eq!(g.verses.len(), 2);
        assert_eq!(g.sections.len(), 1);
        assert_eq!(g.verses.get(0), &[0.6, 0.8]);
        assert!(EmbeddingsFile::read_from(&buf[..buf.len() - 1]).is_err());
        assert!(EmbeddingsFile::read_from(&b"nope"[..]).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        assert_eq!(fingerprint(["a", "b"]), fingerprint(["a", "b"]));
        assert_ne!(fingerprint(["a", "b"]), fingerprint(["ab"]));
        assert_ne!(fingerprint(["a", "b"]), fingerprint(["a", "c"]));
    }
}
