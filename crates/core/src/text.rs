//! Tokenising and stemming for keyword search.
//!
//! Deliberately tiny: lowercase ASCII words, a crude suffix stripper that folds
//! KJV archaic forms ("believeth", "lovest") onto modern ones ("believe",
//! "love"), and a stopword list used for phrase/proximity scoring.

/// Split into lowercase words. Apostrophes are dropped ("needle's" -> "needles").
pub fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '\u{2019}'))
        .filter(|w| !w.is_empty())
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
}

/// Very small stemmer. Same input always maps to the same stem, which is all
/// BM25 needs. Examples: believeth/believed/believing/believe -> "believ",
/// needle/needles -> "needl", loveth/loved/love -> "lov".
pub fn stem(word: &str) -> String {
    const MIN: usize = 3;
    let mut w = word.to_string();
    for suf in ["eth", "est", "ing", "ed", "es", "s", "e"] {
        if w.len() >= suf.len() + MIN && w.ends_with(suf) && !(suf == "s" && w.ends_with("ss")) {
            w.truncate(w.len() - suf.len());
            break;
        }
    }
    // "loved" -> "lov" but "seed" -> "se": guard short words already handled by MIN.
    w
}

/// Words that carry little meaning on their own (modern + KJV).
pub const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "he", "her", "him",
    "his", "i", "in", "into", "is", "it", "me", "my", "not", "of", "on", "or", "so", "that", "the",
    "their", "them", "then", "there", "they", "this", "to", "unto", "was", "we", "were", "which",
    "with", "ye", "you", "your", "thee", "thou", "thy", "hath", "had", "have", "shall", "will",
    "said", "saith", "what", "who", "when", "all", "also", "do", "did",
];

pub fn is_stopword(w: &str) -> bool {
    STOPWORDS.contains(&w)
}

/// Stems of all words, in order (for BM25 term frequencies).
pub fn stems(text: &str) -> Vec<String> {
    words(text).map(|w| stem(&w)).collect()
}

/// Stems of non-stopwords, in order (for phrase/proximity matching).
pub fn content_stems(text: &str) -> Vec<String> {
    words(text)
        .filter(|w| !is_stopword(w))
        .map(|w| stem(&w))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_lowercases() {
        let w: Vec<_> = words("And Jesus said, Follow me! (needle's eye)").collect();
        assert_eq!(
            w,
            ["and", "jesus", "said", "follow", "me", "needles", "eye"]
        );
    }

    #[test]
    fn stems_archaic_forms_together() {
        for w in ["believeth", "believed", "believing", "believe"] {
            assert_eq!(stem(w), "believ", "{w}");
        }
        assert_eq!(stem("needle"), stem("needles"));
        assert_eq!(stem("loveth"), stem("love"));
        assert_eq!(stem("lovest"), stem("loved"));
        assert_eq!(stem("blessed"), "bless");
        assert_eq!(stem("his"), "his");
        assert_eq!(stem("eye"), "eye");
        assert_eq!(stem("cross"), "cross");
    }

    #[test]
    fn content_stems_drop_stopwords() {
        assert_eq!(content_stems("the eye of a needle"), ["eye", "needl"]);
        assert_eq!(content_stems("eye of the needle"), ["eye", "needl"]);
    }
}
