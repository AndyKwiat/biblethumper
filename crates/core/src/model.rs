//! The Bible data model: books, chapters, verses and plain-English sections.
//!
//! Source data lives in `data/gospels.json` (KJV text, made by `tools/import_kjv.py`)
//! and `data/sections.json` (summaries, validated by `tools/check_sections.py`).
//! Both are compiled into the binary with `include_str!`, so there is nothing to
//! deploy or locate at runtime.
//!
//! Everything is stored in flat vectors and addressed by index (`VerseId`,
//! `SectionId`), which keeps lookups O(1) and makes the search indexes simple
//! arrays parallel to `Bible::verses` / `Bible::sections`.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const GOSPELS_JSON: &str = include_str!("../../../data/gospels.json");
pub const SECTIONS_JSON: &str = include_str!("../../../data/sections.json");

/// Index into `Bible::verses`.
pub type VerseId = u32;
/// Index into `Bible::sections`.
pub type SectionId = u32;

/// A styled run of verse text. `wj` = words of Jesus (red letters),
/// `add` = words supplied by the translators (italic in print).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Seg {
    pub t: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub wj: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub add: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone)]
pub struct Verse {
    pub book: u8,
    pub chapter: u16,
    pub verse: u16,
    pub text: String,
    /// A KJV paragraph starts here.
    pub para: bool,
    pub segs: Vec<Seg>,
    pub section: SectionId,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub book: u8,
    pub chapter: u16,
    pub start: u16,
    pub end: u16,
    pub title: String,
    pub summary: String,
    pub first_verse: VerseId,
}

#[derive(Debug, Clone)]
pub struct Chapter {
    pub number: u16,
    pub first_verse: VerseId,
    pub verse_count: u16,
    pub first_section: SectionId,
    pub section_count: u16,
}

#[derive(Debug, Clone)]
pub struct Book {
    /// USFM code: MAT, MRK, LUK, JHN.
    pub id: String,
    pub name: String,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone)]
pub struct Bible {
    pub translation: String,
    pub books: Vec<Book>,
    pub verses: Vec<Verse>,
    pub sections: Vec<Section>,
}

// ---- raw JSON shapes -------------------------------------------------------

#[derive(Deserialize)]
struct RawBible {
    translation: String,
    books: Vec<RawBook>,
}
#[derive(Deserialize)]
struct RawBook {
    id: String,
    name: String,
    chapters: Vec<RawChapter>,
}
#[derive(Deserialize)]
struct RawChapter {
    number: u16,
    verses: Vec<RawVerse>,
}
#[derive(Deserialize)]
struct RawVerse {
    verse: u16,
    text: String,
    #[serde(default)]
    para: bool,
    segs: Vec<Seg>,
}
#[derive(Deserialize)]
pub struct RawSection {
    pub book: String,
    pub chapter: u16,
    pub start: u16,
    pub end: u16,
    pub title: String,
    pub summary: String,
}

impl Bible {
    /// Load the data compiled into the binary.
    pub fn embedded() -> Result<Bible> {
        Bible::from_json(GOSPELS_JSON, SECTIONS_JSON)
    }

    /// Parse and cross-link verses and sections. Fails if the sections do not
    /// cover every verse exactly once, in order.
    pub fn from_json(gospels: &str, sections: &str) -> Result<Bible> {
        let raw: RawBible = serde_json::from_str(gospels).context("parsing gospels json")?;
        let raw_sections: Vec<RawSection> =
            serde_json::from_str(sections).context("parsing sections json")?;

        let mut books = Vec::new();
        let mut verses = Vec::new();
        for (bi, rb) in raw.books.into_iter().enumerate() {
            let mut chapters = Vec::new();
            for rc in rb.chapters {
                let first_verse = verses.len() as VerseId;
                let verse_count = rc.verses.len() as u16;
                for (i, rv) in rc.verses.into_iter().enumerate() {
                    if rv.verse as usize != i + 1 {
                        bail!("{} {}: verse {} out of order", rb.id, rc.number, rv.verse);
                    }
                    verses.push(Verse {
                        book: bi as u8,
                        chapter: rc.number,
                        verse: rv.verse,
                        text: rv.text,
                        para: rv.para,
                        segs: rv.segs,
                        section: u32::MAX,
                    });
                }
                chapters.push(Chapter {
                    number: rc.number,
                    first_verse,
                    verse_count,
                    first_section: 0,
                    section_count: 0,
                });
            }
            books.push(Book {
                id: rb.id,
                name: rb.name,
                chapters,
            });
        }

        let mut bible = Bible {
            translation: raw.translation,
            books,
            verses,
            sections: Vec::new(),
        };
        for rs in raw_sections {
            let book = bible
                .book_index(&rs.book)
                .with_context(|| format!("section for unknown book {}", rs.book))?;
            let first_verse = bible
                .verse_id(book, rs.chapter, rs.start)
                .with_context(|| {
                    format!(
                        "section {} {}:{} out of range",
                        rs.book, rs.chapter, rs.start
                    )
                })?;
            let sid = bible.sections.len() as SectionId;
            {
                let ch = bible.chapter_mut(book, rs.chapter).unwrap();
                if ch.section_count == 0 {
                    ch.first_section = sid;
                }
                ch.section_count += 1;
            }
            for v in rs.start..=rs.end {
                let vid = bible.verse_id(book, rs.chapter, v).with_context(|| {
                    format!(
                        "section {} {}:{}-{} past chapter end",
                        rs.book, rs.chapter, rs.start, rs.end
                    )
                })?;
                let verse = &mut bible.verses[vid as usize];
                if verse.section != u32::MAX {
                    bail!("{} {}:{} is in two sections", rs.book, rs.chapter, v);
                }
                verse.section = sid;
            }
            bible.sections.push(Section {
                book,
                chapter: rs.chapter,
                start: rs.start,
                end: rs.end,
                title: rs.title,
                summary: rs.summary,
                first_verse,
            });
        }
        if let Some(v) = bible.verses.iter().find(|v| v.section == u32::MAX) {
            bail!(
                "{} {}:{} has no section",
                bible.books[v.book as usize].id,
                v.chapter,
                v.verse
            );
        }
        Ok(bible)
    }

    /// Book index by USFM id (case-insensitive), e.g. "MAT".
    pub fn book_index(&self, id: &str) -> Option<u8> {
        self.books
            .iter()
            .position(|b| b.id.eq_ignore_ascii_case(id))
            .map(|i| i as u8)
    }

    pub fn chapter(&self, book: u8, chapter: u16) -> Option<&Chapter> {
        let b = self.books.get(book as usize)?;
        b.chapters.get((chapter as usize).checked_sub(1)?)
    }

    fn chapter_mut(&mut self, book: u8, chapter: u16) -> Option<&mut Chapter> {
        let b = self.books.get_mut(book as usize)?;
        b.chapters.get_mut((chapter as usize).checked_sub(1)?)
    }

    pub fn verse_id(&self, book: u8, chapter: u16, verse: u16) -> Option<VerseId> {
        let ch = self.chapter(book, chapter)?;
        if verse == 0 || verse > ch.verse_count {
            return None;
        }
        Some(ch.first_verse + verse as u32 - 1)
    }

    /// "Matthew 19:24"
    pub fn verse_label(&self, id: VerseId) -> String {
        let v = &self.verses[id as usize];
        format!(
            "{} {}:{}",
            self.books[v.book as usize].name, v.chapter, v.verse
        )
    }

    /// "Luke 15:11–32"
    pub fn section_label(&self, id: SectionId) -> String {
        let s = &self.sections[id as usize];
        let name = &self.books[s.book as usize].name;
        if s.start == s.end {
            format!("{name} {}:{}", s.chapter, s.start)
        } else {
            format!("{name} {}:{}\u{2013}{}", s.chapter, s.start, s.end)
        }
    }

    /// Text used to embed a section: its heading plus the plain-English summary.
    /// Modern-language queries ("Jesus walks on water") match this well.
    pub fn section_embed_text(&self, id: SectionId) -> String {
        let s = &self.sections[id as usize];
        format!("{}. {}", s.title, s.summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    pub(crate) fn bible() -> &'static Bible {
        static B: OnceLock<Bible> = OnceLock::new();
        B.get_or_init(|| Bible::embedded().unwrap())
    }

    #[test]
    fn has_four_gospels_with_kjv_verse_counts() {
        let b = bible();
        let names: Vec<_> = b.books.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Matthew", "Mark", "Luke", "John"]);
        let chapters: Vec<_> = b.books.iter().map(|b| b.chapters.len()).collect();
        assert_eq!(chapters, [28, 16, 24, 21]);
        assert_eq!(b.verses.len(), 3779);
    }

    #[test]
    fn every_verse_has_a_section_and_sections_are_contiguous() {
        let b = bible();
        for (sid, s) in b.sections.iter().enumerate() {
            assert!(s.start <= s.end);
            for v in s.start..=s.end {
                let vid = b.verse_id(s.book, s.chapter, v).unwrap();
                assert_eq!(b.verses[vid as usize].section, sid as u32);
            }
            assert!(!s.title.trim().is_empty() && !s.summary.trim().is_empty());
        }
        for book in &b.books {
            for ch in &book.chapters {
                assert!(
                    ch.section_count > 0,
                    "{} {} has no sections",
                    book.id,
                    ch.number
                );
            }
        }
    }

    #[test]
    fn verse_lookup_and_labels() {
        let b = bible();
        let mat = b.book_index("mat").unwrap();
        let id = b.verse_id(mat, 19, 24).unwrap();
        assert!(b.verses[id as usize].text.contains("eye of a needle"));
        assert_eq!(b.verse_label(id), "Matthew 19:24");
        assert_eq!(b.verse_id(mat, 19, 0), None);
        assert_eq!(b.verse_id(mat, 29, 1), None);
        assert_eq!(b.verse_id(mat, 0, 1), None);
        let jhn = b.book_index("JHN").unwrap();
        assert_eq!(
            b.verses[b.verse_id(jhn, 11, 35).unwrap() as usize].text,
            "Jesus wept."
        );
    }

    #[test]
    fn words_of_jesus_are_marked() {
        let b = bible();
        let jhn = b.book_index("JHN").unwrap();
        let v = &b.verses[b.verse_id(jhn, 3, 16).unwrap() as usize];
        assert!(v.segs.iter().any(|s| s.wj));
        let joined: String = v.segs.iter().map(|s| s.t.as_str()).collect();
        assert_eq!(joined, v.text);
    }

    #[test]
    fn rejects_sections_with_gaps() {
        let gospels = r#"{"translation":"T","books":[{"id":"MAT","name":"Matthew","chapters":[
            {"number":1,"verses":[{"verse":1,"text":"a","segs":[{"t":"a"}]},{"verse":2,"text":"b","segs":[{"t":"b"}]}]}]}]}"#;
        let ok = r#"[{"book":"MAT","chapter":1,"start":1,"end":2,"title":"t","summary":"s"}]"#;
        let gap = r#"[{"book":"MAT","chapter":1,"start":1,"end":1,"title":"t","summary":"s"}]"#;
        let overlap = r#"[{"book":"MAT","chapter":1,"start":1,"end":2,"title":"t","summary":"s"},
                          {"book":"MAT","chapter":1,"start":2,"end":2,"title":"t","summary":"s"}]"#;
        let past_end =
            r#"[{"book":"MAT","chapter":1,"start":1,"end":3,"title":"t","summary":"s"}]"#;
        assert!(Bible::from_json(gospels, ok).is_ok());
        assert!(Bible::from_json(gospels, gap).is_err());
        assert!(Bible::from_json(gospels, overlap).is_err());
        assert!(Bible::from_json(gospels, past_end).is_err());
    }
}
