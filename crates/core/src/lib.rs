//! bt-core: everything Bible Thumper knows, independent of HTTP.
//!
//! - [`model`]: verses, chapters and plain-English sections (data compiled in)
//! - [`reference`]: parse "John 3:16"-style references
//! - [`text`] + [`keyword`]: tokenising, stemming, BM25 keyword index
//! - [`embed`] + [`vector`]: sentence embeddings and the flat vector index/file
//! - [`search`]: the hybrid search engine that ties it together
pub mod embed;
pub mod keyword;
pub mod model;
pub mod reference;
pub mod search;
pub mod text;
pub mod vector;

pub use model::Bible;
pub use search::{Engine, SearchOptions, SearchResponse};
