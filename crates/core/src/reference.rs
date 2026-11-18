//! Parse Bible references typed into the search box: "John 3:16", "jn 3 16",
//! "Matt. 5:3-12", "Mk 4", "Luke". Pure string parsing; checking that the
//! chapter/verse exists is done by the caller against `Bible`.

/// A parsed reference. `book` is the index into `Bible::books`
/// (0 = Matthew, 1 = Mark, 2 = Luke, 3 = John).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reference {
    pub book: u8,
    pub chapter: u16,
    pub verse_start: Option<u16>,
    pub verse_end: Option<u16>,
}

/// Accepted spellings for each book (lowercase, without dots).
/// Add aliases here when adding books.
pub const BOOK_ALIASES: &[(&str, u8)] = &[
    ("matthew", 0),
    ("matt", 0),
    ("mat", 0),
    ("mt", 0),
    ("mathew", 0),
    ("mark", 1),
    ("mrk", 1),
    ("mk", 1),
    ("mr", 1),
    ("luke", 2),
    ("luk", 2),
    ("lk", 2),
    ("lu", 2),
    ("john", 3),
    ("jhn", 3),
    ("jn", 3),
    ("joh", 3),
];

/// Returns the book index for a name or abbreviation.
pub fn book_from_name(name: &str) -> Option<u8> {
    let n: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '.')
        .collect::<String>()
        .to_ascii_lowercase();
    let n = n
        .strip_prefix("st")
        .filter(|rest| BOOK_ALIASES.iter().any(|(a, _)| a == rest))
        .unwrap_or(&n);
    BOOK_ALIASES.iter().find(|(a, _)| *a == n).map(|(_, b)| *b)
}

/// Parse a whole query as a reference. Returns None for anything else, so
/// "john the baptist" is a normal search but "john 1" is a reference.
pub fn parse_reference(input: &str) -> Option<Reference> {
    let lower = input.trim().to_ascii_lowercase();
    let s = lower.as_str();
    if s.is_empty() || s.len() > 40 {
        return None;
    }
    // Split into the leading name and the numeric tail.
    let digit_at = s.find(|c: char| c.is_ascii_digit());
    let (name, tail) = match digit_at {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let name = name.trim().trim_start_matches("the gospel of ").trim();
    let name = name.strip_prefix("gospel of ").unwrap_or(name);
    let book = book_from_name(name)?;

    let nums: Vec<&str> = tail
        .split([':', '.', ' ', ',', 'v'])
        .filter(|p| !p.is_empty())
        .collect();
    let chapter_and_rest = nums.join(" ");
    let mut parts = chapter_and_rest.split_whitespace();

    let chapter: u16 = match parts.next() {
        None => {
            return Some(Reference {
                book,
                chapter: 1,
                verse_start: None,
                verse_end: None,
            });
        }
        Some(p) => p.parse().ok()?,
    };
    let (verse_start, verse_end) = match parts.next() {
        None => (None, None),
        Some(p) => {
            let mut range = p.split(['-', '\u{2013}', '\u{2014}']);
            let a: u16 = range.next()?.parse().ok()?;
            let b: Option<u16> = match range.next() {
                Some(x) if !x.is_empty() => Some(x.parse().ok()?),
                _ => None,
            };
            if range.next().is_some() {
                return None;
            }
            (Some(a), b)
        }
    };
    if parts.next().is_some() || chapter == 0 || verse_start == Some(0) {
        return None;
    }
    if let (Some(a), Some(b)) = (verse_start, verse_end)
        && b < a
    {
        return None;
    }
    Some(Reference {
        book,
        chapter,
        verse_start,
        verse_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(book: u8, chapter: u16, a: Option<u16>, b: Option<u16>) -> Option<Reference> {
        Some(Reference {
            book,
            chapter,
            verse_start: a,
            verse_end: b,
        })
    }

    #[test]
    fn full_references() {
        assert_eq!(parse_reference("John 3:16"), r(3, 3, Some(16), None));
        assert_eq!(
            parse_reference("  matthew 19:24 "),
            r(0, 19, Some(24), None)
        );
        assert_eq!(
            parse_reference("Luke 15:11-32"),
            r(2, 15, Some(11), Some(32))
        );
        assert_eq!(
            parse_reference("Luke 15:11\u{2013}32"),
            r(2, 15, Some(11), Some(32))
        );
        assert_eq!(parse_reference("Mark 1.1"), r(1, 1, Some(1), None));
    }

    #[test]
    fn abbreviations_and_spacing() {
        assert_eq!(parse_reference("jn 3 16"), r(3, 3, Some(16), None));
        assert_eq!(parse_reference("Matt. 5:3"), r(0, 5, Some(3), None));
        assert_eq!(parse_reference("mt5:3"), r(0, 5, Some(3), None));
        assert_eq!(parse_reference("Mk 4"), r(1, 4, None, None));
        assert_eq!(parse_reference("St. John 1:1"), r(3, 1, Some(1), None));
        assert_eq!(parse_reference("gospel of luke 2"), r(2, 2, None, None));
    }

    #[test]
    fn bare_book_goes_to_chapter_one() {
        assert_eq!(parse_reference("Luke"), r(2, 1, None, None));
        assert_eq!(parse_reference("JOHN"), r(3, 1, None, None));
    }

    #[test]
    fn not_references() {
        for q in [
            "",
            "eye of the needle",
            "john the baptist",
            "love",
            "3:16",
            "john 0",
            "john 3:0",
            "john 3:16-10",
            "john 3:16:17:18",
            "genesis 1:1",
            "john 3:x",
        ] {
            assert_eq!(parse_reference(q), None, "{q:?} should not parse");
        }
    }

    #[test]
    fn book_names() {
        assert_eq!(book_from_name("Mathew"), Some(0));
        assert_eq!(book_from_name("stmark"), Some(1));
        assert_eq!(book_from_name("star"), None);
    }
}
