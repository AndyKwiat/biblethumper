//! BM25 keyword index with a phrase/proximity bonus.
//!
//! Documents are plain strings addressed by position (doc 0..n). Scoring:
//!   bm25(doc) * coverage^2 * (1 + PHRASE_BONUS * adjacent_pairs / (n_terms - 1))
//! where `coverage` = fraction of distinct query terms present and
//! `adjacent_pairs` counts consecutive query content words that also appear
//! consecutively in the doc (ignoring stopwords). So "eye of the needle" ranks
//! "...the eye of a needle..." above verses that merely mention "eye".

use crate::text::{content_stems, stems};
use std::collections::HashMap;

const K1: f32 = 1.2;
const B: f32 = 0.75;
const PHRASE_BONUS: f32 = 1.0;

pub struct KeywordIndex {
    vocab: HashMap<String, u32>,
    /// postings[term] = [(doc, term frequency)], sorted by doc.
    postings: Vec<Vec<(u32, u16)>>,
    doc_len: Vec<u16>,
    avg_len: f32,
    /// Content-word term ids per doc, in order (for proximity scoring).
    content: Vec<Vec<u32>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeywordHit {
    pub doc: u32,
    pub score: f32,
    /// All distinct query terms matched.
    pub full: bool,
    /// Every adjacent pair of query content words matched adjacently.
    pub phrase: bool,
}

impl KeywordIndex {
    pub fn build<S: AsRef<str>>(docs: &[S]) -> KeywordIndex {
        let mut vocab: HashMap<String, u32> = HashMap::new();
        let mut postings: Vec<Vec<(u32, u16)>> = Vec::new();
        let mut doc_len = Vec::with_capacity(docs.len());
        let mut content = Vec::with_capacity(docs.len());
        let intern =
            |t: String, vocab: &mut HashMap<String, u32>, postings: &mut Vec<Vec<(u32, u16)>>| {
                let next = vocab.len() as u32;
                let id = *vocab.entry(t).or_insert(next);
                if id as usize == postings.len() {
                    postings.push(Vec::new());
                }
                id
            };
        for (d, doc) in docs.iter().enumerate() {
            let toks = stems(doc.as_ref());
            doc_len.push(toks.len().min(u16::MAX as usize) as u16);
            let mut tf: HashMap<u32, u16> = HashMap::new();
            for t in toks {
                let id = intern(t, &mut vocab, &mut postings);
                *tf.entry(id).or_default() += 1;
            }
            let mut tf: Vec<_> = tf.into_iter().collect();
            tf.sort_unstable();
            for (id, n) in tf {
                postings[id as usize].push((d as u32, n));
            }
            content.push(
                content_stems(doc.as_ref())
                    .into_iter()
                    .map(|t| intern(t, &mut vocab, &mut postings))
                    .collect(),
            );
        }
        let avg_len = if doc_len.is_empty() {
            1.0
        } else {
            doc_len.iter().map(|&l| l as f32).sum::<f32>() / doc_len.len() as f32
        };
        KeywordIndex {
            vocab,
            postings,
            doc_len,
            avg_len,
            content,
        }
    }

    pub fn len(&self) -> usize {
        self.doc_len.len()
    }

    pub fn is_empty(&self) -> bool {
        self.doc_len.is_empty()
    }

    fn idf(&self, term: u32) -> f32 {
        let n = self.len() as f32;
        let df = self.postings[term as usize].len() as f32;
        ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
    }

    /// Top `k` docs for the query, best first. Docs must match at least one term.
    pub fn search(&self, query: &str, k: usize) -> Vec<KeywordHit> {
        let mut terms: Vec<u32> = stems(query)
            .iter()
            .filter_map(|t| self.vocab.get(t).copied())
            .collect();
        terms.sort_unstable();
        terms.dedup();
        let n_query_terms = {
            let mut all = stems(query);
            all.sort();
            all.dedup();
            all.len()
        };
        if terms.is_empty() || k == 0 {
            return Vec::new();
        }
        // Content words of the query, in order; unknown words get u32::MAX (never match).
        let qcontent: Vec<u32> = content_stems(query)
            .iter()
            .map(|t| self.vocab.get(t).copied().unwrap_or(u32::MAX))
            .collect();

        let mut scores: HashMap<u32, (f32, u16)> = HashMap::new();
        for &t in &terms {
            let idf = self.idf(t);
            for &(doc, tf) in &self.postings[t as usize] {
                let tf = tf as f32;
                let norm = 1.0 - B + B * self.doc_len[doc as usize] as f32 / self.avg_len;
                let s = idf * tf * (K1 + 1.0) / (tf + K1 * norm);
                let e = scores.entry(doc).or_default();
                e.0 += s;
                e.1 += 1;
            }
        }
        let mut hits: Vec<KeywordHit> = scores
            .into_iter()
            .map(|(doc, (bm25, matched))| {
                let coverage = matched as f32 / n_query_terms.max(1) as f32;
                let (pairs, possible) = self.adjacent_pairs(doc, &qcontent);
                let phrase_factor = if possible > 0 {
                    pairs as f32 / possible as f32
                } else {
                    0.0
                };
                KeywordHit {
                    doc,
                    score: bm25 * coverage * coverage * (1.0 + PHRASE_BONUS * phrase_factor),
                    full: matched as usize == n_query_terms,
                    phrase: possible > 0 && pairs == possible,
                }
            })
            .collect();
        let cmp =
            |a: &KeywordHit, b: &KeywordHit| b.score.total_cmp(&a.score).then(a.doc.cmp(&b.doc));
        if hits.len() > k {
            hits.select_nth_unstable_by(k - 1, cmp);
            hits.truncate(k);
        }
        hits.sort_unstable_by(cmp);
        hits
    }

    /// (pairs matched, pairs possible) for consecutive query content words.
    fn adjacent_pairs(&self, doc: u32, q: &[u32]) -> (usize, usize) {
        if q.len() < 2 {
            return (0, 0);
        }
        let d = &self.content[doc as usize];
        let matched = q
            .windows(2)
            .filter(|p| p[0] != u32::MAX && d.windows(2).any(|w| w[0] == p[0] && w[1] == p[1]))
            .count();
        (matched, q.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> KeywordIndex {
        KeywordIndex::build(&[
            "It is easier for a camel to go through the eye of a needle",
            "If thine eye offend thee, pluck it out",
            "Thread the needle carefully",
            "The light of the body is the eye",
            "Blessed are the meek: for they shall inherit the earth",
        ])
    }

    #[test]
    fn phrase_match_ranks_first() {
        let hits = idx().search("eye of the needle", 10);
        assert_eq!(hits[0].doc, 0);
        assert!(hits[0].full && hits[0].phrase);
        assert!(hits.iter().skip(1).all(|h| !h.phrase));
    }

    #[test]
    fn stemming_matches_inflections() {
        let hits = idx().search("bless meekness", 10);
        assert!(hits.iter().any(|h| h.doc == 4));
        let hits = idx().search("needles", 10);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn no_match_and_limits() {
        assert!(idx().search("zebra", 10).is_empty());
        assert!(idx().search("", 10).is_empty());
        assert!(idx().search("eye", 0).is_empty());
        assert_eq!(idx().search("eye", 2).len(), 2);
    }

    #[test]
    fn results_are_sorted_and_deterministic() {
        let a = idx().search("the eye", 10);
        let b = idx().search("the eye", 10);
        assert_eq!(a, b);
        assert!(a.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn partial_coverage_is_penalised() {
        // doc 0 has both words; doc 2 only "needle".
        let hits = idx().search("camel needle", 10);
        assert_eq!(hits[0].doc, 0);
        assert!(hits[0].full);
        assert!(!hits.iter().find(|h| h.doc == 2).unwrap().full);
    }
}
