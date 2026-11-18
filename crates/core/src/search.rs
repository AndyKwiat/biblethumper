//! Hybrid search: reference lookup, BM25 keywords and embedding similarity,
//! merged with weighted reciprocal-rank fusion (RRF).
//!
//! Pipeline for a query:
//! 1. If the whole query is a reference ("John 3:16", "mk 4"), return it directly.
//! 2. Otherwise gather four ranked lists:
//!    verse keywords (BM25 on KJV text)      verse semantics (cosine on verse vectors)
//!    section keywords (BM25 on title+summary) section semantics (cosine on summary vectors)
//! 3. Verses score   W_KW/(K+rank_kw) + W_SEM/(K+rank_sem) + W_SECTION/(K+rank of their section)
//!    plus PHRASE_BOOST when every query word matched in order ("eye of the needle").
//!    Sections score the same way from their two lists.
//!
//! Tuning knobs are the constants below; `tests/search_quality.rs` (server crate)
//! holds golden queries to check any change against the real model.

use crate::embed::Embedder;
use crate::keyword::KeywordIndex;
use crate::model::{Bible, SectionId, VerseId};
use crate::reference::parse_reference;
use crate::vector::{EmbeddingsFile, VectorIndex, fingerprint};
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const RRF_K: f32 = 10.0;
const W_KW: f32 = 1.0;
const W_SEM: f32 = 1.0;
const W_SECTION: f32 = 0.5;
const PHRASE_BOOST: f32 = 0.15;
/// How many candidates each list contributes before fusion.
const CANDIDATES: usize = 120;
/// Semantic hits below this cosine are noise and are dropped.
const MIN_COSINE: f32 = 0.2;
/// Max verses returned for a reference range.
const MAX_REFERENCE_VERSES: usize = 60;
const CACHE_CAP: usize = 2048;
const EMBED_BATCH: usize = 32;

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub limit: usize,
    pub section_limit: usize,
    /// Restrict to one book (index into `Bible::books`).
    pub book: Option<u8>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            limit: 20,
            section_limit: 5,
            book: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub reference: Option<ReferenceHit>,
    pub verses: Vec<VerseHit>,
    pub sections: Vec<SectionHit>,
    pub took_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceHit {
    pub label: String,
    pub book: String,
    pub chapter: u16,
    pub verse_start: Option<u16>,
    pub verse_end: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerseHit {
    pub label: String,
    pub book: String,
    pub chapter: u16,
    pub verse: u16,
    pub text: String,
    pub score: f32,
    /// Matched by keyword search.
    pub keyword: bool,
    /// Matched by meaning (embedding similarity).
    pub semantic: bool,
    pub section_title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionHit {
    pub label: String,
    pub book: String,
    pub chapter: u16,
    pub start: u16,
    pub end: u16,
    pub title: String,
    pub summary: String,
    pub score: f32,
}

pub struct Engine {
    pub bible: Bible,
    verse_kw: KeywordIndex,
    section_kw: KeywordIndex,
    verse_vecs: VectorIndex,
    section_vecs: VectorIndex,
    embedder: Arc<dyn Embedder>,
    cache: Mutex<HashMap<String, Arc<Vec<f32>>>>,
}

/// Fingerprint of everything the embeddings depend on.
pub fn embeddings_fingerprint(bible: &Bible, embedder: &dyn Embedder) -> u64 {
    let id = embedder.id();
    let sections: Vec<String> = (0..bible.sections.len())
        .map(|i| bible.section_embed_text(i as u32))
        .collect();
    fingerprint(
        std::iter::once(id.as_str())
            .chain(bible.verses.iter().map(|v| v.text.as_str()))
            .chain(sections.iter().map(String::as_str)),
    )
}

/// Embed every verse and section. `progress(done, total)` is called per batch.
pub fn compute_embeddings(
    bible: &Bible,
    embedder: &dyn Embedder,
    mut progress: impl FnMut(usize, usize),
) -> Result<EmbeddingsFile> {
    let section_texts: Vec<String> = (0..bible.sections.len())
        .map(|i| bible.section_embed_text(i as u32))
        .collect();
    let verse_texts: Vec<&str> = bible.verses.iter().map(|v| v.text.as_str()).collect();
    let total = verse_texts.len() + section_texts.len();
    let mut done = 0;
    let mut embed_all = |texts: Vec<&str>| -> Result<VectorIndex> {
        // Batch texts of similar length together so little time is spent on padding.
        let mut order: Vec<usize> = (0..texts.len()).collect();
        order.sort_by_key(|&i| texts[i].len());
        let mut vecs: Vec<Vec<f32>> = vec![Vec::new(); texts.len()];
        for chunk in order.chunks(EMBED_BATCH) {
            let batch: Vec<&str> = chunk.iter().map(|&i| texts[i]).collect();
            for (&i, v) in chunk.iter().zip(embedder.embed(&batch)?) {
                vecs[i] = v;
            }
            done += chunk.len();
            progress(done, total);
        }
        let mut idx = VectorIndex::new(embedder.dim());
        for v in &vecs {
            idx.push(v);
        }
        Ok(idx)
    };
    let verses = embed_all(verse_texts)?;
    let sections = embed_all(section_texts.iter().map(String::as_str).collect())?;
    Ok(EmbeddingsFile {
        fingerprint: embeddings_fingerprint(bible, embedder),
        verses,
        sections,
    })
}

impl Engine {
    /// Build the engine. `embeddings` must match the bible + embedder
    /// (check with `embeddings_fingerprint`); use `compute_embeddings` otherwise.
    pub fn new(
        bible: Bible,
        embedder: Arc<dyn Embedder>,
        embeddings: EmbeddingsFile,
    ) -> Result<Engine> {
        if embeddings.verses.len() != bible.verses.len()
            || embeddings.sections.len() != bible.sections.len()
        {
            bail!(
                "embeddings do not match the data (verses {} vs {}, sections {} vs {})",
                embeddings.verses.len(),
                bible.verses.len(),
                embeddings.sections.len(),
                bible.sections.len()
            );
        }
        if embeddings.verses.dim() != embedder.dim() {
            bail!(
                "embedding dim {} does not match embedder dim {}",
                embeddings.verses.dim(),
                embedder.dim()
            );
        }
        let verse_kw = KeywordIndex::build(
            &bible
                .verses
                .iter()
                .map(|v| v.text.as_str())
                .collect::<Vec<_>>(),
        );
        let section_kw = KeywordIndex::build(
            &bible
                .sections
                .iter()
                .map(|s| format!("{} {}", s.title, s.summary))
                .collect::<Vec<_>>(),
        );
        Ok(Engine {
            bible,
            verse_kw,
            section_kw,
            verse_vecs: embeddings.verses,
            section_vecs: embeddings.sections,
            embedder,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Convenience for tests/dev: compute embeddings in-process.
    pub fn build(bible: Bible, embedder: Arc<dyn Embedder>) -> Result<Engine> {
        let emb = compute_embeddings(&bible, embedder.as_ref(), |_, _| {})?;
        Engine::new(bible, embedder, emb)
    }

    pub fn embedder_id(&self) -> String {
        self.embedder.id()
    }

    fn query_vector(&self, q: &str) -> Result<Arc<Vec<f32>>> {
        let key = q.to_lowercase();
        if let Some(v) = self.cache.lock().unwrap().get(&key) {
            return Ok(v.clone());
        }
        let v = Arc::new(self.embedder.embed(&[&key])?.remove(0));
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, v.clone());
        Ok(v)
    }

    pub fn search(&self, query: &str, opts: &SearchOptions) -> Result<SearchResponse> {
        let t0 = Instant::now();
        let q = query.trim();
        let mut resp = SearchResponse {
            query: q.to_string(),
            reference: None,
            verses: Vec::new(),
            sections: Vec::new(),
            took_ms: 0.0,
        };
        if q.is_empty() {
            return Ok(resp);
        }
        if let Some(hit) = self.reference_search(q, opts, &mut resp) {
            resp.reference = Some(hit);
        } else {
            self.hybrid_search(q, opts, &mut resp)?;
        }
        resp.took_ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(resp)
    }

    fn reference_search(
        &self,
        q: &str,
        opts: &SearchOptions,
        resp: &mut SearchResponse,
    ) -> Option<ReferenceHit> {
        let r = parse_reference(q)?;
        let b = &self.bible;
        let ch = b.chapter(r.book, r.chapter)?;
        let book = &b.books[r.book as usize];
        let (label, range) = match (r.verse_start, r.verse_end) {
            (None, _) => (format!("{} {}", book.name, r.chapter), None),
            (Some(a), end) => {
                if a > ch.verse_count {
                    return None;
                }
                let e = end.unwrap_or(a).min(ch.verse_count);
                let label = if e == a {
                    format!("{} {}:{}", book.name, r.chapter, a)
                } else {
                    format!("{} {}:{}\u{2013}{}", book.name, r.chapter, a, e)
                };
                (label, Some((a, e)))
            }
        };
        if let Some((a, e)) = range {
            let first = b.verse_id(r.book, r.chapter, a)?;
            let n = ((e - a + 1) as usize).min(MAX_REFERENCE_VERSES);
            let mut seen_sections = Vec::new();
            for vid in first..first + n as u32 {
                resp.verses.push(self.verse_hit(vid, 1.0, false, false));
                let sid = b.verses[vid as usize].section;
                if !seen_sections.contains(&sid) {
                    seen_sections.push(sid);
                }
            }
            resp.sections = seen_sections
                .into_iter()
                .take(opts.section_limit.max(1))
                .map(|s| self.section_hit(s, 1.0))
                .collect();
        }
        Some(ReferenceHit {
            label,
            book: book.id.clone(),
            chapter: r.chapter,
            verse_start: range.map(|r| r.0),
            verse_end: range.map(|r| r.1),
        })
    }

    fn hybrid_search(
        &self,
        q: &str,
        opts: &SearchOptions,
        resp: &mut SearchResponse,
    ) -> Result<()> {
        let b = &self.bible;
        let in_book = |book: u8| opts.book.is_none_or(|x| x == book);
        let pool = if opts.book.is_some() {
            CANDIDATES * 4
        } else {
            CANDIDATES
        };
        let qv = self.query_vector(q)?;

        // --- sections first: their ranks feed into verse scores -----------
        let sec_sem: Vec<(SectionId, f32)> = self
            .section_vecs
            .top_k(&qv, pool)
            .into_iter()
            .filter(|(s, c)| *c >= MIN_COSINE && in_book(b.sections[*s as usize].book))
            .collect();
        let sec_kw: Vec<SectionId> = self
            .section_kw
            .search(q, pool)
            .into_iter()
            .map(|h| h.doc)
            .filter(|s| in_book(b.sections[*s as usize].book))
            .collect();
        let mut sec_scores: HashMap<SectionId, f32> = HashMap::new();
        for (rank, (s, _)) in sec_sem.iter().enumerate() {
            *sec_scores.entry(*s).or_default() += W_SEM / (RRF_K + rank as f32);
        }
        for (rank, s) in sec_kw.iter().enumerate() {
            *sec_scores.entry(*s).or_default() += W_KW / (RRF_K + rank as f32);
        }
        let ranked_sections = sort_scores(sec_scores);
        let section_rank: HashMap<SectionId, usize> = ranked_sections
            .iter()
            .enumerate()
            .map(|(r, (s, _))| (*s, r))
            .collect();

        // --- verses --------------------------------------------------------
        let kw: Vec<_> = self
            .verse_kw
            .search(q, pool)
            .into_iter()
            .filter(|h| in_book(b.verses[h.doc as usize].book))
            .collect();
        let sem: Vec<(VerseId, f32)> = self
            .verse_vecs
            .top_k(&qv, pool)
            .into_iter()
            .filter(|(v, c)| *c >= MIN_COSINE && in_book(b.verses[*v as usize].book))
            .collect();
        let multiword = crate::text::content_stems(q).len() >= 2;
        let mut scores: HashMap<VerseId, (f32, bool, bool)> = HashMap::new();
        for (rank, h) in kw.iter().enumerate() {
            let e = scores.entry(h.doc).or_default();
            e.0 += W_KW / (RRF_K + rank as f32);
            if multiword && h.full && h.phrase {
                e.0 += PHRASE_BOOST;
            }
            e.1 = true;
        }
        for (rank, (v, _)) in sem.iter().enumerate() {
            let e = scores.entry(*v).or_default();
            e.0 += W_SEM / (RRF_K + rank as f32);
            e.2 = true;
        }
        for (v, e) in scores.iter_mut() {
            if let Some(r) = section_rank.get(&b.verses[*v as usize].section) {
                e.0 += W_SECTION / (RRF_K + *r as f32);
            }
        }
        let mut ranked: Vec<(VerseId, (f32, bool, bool))> = scores.into_iter().collect();
        ranked.sort_unstable_by(|a, b| b.1.0.total_cmp(&a.1.0).then(a.0.cmp(&b.0)));
        resp.verses = ranked
            .into_iter()
            .take(opts.limit)
            .map(|(v, (s, kw, sem))| self.verse_hit(v, s, kw, sem))
            .collect();
        resp.sections = ranked_sections
            .into_iter()
            .take(opts.section_limit)
            .map(|(s, score)| self.section_hit(s, score))
            .collect();
        Ok(())
    }

    fn verse_hit(&self, id: VerseId, score: f32, keyword: bool, semantic: bool) -> VerseHit {
        let b = &self.bible;
        let v = &b.verses[id as usize];
        VerseHit {
            label: b.verse_label(id),
            book: b.books[v.book as usize].id.clone(),
            chapter: v.chapter,
            verse: v.verse,
            text: v.text.clone(),
            score,
            keyword,
            semantic,
            section_title: b.sections[v.section as usize].title.clone(),
        }
    }

    fn section_hit(&self, id: SectionId, score: f32) -> SectionHit {
        let b = &self.bible;
        let s = &b.sections[id as usize];
        SectionHit {
            label: b.section_label(id),
            book: b.books[s.book as usize].id.clone(),
            chapter: s.chapter,
            start: s.start,
            end: s.end,
            title: s.title.clone(),
            summary: s.summary.clone(),
            score,
        }
    }
}

fn sort_scores(m: HashMap<u32, f32>) -> Vec<(u32, f32)> {
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;
    use std::sync::OnceLock;

    fn engine() -> &'static Engine {
        static E: OnceLock<Engine> = OnceLock::new();
        E.get_or_init(|| {
            Engine::build(
                Bible::embedded().unwrap(),
                Arc::new(HashEmbedder::default()),
            )
            .unwrap()
        })
    }

    fn labels(r: &SearchResponse) -> Vec<&str> {
        r.verses.iter().map(|v| v.label.as_str()).collect()
    }

    #[test]
    fn eye_of_the_needle_finds_all_three_gospels() {
        let r = engine()
            .search("eye of the needle", &SearchOptions::default())
            .unwrap();
        let top: Vec<_> = labels(&r).into_iter().take(3).collect();
        for want in ["Matthew 19:24", "Mark 10:25", "Luke 18:25"] {
            assert!(top.contains(&want), "{want} not in top 3: {top:?}");
        }
        assert!(r.verses[0].keyword);
    }

    #[test]
    fn reference_queries_return_the_verse() {
        let r = engine()
            .search("John 3:16", &SearchOptions::default())
            .unwrap();
        assert_eq!(r.reference.as_ref().unwrap().label, "John 3:16");
        assert_eq!(labels(&r), ["John 3:16"]);
        assert_eq!(r.sections.len(), 1);
    }

    #[test]
    fn reference_ranges_are_clamped() {
        let r = engine()
            .search("jn 11:33-200", &SearchOptions::default())
            .unwrap();
        let hit = r.reference.unwrap();
        assert_eq!(hit.verse_start, Some(33));
        assert_eq!(hit.verse_end, Some(57));
        assert_eq!(r.verses.len(), 25);
    }

    #[test]
    fn chapter_reference_has_no_verses() {
        let r = engine()
            .search("Mark 4", &SearchOptions::default())
            .unwrap();
        assert_eq!(r.reference.unwrap().label, "Mark 4");
        assert!(r.verses.is_empty());
    }

    #[test]
    fn out_of_range_reference_falls_back_to_text_search() {
        let r = engine()
            .search("Mark 40", &SearchOptions::default())
            .unwrap();
        assert!(r.reference.is_none());
    }

    #[test]
    fn book_filter_and_limit() {
        let opts = SearchOptions {
            limit: 5,
            section_limit: 2,
            book: Some(3),
        };
        let r = engine().search("love one another", &opts).unwrap();
        assert!(!r.verses.is_empty() && r.verses.len() <= 5);
        assert!(r.verses.iter().all(|v| v.book == "JHN"));
        assert!(r.sections.len() <= 2 && r.sections.iter().all(|s| s.book == "JHN"));
    }

    #[test]
    fn empty_and_nonsense_queries() {
        let r = engine().search("   ", &SearchOptions::default()).unwrap();
        assert!(r.verses.is_empty() && r.reference.is_none());
        let r = engine()
            .search("qwxzzy", &SearchOptions::default())
            .unwrap();
        assert!(r.verses.iter().all(|v| !v.keyword));
    }

    #[test]
    fn sections_match_summaries() {
        let r = engine()
            .search("Lazarus raised from the dead", &SearchOptions::default())
            .unwrap();
        assert!(
            r.sections
                .iter()
                .any(|s| s.book == "JHN" && s.chapter == 11),
            "{:?}",
            r.sections
        );
    }

    #[test]
    fn scores_are_descending() {
        let r = engine()
            .search(
                "faith as a grain of mustard seed",
                &SearchOptions::default(),
            )
            .unwrap();
        assert!(r.verses.windows(2).all(|w| w[0].score >= w[1].score));
        assert!(r.sections.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn mismatched_embeddings_are_rejected() {
        let bible = Bible::embedded().unwrap();
        let empty = EmbeddingsFile {
            fingerprint: 0,
            verses: VectorIndex::new(384),
            sections: VectorIndex::new(384),
        };
        assert!(Engine::new(bible, Arc::new(HashEmbedder::default()), empty).is_err());
    }
}
